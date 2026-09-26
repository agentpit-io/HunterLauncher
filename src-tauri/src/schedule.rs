//! 定时备份的系统级定时任务（I13 · R6 6.3）。
//!
//! **启动器没开的时候也要能备份**，所以这件事必须交给操作系统自己的定时机制，
//! 不能靠启动器常驻。三个平台各一套：
//!
//! | 平台 | 机制 | 错过了会怎样 |
//! |---|---|---|
//! | macOS | 用户级 LaunchAgent，`StartCalendarInterval` | launchd 在**唤醒之后补跑**（这是它自己的语义，不是我们做的） |
//! | Linux | `systemd --user` 定时器，`Persistent=true` | systemd 记下上次跑的时间，开机/唤醒后补跑 |
//! | Windows | 任务计划程序，`StartWhenAvailable` | 「错过后尽快运行」 |
//!
//! 三条路跑的都是同一条命令：`hunter-launcher --backup --scheduled`
//! （[`crate::headless`] 里那个无界面入口）。
//!
//! ## 三件不做的事
//!
//! 1. **不用 root、不装系统级服务。** macOS 走 `~/Library/LaunchAgents`（用户级），
//!    Linux 走 `systemd --user`，Windows 建的是当前用户的任务。
//!    全程没有一个 `sudo`，不弹管理员密码框。
//! 2. **不让用户自己去终端敲命令。** 安装、改时间、移除全由启动器完成
//!    （设置页的开关与时间框），界面上不出现任何「请执行……」。
//! 3. **不替用户开 linger。** Linux 上 `systemd --user` 的定时器默认只在
//!    用户有会话时活着；`loginctl enable-linger` 是一条会改变系统行为的命令，
//!    我们只**如实报告**这个状态（[`Status::lines`]），不替他做决定。
//!
//! ## 删除应用时一并移除
//!
//! 定时任务是「启动器自己的东西」（方案第六节第 3 条），
//! 所以 [`crate::uninstall`] 的两种范围都会调 [`remove`]。

use std::path::PathBuf;
use std::time::Duration;

use serde::Serialize;

use crate::err::{AppError, AppResult, Code};

/// LaunchAgent 的 Label、systemd 的单元名前缀、任务计划的任务名 —— 只有这一处。
pub const LABEL: &str = "io.agentpit.hunter-backup";
/// systemd 单元名（`--user`）。
pub const UNIT: &str = "hunter-backup";
/// Windows 任务计划里的任务名。**ASCII**：中文任务名在某些区域设置下 `schtasks` 会认不出来。
pub const TASK: &str = "HunterLauncherBackup";

const T: Duration = Duration::from_secs(30);

/// 这台机器上用哪套机制。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mech {
    LaunchAgent,
    SystemdUser,
    SchTasks,
    /// 认得出平台，但这台机器上缺了那套机制（例如没有 systemd 的 Linux）
    None,
}

impl Mech {
    pub fn cn(self) -> &'static str {
        match self {
            Mech::LaunchAgent => "macOS 用户级 LaunchAgent",
            Mech::SystemdUser => "systemd --user 定时器",
            Mech::SchTasks => "Windows 任务计划程序",
            Mech::None => "这台机器上没有可用的定时机制",
        }
    }
}

/// 定时任务的现状。**「装没装」一律现查系统**，不读配置里的备忘（红线 1）。
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub mech: String,
    pub supported: bool,
    /// 系统里真的有这个任务吗
    pub installed: bool,
    /// 装着而且是启用的吗（systemd 的 enabled、launchd 的已 bootstrap、schtasks 的 Ready）
    pub enabled: bool,
    /// 配置里的开关（用户想不想要）
    pub wanted: bool,
    /// 配置里的时间 `HH:MM`
    pub time: String,
    /// 任务文件在哪（plist / unit / 任务名）
    pub path: String,
    /// 系统报的「下次什么时候跑」。问不到就是空 + 一句原因（不猜）
    pub next_run: String,
    /// 逐条实测原话
    pub lines: Vec<String>,
    /// 查不了 / 装不了的原因
    pub reason: String,
}

/// 这台机器用哪套。
pub fn mech() -> Mech {
    if cfg!(target_os = "macos") {
        return Mech::LaunchAgent;
    }
    if cfg!(windows) {
        return Mech::SchTasks;
    }
    if cfg!(target_os = "linux") {
        // systemctl 不在就没有这条路 —— 如实说，不假装装上了
        if crate::runtime::which::find_program("systemctl").is_some() {
            return Mech::SystemdUser;
        }
        return Mech::None;
    }
    Mech::None
}

/// 启动器自己的可执行文件（定时任务要跑的就是它）。
pub fn exe() -> AppResult<PathBuf> {
    std::env::current_exe()
        .map_err(|e| AppError::new(Code::Unknown, format!("定位不到启动器自己的位置：{e}")))
}

pub fn plist_path() -> PathBuf {
    crate::paths::home()
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{LABEL}.plist"))
}

pub fn systemd_dir() -> PathBuf {
    crate::paths::home()
        .join(".config")
        .join("systemd")
        .join("user")
}

pub fn service_path() -> PathBuf {
    systemd_dir().join(format!("{UNIT}.service"))
}

pub fn timer_path() -> PathBuf {
    systemd_dir().join(format!("{UNIT}.timer"))
}

/// 定时任务自己的日志（三个平台都把 stdout/stderr 接到这里）。
/// 没有界面的那一次跑失败了，这是唯一能查的地方。
pub fn log_path() -> PathBuf {
    crate::paths::logs_dir().join("backup-schedule.log")
}

// ── 文件内容 ──────────────────────────────────────────────────────────────

/// LaunchAgent 的 plist。**纯函数**，方便单测逐条核字段。
pub fn plist_text(exe: &str, hour: u8, minute: u8, hunter_home: Option<&str>, log: &str) -> String {
    let envs = match hunter_home {
        Some(h) => format!(
            "  <key>EnvironmentVariables</key>\n  <dict>\n    \
             <key>HUNTER_HOME</key>\n    <string>{}</string>\n  </dict>\n",
            xml(h)
        ),
        None => String::new(),
    };
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n  \
           <key>Label</key>\n  <string>{LABEL}</string>\n  \
           <key>ProgramArguments</key>\n  <array>\n    \
             <string>{exe}</string>\n    <string>--backup</string>\n    \
             <string>--scheduled</string>\n  </array>\n  \
           <key>StartCalendarInterval</key>\n  <dict>\n    \
             <key>Hour</key>\n    <integer>{hour}</integer>\n    \
             <key>Minute</key>\n    <integer>{minute}</integer>\n  </dict>\n  \
           <key>RunAtLoad</key>\n  <false/>\n  \
           <key>ProcessType</key>\n  <string>Background</string>\n  \
           <key>StandardOutPath</key>\n  <string>{log}</string>\n  \
           <key>StandardErrorPath</key>\n  <string>{log}</string>\n\
         {envs}</dict>\n\
         </plist>\n",
        exe = xml(exe),
        log = xml(log),
    )
}

/// systemd 的 `.service`。
///
/// 可执行文件路径与 `HUNTER_HOME` 都**加双引号**：AppImage 常常躺在
/// `~/下载/Hunter 启动器.AppImage` 这种带空格的路径下，不加引号 systemd 会把
/// 空格当成参数分隔符，`ExecStart` 直接解析失败（而且失败得很安静 ——
/// 只在 `systemctl --user status` 里留一行）。systemd 自己会把引号剥掉。
pub fn service_text(exe: &str, hunter_home: Option<&str>) -> String {
    let env = match hunter_home {
        Some(h) => format!("Environment=\"HUNTER_HOME={h}\"\n"),
        None => String::new(),
    };
    format!(
        "# 由 Hunter 启动器生成 · 请勿手改（改了下次保存会被覆盖）\n\
         [Unit]\n\
         Description=Hunter 启动器 · 本机数据备份\n\
         \n\
         [Service]\n\
         Type=oneshot\n\
         {env}\
         ExecStart=\"{exe}\" --backup --scheduled\n"
    )
}

/// systemd 的 `.timer`。`Persistent=true` 就是「错过了补跑」。
pub fn timer_text(hour: u8, minute: u8) -> String {
    format!(
        "# 由 Hunter 启动器生成 · 请勿手改（改了下次保存会被覆盖）\n\
         [Unit]\n\
         Description=Hunter 启动器 · 本机数据备份定时器\n\
         \n\
         [Timer]\n\
         OnCalendar=*-*-* {hour:02}:{minute:02}:00\n\
         Persistent=true\n\
         AccuracySec=1min\n\
         Unit={UNIT}.service\n\
         \n\
         [Install]\n\
         WantedBy=timers.target\n"
    )
}

/// Windows 任务计划的 XML。`StartWhenAvailable` 就是「错过后尽快运行」。
///
/// **I16 改了两处**（客户那台中文 Windows 上 `schtasks /Create` 一直
/// 报 `错误: 未指定的错误`，见 [`install`] 的注释）：
///
/// 1. 补上 `<Principals>`。原先 `<Actions Context="Author">` 指向一个
///    **XML 里根本不存在的 principal** —— 任务计划的 schema 要求
///    `Context` 与某个 `<Principal id=…>` 对得上；
/// 2. `<Settings>` 的子元素按 schema 里的顺序排。
///
/// 两处都不是「我们确认的根因」（没有 Windows 机器，确认不了），
/// 而是「照着规范本来就该这么写」。真正保证它装得上的是
/// [`install`] 里那条**不用 XML 的兜底路**。
pub fn task_xml(exe: &str, hour: u8, minute: u8, _hunter_home: Option<&str>) -> String {
    // schtasks 的 XML 要求 UTF-16；写文件时再转（见 write_utf16）。这里只管内容。
    // Windows 上没法在 XML 里单独给环境变量，工作目录由 --backup 那条路自己定
    let args = TASK_ARGS;
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-16\"?>\n\
         <Task version=\"1.2\" xmlns=\"http://schemas.microsoft.com/windows/2004/02/mit/task\">\n  \
           <RegistrationInfo>\n    \
             <Description>Hunter 启动器 · 本机数据备份</Description>\n  \
           </RegistrationInfo>\n  \
           <Triggers>\n    <CalendarTrigger>\n      \
             <StartBoundary>2026-01-01T{hour:02}:{minute:02}:00</StartBoundary>\n      \
             <Enabled>true</Enabled>\n      \
             <ScheduleByDay><DaysInterval>1</DaysInterval></ScheduleByDay>\n    \
             </CalendarTrigger>\n  </Triggers>\n  \
           <Principals>\n    <Principal id=\"Author\">\n      \
             <LogonType>InteractiveToken</LogonType>\n      \
             <RunLevel>LeastPrivilege</RunLevel>\n    \
             </Principal>\n  </Principals>\n  \
           <Settings>\n    \
             <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>\n    \
             <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>\n    \
             <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>\n    \
             <StartWhenAvailable>true</StartWhenAvailable>\n    \
             <Enabled>true</Enabled>\n    \
             <ExecutionTimeLimit>PT2H</ExecutionTimeLimit>\n  </Settings>\n  \
           <Actions Context=\"Author\">\n    <Exec>\n      \
             <Command>{exe}</Command>\n      \
             <Arguments>{args}</Arguments>\n    </Exec>\n  </Actions>\n\
         </Task>\n",
        exe = xml(exe),
    )
}

/// 定时任务跑的那条命令的参数。XML 那条路与不用 XML 那条路共用同一份，
/// 免得两条路装出两个跑法不同的任务。
pub const TASK_ARGS: &str = "--backup --scheduled";

/// `<Settings>` 里子元素该有的顺序（任务计划 schema 的 `settingsType`）。
/// 单测拿它对账，防止以后有人往中间插一条把顺序又打乱。
#[cfg(test)]
const SETTINGS_ORDER: [&str; 6] = [
    "MultipleInstancesPolicy",
    "DisallowStartIfOnBatteries",
    "StopIfGoingOnBatteries",
    "StartWhenAvailable",
    "Enabled",
    "ExecutionTimeLimit",
];

fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

// ── 装 / 卸 / 查 ─────────────────────────────────────────────────────────

/// 按配置把定时任务弄成该有的样子（开关开着就装/更新，关着就卸）。
///
/// 设置页每改一次备份设置都会调它一次 —— **「配置里写着几点」和
/// 「系统里真的几点跑」永远不该是两件事**。
pub fn sync(mut note: impl FnMut(&str)) -> AppResult<Status> {
    let cfg = crate::config::LauncherConfig::load();
    if cfg.backup.enabled {
        install(&mut note)
    } else {
        remove(&mut note)?;
        Ok(status())
    }
}

/// 装（或更新）定时任务。**幂等**：已经装了就按新的时间重写一遍。
///
/// 这一层只做一件额外的事：**把成败记进配置**（I16 · P0-3）。
///
/// 客户那台 Windows 上「定时备份没装上」这件事，0.1.15 只往日志里写了一行
/// `装完挂定时备份没成（不影响安装）`，界面上一个字都没有 ——
/// 于是他以为自己有每日自动备份，实际上一次都没跑过。
/// **对用户来说「备份没了」就是天大的事**（I14 的教训：看不见的事等于没做）。
/// `[backup] schedule_error` 就是运行面板上那条红色横幅的来源。
pub fn install(note: impl FnMut(&str)) -> AppResult<Status> {
    let r = install_inner(note);
    let mut cfg = crate::config::LauncherConfig::load();
    let want = match &r {
        Ok(_) => String::new(),
        Err(e) => e.msg.clone(),
    };
    if cfg.backup.schedule_error != want {
        cfg.backup.schedule_error = want;
        let _ = cfg.save();
    }
    r
}

fn install_inner(mut note: impl FnMut(&str)) -> AppResult<Status> {
    let cfg = crate::config::LauncherConfig::load();
    let (h, m) = cfg.backup.hhmm();
    let exe = exe()?;
    let exe_s = exe.to_string_lossy().into_owned();
    let home = std::env::var("HUNTER_HOME").ok().filter(|s| !s.is_empty());
    crate::paths::ensure_dirs()?;
    let log = log_path().to_string_lossy().into_owned();

    match mech() {
        Mech::LaunchAgent => {
            let p = plist_path();
            if let Some(d) = p.parent() {
                std::fs::create_dir_all(d).map_err(|e| {
                    AppError::new(Code::ConfigWrite, format!("建 LaunchAgents 目录失败：{e}"))
                })?;
            }
            std::fs::write(&p, plist_text(&exe_s, h, m, home.as_deref(), &log)).map_err(|e| {
                AppError::new(Code::ConfigWrite, format!("写 {} 失败：{e}", p.display()))
            })?;
            let target = format!("gui/{}/{LABEL}", uid());
            // 先卸再装：改了时间之后不 bootout 的话 launchd 还认旧的那一份
            let _ = run(&["bootout", &target]);
            let ps = p.to_string_lossy().into_owned();
            let r = run(&["bootstrap", &format!("gui/{}", uid()), &ps])?;
            if !r.ok() {
                return Err(AppError::new(
                    Code::ConfigWrite,
                    format!("launchctl bootstrap 失败：{}", r.err_line()),
                ));
            }
            note(&format!(
                "已装好 macOS 定时任务（每天 {h:02}:{m:02}），它在 {}",
                crate::redact::mask_home(&ps)
            ));
        }
        Mech::SystemdUser => {
            std::fs::create_dir_all(systemd_dir()).map_err(|e| {
                AppError::new(Code::ConfigWrite, format!("建 systemd 用户目录失败：{e}"))
            })?;
            std::fs::write(service_path(), service_text(&exe_s, home.as_deref()))
                .map_err(|e| AppError::new(Code::ConfigWrite, format!("写 .service 失败：{e}")))?;
            std::fs::write(timer_path(), timer_text(h, m))
                .map_err(|e| AppError::new(Code::ConfigWrite, format!("写 .timer 失败：{e}")))?;
            let _ = run(&["--user", "daemon-reload"]);
            let unit = format!("{UNIT}.timer");
            let r = run(&["--user", "enable", "--now", &unit])?;
            if !r.ok() {
                return Err(AppError::new(
                    Code::ConfigWrite,
                    format!("systemctl --user enable --now 失败：{}", r.err_line()),
                ));
            }
            note(&format!(
                "已装好 systemd 用户定时器（每天 {h:02}:{m:02}，错过会补跑）"
            ));
        }
        Mech::SchTasks => {
            let (way, tried) = install_schtasks(h, m, &exe_s)?;
            for t in &tried {
                crate::lwarn!("schtasks：{t}");
            }
            note(&format!(
                "已装好 Windows 计划任务（每天 {h:02}:{m:02}{}）",
                way.tail()
            ));
        }
        Mech::None => {
            return Err(AppError::new(
                Code::NotImplemented,
                "这台机器上没有可用的定时机制（Linux 上需要 systemd --user）。\
                 自动备份没法在启动器关着的时候跑，界面上会如实写明。"
                    .to_string(),
            ))
        }
    }
    let mut cfg = crate::config::LauncherConfig::load();
    cfg.backup.schedule_installed = true;
    let _ = cfg.save();
    Ok(status())
}

/// 卸掉定时任务。**删除应用时也走这里**（方案第六节第 3 条）。
/// 本来就没装也返回 `Ok` —— 「已经是想要的样子」不是错误。
pub fn remove(mut note: impl FnMut(&str)) -> AppResult<()> {
    match mech() {
        Mech::LaunchAgent => {
            let _ = run(&["bootout", &format!("gui/{}/{LABEL}", uid())]);
            let p = plist_path();
            if p.exists() {
                std::fs::remove_file(&p).map_err(|e| {
                    AppError::new(Code::ConfigWrite, format!("删 {} 失败：{e}", p.display()))
                })?;
                note("已移除 macOS 定时备份任务");
            }
        }
        Mech::SystemdUser => {
            let unit = format!("{UNIT}.timer");
            let _ = run(&["--user", "disable", "--now", &unit]);
            for p in [timer_path(), service_path()] {
                if p.exists() {
                    let _ = std::fs::remove_file(&p);
                }
            }
            let _ = run(&["--user", "daemon-reload"]);
            note("已移除 systemd 用户定时器");
        }
        Mech::SchTasks => {
            let r = run(&["/Delete", "/TN", TASK, "/F"]);
            if matches!(&r, Ok(x) if x.ok()) {
                note("已移除 Windows 计划任务");
            }
        }
        Mech::None => {}
    }
    let mut cfg = crate::config::LauncherConfig::load();
    // 开关关了就不该再挂着一条「定时任务没装上」的红横幅 —— 那是用户自己的决定
    if cfg.backup.schedule_installed || !cfg.backup.schedule_error.is_empty() {
        cfg.backup.schedule_installed = false;
        cfg.backup.schedule_error = String::new();
        let _ = cfg.save();
    }
    Ok(())
}

/// Windows 上装定时任务走的是哪条路。两条路装出来的任务不一样，**得说出来**。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Way {
    /// `/XML`：带 `StartWhenAvailable`，错过的那一次开机后会补跑
    Xml,
    /// `/SC DAILY /ST`：schtasks 自己的参数，**没有「错过后补跑」**
    Simple,
}

impl Way {
    fn tail(self) -> &'static str {
        match self {
            Way::Xml => "，错过会补跑",
            // 老老实实说清楚少了什么（红线 1）：这条路装出来的任务，
            // 电脑在 00:00 时是关着的话，那一天就没有备份
            Way::Simple => "；这条路装出来的任务**错过了不补跑**（电脑当时关着就没有那一天的备份）",
        }
    }
}

/// 同一个进程里**一次只允许一个**装/卸定时任务。
///
/// 安装收尾（`flow.rs`）与开机自查（`lib.rs` 里那个 spawn 出来的线程）都会调
/// [`sync`]，两边撞上的时候会对着同一个任务名同时 `/Create`。
static SCHTASKS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 每次调用各用各的 XML 文件名。
///
/// 原先是写死的 `~/.hunter/hunter-backup-task.xml`：两处同时装的时候，
/// **一边的 `remove_file` 会把另一边正在读的那份删掉**，
/// schtasks 于是报一句什么都没说的 `错误: 未指定的错误`。
fn task_xml_path() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    crate::paths::root().join(format!(
        "hunter-backup-task-{}-{}.xml",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ))
}

/// 不用 XML 的那条路的参数。`/TR` 是**一个** argv 元素
/// （里头的引号是给 Windows 命令行用的，不是我们在拼 shell —— 红线 3）。
pub fn simple_argv(exe: &str, hour: u8, minute: u8) -> Vec<String> {
    vec![
        "/Create".into(),
        "/TN".into(),
        TASK.into(),
        "/TR".into(),
        format!("\"{exe}\" {TASK_ARGS}"),
        "/SC".into(),
        "DAILY".into(),
        "/ST".into(),
        format!("{hour:02}:{minute:02}"),
        "/F".into(),
    ]
}

/// Windows 上真正去装。**三步，前一步不成才走下一步**，每一步的原话都留下来。
///
/// 为什么要有第二、第三步：客户 2026-09-26 那份诊断包里，
/// `schtasks /Create … /XML … /F` 连着四次失败、中间成功过一次，
/// 报的全是同一句 `错误: 未指定的错误`（原文是 GBK，0.1.15 把它当 UTF-8
/// 解成了 `����: δָ���Ĵ���`，所以我们**一次都没看见过**）。
/// 「未指定的错误」就是任务计划服务说「我不告诉你」——
/// 没有 Windows 机器就查不出它到底嫌什么。
///
/// 于是这里不赌某一个猜想，而是把**不依赖 XML 的那条路**加上：
/// `/SC DAILY /ST HH:MM` 是 schtasks 自己的参数，一个 XML 文件都不碰。
/// 它换来的代价是没有 `StartWhenAvailable`（错过不补跑），
/// 所以只在 XML 那条路不成时才走，而且界面上要说明白。
///
/// 返回「最后是靠哪条路装上的」与「前面几步各报了什么」。
fn install_schtasks(h: u8, m: u8, exe_s: &str) -> AppResult<(Way, Vec<String>)> {
    let _g = SCHTASKS_LOCK.lock();
    let mut tried: Vec<String> = Vec::new();

    // ① 首选：XML（带「错过后补跑」）
    let xmlp = task_xml_path();
    write_utf16(&xmlp, &task_xml(exe_s, h, m, None))?;
    let xs = xmlp.to_string_lossy().into_owned();
    let r1 = run(&["/Create", "/TN", TASK, "/XML", &xs, "/F"]);
    let _ = std::fs::remove_file(&xmlp);
    match &r1 {
        Ok(r) if r.ok() => return Ok((Way::Xml, tried)),
        Ok(r) => tried.push(format!("/Create /XML 失败：{}", r.err_line())),
        Err(e) => tried.push(format!("/Create /XML 跑不起来：{}", e.msg)),
    }

    // ② 先删掉可能存在的同名任务再试一次 XML。
    //    `/F` 按说就该覆盖，但覆盖一个**已经存在**的任务是四次失败里
    //    唯一与那一次成功不同的地方，值得花一次调用排除掉
    let _ = run(&["/Delete", "/TN", TASK, "/F"]);
    let xmlp = task_xml_path();
    write_utf16(&xmlp, &task_xml(exe_s, h, m, None))?;
    let xs = xmlp.to_string_lossy().into_owned();
    let r2 = run(&["/Create", "/TN", TASK, "/XML", &xs, "/F"]);
    let _ = std::fs::remove_file(&xmlp);
    match &r2 {
        Ok(r) if r.ok() => return Ok((Way::Xml, tried)),
        Ok(r) => tried.push(format!("先删后建（/XML）还是失败：{}", r.err_line())),
        Err(e) => tried.push(format!("先删后建（/XML）跑不起来：{}", e.msg)),
    }

    // ③ 兜底：完全不用 XML
    let argv = simple_argv(exe_s, h, m);
    let args: Vec<&str> = argv.iter().map(String::as_str).collect();
    match run(&args) {
        Ok(r) if r.ok() => Ok((Way::Simple, tried)),
        Ok(r) => {
            tried.push(format!("/SC DAILY 也失败：{}", r.err_line()));
            Err(AppError::new(
                Code::ConfigWrite,
                format!("schtasks 三条路都没装上定时备份：{}", tried.join("；")),
            ))
        }
        Err(e) => {
            tried.push(format!("/SC DAILY 跑不起来：{}", e.msg));
            Err(AppError::new(
                Code::ConfigWrite,
                format!("schtasks 三条路都没装上定时备份：{}", tried.join("；")),
            ))
        }
    }
}

/// 现状。**每一项都现查系统**。
pub fn status() -> Status {
    let cfg = crate::config::LauncherConfig::load();
    let mut s = Status {
        mech: mech().cn().to_string(),
        supported: mech() != Mech::None,
        wanted: cfg.backup.enabled,
        time: cfg.backup.time.clone(),
        ..Default::default()
    };
    match mech() {
        Mech::LaunchAgent => {
            let p = plist_path();
            s.path = crate::redact::mask_home(&p.to_string_lossy());
            s.installed = p.exists();
            match run(&["print", &format!("gui/{}/{LABEL}", uid())]) {
                Ok(r) if r.ok() => {
                    s.enabled = true;
                    s.lines.push("launchctl print：这个任务已经加载".into());
                    if let Some(l) = r.stdout.lines().find(|l| l.contains("runs =")) {
                        s.lines.push(l.trim().to_string());
                    }
                }
                Ok(r) => {
                    s.lines
                        .push(format!("launchctl print 没查到：{}", r.err_line()));
                }
                Err(e) => s.reason = format!("跑不了 launchctl：{}", e.msg),
            }
            s.next_run = String::new();
            s.lines
                .push("launchd 不报「下次几点」；睡眠错过的那一次会在唤醒后补跑".into());
        }
        Mech::SystemdUser => {
            s.path = crate::redact::mask_home(&timer_path().to_string_lossy());
            s.installed = timer_path().exists() && service_path().exists();
            let unit = format!("{UNIT}.timer");
            match run(&["--user", "is-enabled", &unit]) {
                Ok(r) => {
                    let v = r.stdout.trim().to_string();
                    s.enabled = v == "enabled";
                    s.lines.push(format!("systemctl --user is-enabled：{v}"));
                }
                Err(e) => s.reason = format!("跑不了 systemctl：{}", e.msg),
            }
            if let Ok(r) = run(&["--user", "list-timers", &unit, "--no-pager", "--all"]) {
                for l in r.stdout.lines().skip(1).take(1) {
                    let t = l.trim();
                    if !t.is_empty() && !t.starts_with("NEXT") {
                        s.next_run = t.to_string();
                    }
                }
            }
            // linger：关着的话，退出登录之后这个定时器就不跑了。**如实说，不替他改**
            match linger() {
                Some(true) => s
                    .lines
                    .push("这个账号开着 linger，退出登录之后定时器照样跑".into()),
                Some(false) => s.lines.push(
                    "这个账号没开 linger（systemd 的默认）：退出登录之后定时器不跑，\
                     下次登录会补跑错过的那一次"
                        .into(),
                ),
                None => {}
            }
        }
        Mech::SchTasks => {
            s.path = TASK.to_string();
            match run(&["/Query", "/TN", TASK, "/FO", "LIST"]) {
                Ok(r) if r.ok() => {
                    s.installed = true;
                    for l in r.stdout.lines() {
                        let t = l.trim();
                        if t.starts_with("Next Run Time:") || t.starts_with("下次运行时间:") {
                            s.next_run = t
                                .split_once(':')
                                .map(|x| x.1.trim().to_string())
                                .unwrap_or_default();
                        }
                        if t.starts_with("Status:") || t.starts_with("状态:") {
                            s.enabled = !t.contains("Disabled") && !t.contains("已禁用");
                        }
                    }
                }
                Ok(_) => s.lines.push("schtasks /Query：没有这个任务".into()),
                Err(e) => s.reason = format!("跑不了 schtasks：{}", e.msg),
            }
        }
        Mech::None => {
            s.reason = "这台机器上没有可用的定时机制（Linux 上需要 systemd --user）".into();
        }
    }
    s
}

/// 上一次定时备份是不是**该跑而没跑**（睡眠 / 关机错过，而且补跑也没成）。
///
/// 判据只看时间，不猜原因：开着自动备份、而且**最近一次成功**距今超过
/// 25 小时（一天 + 一小时的余量）。返回一句给用户看的话。
pub fn missed() -> Option<String> {
    let cfg = crate::config::LauncherConfig::load();
    if !cfg.backup.enabled {
        return None;
    }
    let last = cfg.backup.last_ok_at.trim();
    if last.is_empty() {
        return None;
    }
    let hours = crate::timefmt::hours_since_shanghai(last)?;
    (hours > 25.0).then(|| {
        format!(
            "上一次成功的自动备份是 {last}（{:.0} 小时前）。电脑当时可能关着或者睡着了。",
            hours
        )
    })
}

// ── 子进程 ────────────────────────────────────────────────────────────────

/// 跑定时机制那个命令。**参数先过 [`crate::assist::guard::argv_schedule`]**
/// —— 那张表逐字写死，一条 `sudo` 都没有。
fn run(args: &[&str]) -> AppResult<crate::proc::Ran> {
    let prog = program();
    let mut argv = vec![prog.clone()];
    argv.extend(args.iter().map(|s| (*s).to_string()));
    crate::assist::guard::argv_schedule(&argv)?;
    crate::proc::run_timeout(&prog, args, T)
}

fn program() -> String {
    match mech() {
        Mech::LaunchAgent => "/bin/launchctl".to_string(),
        Mech::SystemdUser => crate::runtime::which::find_program("systemctl")
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| "systemctl".to_string()),
        Mech::SchTasks => "schtasks.exe".to_string(),
        Mech::None => "true".to_string(),
    }
}

/// 当前用户的 uid（launchctl 的 `gui/<uid>` 域要它）。问不到就退回 501
/// （macOS 上第一个用户的 uid）—— 这一支只在 macOS 上走得到。
fn uid() -> u32 {
    crate::proc::run_timeout("/usr/bin/id", &["-u"], Duration::from_secs(10))
        .ok()
        .filter(crate::proc::Ran::ok)
        .and_then(|r| r.stdout.trim().parse::<u32>().ok())
        .unwrap_or(501)
}

/// 这个账号开没开 linger。查不了就返回 `None`（不猜）。
fn linger() -> Option<bool> {
    let user = std::env::var("USER").ok()?;
    let r = crate::proc::run_timeout(
        "loginctl",
        &["show-user", &user, "-p", "Linger", "--value"],
        Duration::from_secs(10),
    )
    .ok()?;
    if !r.ok() {
        return None;
    }
    match r.stdout.trim() {
        "yes" => Some(true),
        "no" => Some(false),
        _ => None,
    }
}

/// schtasks 的 XML 必须是 UTF-16LE 带 BOM，否则它会报「格式不正确」。
fn write_utf16(p: &std::path::Path, s: &str) -> AppResult<()> {
    let mut bytes: Vec<u8> = vec![0xFF, 0xFE];
    for u in s.encode_utf16() {
        bytes.extend_from_slice(&u.to_le_bytes());
    }
    std::fs::write(p, bytes)
        .map_err(|e| AppError::new(Code::ConfigWrite, format!("写 {} 失败：{e}", p.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plist_里跑的就是那条无界面命令且时间对得上() {
        let s = plist_text(
            "/Applications/Hunter.app/Contents/MacOS/hunter-launcher",
            0,
            0,
            None,
            "/tmp/x.log",
        );
        assert!(s.contains("<string>--backup</string>"), "{s}");
        assert!(s.contains("<string>--scheduled</string>"), "{s}");
        assert!(s.contains(&format!("<string>{LABEL}</string>")), "{s}");
        assert!(
            s.contains("<key>Hour</key>\n    <integer>0</integer>"),
            "{s}"
        );
        assert!(
            s.contains("<key>Minute</key>\n    <integer>0</integer>"),
            "{s}"
        );
        // RunAtLoad 必须是 false：装上它的那一刻不该立刻跑一次备份
        assert!(s.contains("<key>RunAtLoad</key>\n  <false/>"), "{s}");
        // 一条 sudo 都不该有
        assert!(!s.contains("sudo"), "{s}");
    }

    #[test]
    fn plist_的时间改了之后内容真的变了() {
        let a = plist_text("/x", 0, 0, None, "/tmp/l");
        let b = plist_text("/x", 23, 30, None, "/tmp/l");
        assert_ne!(a, b);
        assert!(b.contains("<integer>23</integer>"));
        assert!(b.contains("<integer>30</integer>"));
    }

    #[test]
    fn plist_里的路径要转义_xml() {
        let s = plist_text("/Users/a&b/Hunter <1>", 1, 2, Some("/home/x&y"), "/tmp/l");
        assert!(s.contains("/Users/a&amp;b/Hunter &lt;1&gt;"), "{s}");
        assert!(s.contains("<string>/home/x&amp;y</string>"), "{s}");
    }

    #[test]
    fn systemd_定时器必须是_persistent_才能补跑() {
        let s = timer_text(0, 0);
        assert!(s.contains("OnCalendar=*-*-* 00:00:00"), "{s}");
        assert!(
            s.contains("Persistent=true"),
            "没有它，电脑睡过 00:00 那一次就永远不补跑了：{s}"
        );
        assert!(s.contains(&format!("Unit={UNIT}.service")), "{s}");
        assert!(s.contains("WantedBy=timers.target"), "{s}");
    }

    #[test]
    fn systemd_的时间是两位数补零的() {
        assert!(timer_text(7, 5).contains("OnCalendar=*-*-* 07:05:00"));
        assert!(timer_text(23, 30).contains("OnCalendar=*-*-* 23:30:00"));
    }

    #[test]
    fn systemd_服务跑的是那条无界面命令且不提权() {
        let s = service_text("/usr/bin/hunter-launcher", Some("/home/u/.hunter"));
        assert!(
            s.contains(r#"ExecStart="/usr/bin/hunter-launcher" --backup --scheduled"#),
            "{s}"
        );
        assert!(
            s.contains(r#"Environment="HUNTER_HOME=/home/u/.hunter""#),
            "{s}"
        );
        assert!(!s.contains("User=root"), "{s}");
        assert!(!s.contains("sudo"), "{s}");
        assert!(s.contains("Type=oneshot"), "{s}");
    }

    #[test]
    fn systemd_路径带空格也解析得了() {
        // AppImage 常常躺在「~/下载/Hunter 启动器.AppImage」这种路径下。
        // 不加引号的话 systemd 会把空格当参数分隔符，ExecStart 直接解析失败
        let s = service_text("/home/u/下载/Hunter 启动器.AppImage", None);
        assert!(
            s.contains(r#"ExecStart="/home/u/下载/Hunter 启动器.AppImage" --backup --scheduled"#),
            "{s}"
        );
    }

    #[test]
    fn windows_任务必须带_错过后尽快运行() {
        let s = task_xml("C:\\P\\hunter-launcher.exe", 0, 0, None);
        assert!(
            s.contains("<StartWhenAvailable>true</StartWhenAvailable>"),
            "方案 R6 6.3 要求「错过后尽快运行」：{s}"
        );
        assert!(
            s.contains("<StartBoundary>2026-01-01T00:00:00</StartBoundary>"),
            "{s}"
        );
        assert!(s.contains("--backup --scheduled"), "{s}");
        assert!(s.contains("<DaysInterval>1</DaysInterval>"), "{s}");
    }

    #[test]
    fn windows_的_xml_是_utf16_带_bom() {
        let _h = crate::paths::test_home("sched-utf16");
        let p = crate::paths::root().join("t.xml");
        write_utf16(&p, "<a/>").unwrap();
        let b = std::fs::read(&p).unwrap();
        assert_eq!(&b[..2], &[0xFF, 0xFE], "schtasks 只认带 BOM 的 UTF-16LE");
        assert_eq!(&b[2..4], &[b'<', 0]);
    }

    #[test]
    fn 三个平台的路径都在用户自己的地盘里() {
        // 一条都不许落到 /Library/LaunchDaemons、/etc/systemd/system 这类要 root 的地方
        let home = crate::paths::home();
        assert!(plist_path().starts_with(&home), "{:?}", plist_path());
        assert!(timer_path().starts_with(&home), "{:?}", timer_path());
        assert!(service_path().starts_with(&home), "{:?}", service_path());
        // 按**路径分量**比，不按字符串比：Windows 上分隔符是反斜杠，
        // `contains("Library/LaunchAgents")` 在那儿永远为假（I13 的 CI 上红过一次）。
        // 这条断言在三个平台上都该成立 —— `plist_path()` 是平台无关地拼出来的
        let comps: Vec<String> = plist_path()
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        assert!(comps.contains(&"LaunchAgents".to_string()), "{comps:?}");
        assert!(
            !comps.contains(&"LaunchDaemons".to_string()),
            "用户级是 LaunchAgents，不是 LaunchDaemons：{comps:?}"
        );
    }
    // ── I16 · P0-3：Windows 上定时备份从第一天起就没装上 ──────────────────

    /// XML 里 `<Actions Context="…">` 指的那个 principal **必须真的存在**。
    ///
    /// 0.1.15 的 XML 里 `Context="Author"`，而整份文档里没有一个 `<Principal>`。
    #[test]
    fn xml_里的_principal_要对得上_actions_的_context() {
        let s = task_xml("C:\\P\\hunter-launcher.exe", 0, 0, None);
        assert!(s.contains("<Principals>"), "{s}");
        assert!(s.contains("<Principal id=\"Author\">"), "{s}");
        assert!(s.contains("<Actions Context=\"Author\">"), "{s}");
        // 顺序也要对：RegistrationInfo → Triggers → Principals → Settings → Actions
        let at = |tag: &str| s.find(tag).unwrap_or_else(|| panic!("没有 {tag}：{s}"));
        assert!(at("<RegistrationInfo>") < at("<Triggers>"));
        assert!(at("<Triggers>") < at("<Principals>"));
        assert!(at("<Principals>") < at("<Settings>"));
        assert!(at("<Settings>") < at("<Actions"));
    }

    /// `<Settings>` 的子元素按 schema 里的顺序排。
    #[test]
    fn settings_的子元素按_schema_顺序() {
        let whole = task_xml("C:\\P\\hunter-launcher.exe", 7, 30, None);
        // **只在 `<Settings>` 这一段里找。** `<Enabled>` 在 `<CalendarTrigger>` 里
        // 也有一个，按整份文档找会先撞上那一个
        let a = whole.find("<Settings>").expect("没有 <Settings>");
        let b = whole.find("</Settings>").expect("没有 </Settings>");
        let s = &whole[a..b];
        let mut last = 0usize;
        for tag in SETTINGS_ORDER {
            let i = s
                .find(&format!("<{tag}>"))
                .unwrap_or_else(|| panic!("<{tag}> 不在 <Settings> 里：{whole}"));
            assert!(
                i > last,
                "<{tag}> 的位置不对（应该排在前一项之后）：{whole}"
            );
            last = i;
        }
    }

    /// **两处同时装的时候不许共用一个临时文件名。**
    ///
    /// 原先是写死的 `~/.hunter/hunter-backup-task.xml`：安装收尾与开机自查
    /// 撞在一起时，一边的 `remove_file` 会把另一边正在读的那份删掉，
    /// schtasks 于是报一句什么都没说的 `错误: 未指定的错误`。
    #[test]
    fn 每次装用各自的_xml_文件名() {
        let _h = crate::paths::test_home("sched-xmlpath");
        let a = task_xml_path();
        let b = task_xml_path();
        assert_ne!(a, b, "两次调用拿到了同一个文件名：{a:?}");
        for p in [&a, &b] {
            assert!(
                p.starts_with(crate::paths::root()),
                "临时文件只许落在 ~/.hunter 里：{p:?}"
            );
        }
    }

    /// 不用 XML 的兜底路：argv 的形状与守卫都要对得上。
    #[test]
    fn 兜底那条路跑的还是同一条命令而且过得了守卫() {
        let argv = simple_argv("C:\\P\\hunter-launcher.exe", 0, 0);
        assert_eq!(argv[0], "/Create");
        assert_eq!(argv[2], TASK);
        assert_eq!(
            argv[4], "\"C:\\P\\hunter-launcher.exe\" --backup --scheduled",
            "跑的必须还是那条无界面命令"
        );
        assert_eq!(argv[8], "00:00");
        let mut full = vec!["schtasks.exe".to_string()];
        full.extend(argv);
        crate::assist::guard::argv_schedule(&full).expect("兜底那条路应当在白名单里");
    }

    /// 守卫**只放行那一条命令** —— 换个程序、换个时间格式、塞个 `&` 一律拒。
    #[test]
    fn 兜底那条路不许被塞进别的命令() {
        let bad = |tr: &str, st: &str| {
            let argv: Vec<String> = vec![
                "schtasks.exe",
                "/Create",
                "/TN",
                TASK,
                "/TR",
                tr,
                "/SC",
                "DAILY",
                "/ST",
                st,
                "/F",
            ]
            .into_iter()
            .map(String::from)
            .collect();
            crate::assist::guard::argv_schedule(&argv).is_err()
        };
        assert!(bad(
            "\"C:\\x.exe\" --backup --scheduled & calc.exe",
            "00:00"
        ));
        assert!(bad("calc.exe", "00:00"));
        assert!(bad("\"C:\\x.exe\" --backup", "00:00"));
        assert!(bad("\"C:\\x.exe\" --backup --scheduled", "0:00"));
        assert!(bad("\"C:\\x.exe\" --backup --scheduled", "aa:bb"));
    }

    /// 两条路装出来的任务跑的是**同一条**命令 —— 不许一条路装出个跑法不同的任务。
    #[test]
    fn 两条路跑的是同一条命令() {
        let exe = "C:\\P\\hunter-launcher.exe";
        assert!(task_xml(exe, 0, 0, None).contains(&format!("<Arguments>{TASK_ARGS}</Arguments>")));
        assert!(simple_argv(exe, 0, 0)[4].ends_with(TASK_ARGS));
    }

    /// 兜底那条路**少了「错过后补跑」，这件事必须说出来**（红线 1）。
    #[test]
    fn 兜底那条路要如实说它不补跑() {
        assert!(Way::Xml.tail().contains("补跑"));
        assert!(Way::Simple.tail().contains("不补跑"));
        assert_ne!(Way::Xml.tail(), Way::Simple.tail());
    }
}
