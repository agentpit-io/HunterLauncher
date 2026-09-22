//! 守卫（I5 · 设计文档 §3.3、§四）。**不靠提示词，靠代码。**
//!
//! 一次授权页上写给用户的那五条「AI 绝不会做的事」，在这里各有一条对应的硬校验。
//! 提示词里也写了同样的话，但提示词是**建议**，这个文件是**执行路径上唯一的门**：
//! 模型想干、界面有 bug、或者以后有人往动作表里加了危险的东西，都要先过这里。
//!
//! | 承诺 | 这里的实现 |
//! |---|---|
//! | 不删你的任何文件 | [`writable_path`]：`canonicalize` 之后必须落在 `~/.hunter/` 内；[`deletable_path`] 再限定到「启动器自己生成的那几个文件」的白名单 |
//! | 不删不停你别的容器 | [`own_container`]：执行前查 `com.docker.compose.project` 标签，不是 `hunter` 一律拒绝；[`argv`] 强制 docker 操作带 `--project-name hunter` |
//! | 不改你的网络设置 | [`argv`]：`networksetup` / `scutil` / `pfctl` / `iptables` / `/etc/hosts` 等一律拒绝 |
//! | 不用管理员密码 | [`argv`]：`sudo` / `pkexec` / `runas` / `osascript … administrator privileges` 一律拒绝 |
//! | 不执行它自己编的命令 | 动作表机制（[`super::actions`]）+ [`argv`] 里的 `sh -c` 类拦截 |
//!
//! ## 为什么路径要 `canonicalize`
//!
//! `~/.hunter/../../Documents/x` 这种写法，字符串前缀比对会放行。`canonicalize`
//! 会解开 `..` 与软链，拿到真实位置再比 —— 这是唯一可靠的判断方式。
//! 文件不存在时 `canonicalize` 会失败，所以对「要创建的文件」改为 canonicalize
//! 它的父目录，再拼上文件名。
//!
//! ## 审计
//!
//! 每一次校验（通过与拒绝）都写 `~/.hunter/logs/assist-audit.log`，
//! 一行一条 JSON：时间、动作、参数、由谁提出、结果。**这个文件不进诊断包也不上报**
//! （它里面有路径），只给用户自己看。

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::config::PROJECT;
use crate::err::{AppError, AppResult, Code};

/// 动作的风险级别（设计文档 §五）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Level {
    /// 只看不改
    ReadOnly,
    /// 只改 `~/.hunter`，或只影响本项目 `hunter` 的容器
    Safe,
    /// 会影响这台机器上别的东西。**`auto` 档下也要先问用户**
    Sensitive,
}

impl Level {
    pub fn cn(self) -> &'static str {
        match self {
            Level::ReadOnly => "只读",
            Level::Safe => "安全（只动 Hunter 自己的东西）",
            Level::Sensitive => "需要你同意",
        }
    }
}

/// 授权档位（设计文档 §3.2）。存 `launcher.toml [assist] mode`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    /// 全自动：动作表内的动作（含 `Sensitive`）由「守卫 + 复核员」两道门把关之后
    /// **直接执行**，安装过程里一次都不问用户（I8 · 任务书〇·五）
    Auto,
    /// 逐步确认：和 0.1.4 一样，改动类动作逐条弹确认
    Confirm,
    /// 关闭：只跑确定性规则，不调模型
    Off,
}

impl Mode {
    pub fn parse(s: &str) -> Mode {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => Mode::Auto,
            "off" => Mode::Off,
            // 认不得的值一律落到最保守的那一档（不是最方便的那一档）
            _ => Mode::Confirm,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Auto => "auto",
            Mode::Confirm => "confirm",
            Mode::Off => "off",
        }
    }
    pub fn cn(self) -> &'static str {
        match self {
            Mode::Auto => "全自动",
            Mode::Confirm => "逐步确认",
            Mode::Off => "关闭",
        }
    }
    /// 这个级别的动作，在这一档下要不要先问用户。
    ///
    /// I8 之前 `Sensitive` 在任何档位下都要问。用户 2026-09-21 22:35 把那一条否了：
    ///
    /// > 需要支持全自动的安装、问题修改，不要让用户参与决策和点击确认和执行，
    /// > 出现问题，自主分析，按最佳方案执行。
    ///
    /// 所以**全自动档下 `Sensitive` 不再问**——但把关的门一道没少，只是换了把关的人：
    ///
    /// | 门 | 谁 | 全自动档下 |
    /// |---|---|---|
    /// | ① 动作表 | 代码 | 表外的一律不执行（不变） |
    /// | ② 守卫 | 代码 | 路径 / 删除 / 网络 / 他人容器的禁止项一条不放松（不变） |
    /// | ③ 复核员 | 另一个模型 | 含 `Sensitive` 的计划必过，否决就退回诊断员换方案（不变） |
    /// | ④ 用户点一下 | 用户 | **取消**（`Confirm` 档仍保留，在设置页给高级用户） |
    ///
    /// 换句话说：**放松的是「问不问用户」，不是「拦不拦得住」**。
    /// 守卫是代码写死的硬校验，它不看档位 —— 见 [`writable_path`] / [`deletable_files`]。
    pub fn needs_confirm(self, level: Level) -> bool {
        match (self, level) {
            (_, Level::ReadOnly) => false,
            // 全自动：守卫 + 复核员两道门过了就执行，不再等用户
            (Mode::Auto, _) => false,
            (Mode::Confirm, _) => true,
            (Mode::Off, _) => true,
        }
    }
}

// ── 路径守卫 ──────────────────────────────────────────────────────────────

/// 把一个路径解析成真实位置，并确认它落在 `~/.hunter/` 内。
///
/// 返回解析后的绝对路径。**只有它能进后续的写/删**（不要再用调用方给的原串）。
pub fn writable_path(p: &Path) -> AppResult<PathBuf> {
    let root = canon_root()?;
    let real = canon_for_write(p)?;
    if !real.starts_with(&root) {
        return Err(reject(format!(
            "路径 {} 解析之后落在 {} 外面，拒绝（AI 只能在 Hunter 自己的文件夹里动东西）。",
            crate::redact::mask_home(&real.to_string_lossy()),
            crate::redact::mask_home(&root.to_string_lossy())
        )));
    }
    Ok(real)
}

/// 启动器**自己生成**的文件。只有这张表里的才允许删。
///
/// 为什么是白名单而不是「`~/.hunter` 下随便删」：用户完全可能把自己的东西放进去
/// （备份、导出的诊断包、手抄的 .env 副本），而数据卷更是一删就没。
/// 会员名单短一点、清楚一点，比规则复杂一点好。
pub fn deletable_files() -> Vec<PathBuf> {
    vec![
        crate::paths::override_file(),
        crate::paths::compose_file(),
        crate::paths::version_file(),
        crate::paths::app_dir().join("docker-compose.launcher.yml.bak"),
    ]
}

/// 允许**整棵删掉**的目录。目前只有一个：内置运行时（I7）。
///
/// 它和 [`deletable_files`] 是同一条原则的两种形态 —— 只有**启动器自己从零
/// 生成**的东西才允许删。`~/.hunter/runtime` 下的每一个字节都是
/// [`crate::runtime::builtin::install`] 写进去的：下载回来的四个二进制、
/// 解压出来的 lima、colima 建的虚拟机磁盘。用户不会往这里放东西
/// （界面上也从不引导他往这里放），所以整棵删是安全的。
///
/// `~/.hunter` 下别的目录一个都不在这张表里：`app/` 有 `.env`（他的 key）、
/// `backups/` 是他的备份、`diagnostics/` 是他导出的包。
pub fn deletable_trees() -> Vec<PathBuf> {
    vec![crate::paths::runtime_dir()]
}

/// 允许整棵删吗。先过 [`writable_path`]，再比对目录白名单。
pub fn deletable_tree(p: &Path) -> AppResult<PathBuf> {
    let real = writable_path(p)?;
    let ok = deletable_trees()
        .iter()
        .any(|w| matches!(canon_for_write(w), Ok(c) if c == real));
    if !ok {
        return Err(reject(format!(
            "{} 不在「可以整棵删掉」的清单里（那里面可能有你自己的东西），拒绝。",
            crate::redact::mask_home(&real.to_string_lossy())
        )));
    }
    Ok(real)
}

/// 允许删吗。先过 [`writable_path`]，再比对白名单。
pub fn deletable_path(p: &Path) -> AppResult<PathBuf> {
    let real = writable_path(p)?;
    let ok = deletable_files().iter().any(|w| {
        // 白名单里的文件可能还不存在，canonicalize 不了；按父目录 + 文件名比
        match canon_for_write(w) {
            Ok(c) => c == real,
            Err(_) => false,
        }
    });
    if !ok {
        return Err(reject(format!(
            "{} 不在「启动器自己生成的文件」清单里，拒绝删除。",
            crate::redact::mask_home(&real.to_string_lossy())
        )));
    }
    Ok(real)
}

fn canon_root() -> AppResult<PathBuf> {
    let root = crate::paths::root();
    std::fs::create_dir_all(&root).ok();
    root.canonicalize().map_err(|e| {
        AppError::new(
            Code::ConfigWrite,
            format!("解析不了工作目录 {}：{e}", root.display()),
        )
    })
}

/// 文件可能还不存在（要创建），所以 canonicalize 它的父目录再拼文件名。
fn canon_for_write(p: &Path) -> AppResult<PathBuf> {
    if let Ok(c) = p.canonicalize() {
        return Ok(c);
    }
    let parent = p.parent().ok_or_else(|| {
        reject(format!(
            "路径 {} 没有父目录，判断不了它在哪，拒绝。",
            crate::redact::mask_home(&p.to_string_lossy())
        ))
    })?;
    let name = p.file_name().ok_or_else(|| {
        reject(format!(
            "路径 {} 没有文件名，拒绝。",
            crate::redact::mask_home(&p.to_string_lossy())
        ))
    })?;
    let cp = parent.canonicalize().map_err(|e| {
        reject(format!(
            "解析不了 {}：{e}，拒绝。",
            crate::redact::mask_home(&parent.to_string_lossy())
        ))
    })?;
    Ok(cp.join(name))
}

// ── 容器守卫 ──────────────────────────────────────────────────────────────

/// 这个容器是**本项目**的吗。查 `com.docker.compose.project` 标签，
/// 不是 `hunter` 一律拒绝；查不到也拒绝（**不确定就不动**）。
pub fn own_container(id_or_name: &str) -> AppResult<()> {
    if id_or_name.trim().is_empty() {
        return Err(reject("没给容器名，拒绝。".to_string()));
    }
    let bin = crate::runtime::which::docker_bin();
    let r = crate::proc::run_timeout(
        &bin,
        &[
            "inspect",
            "-f",
            "{{index .Config.Labels \"com.docker.compose.project\"}}",
            id_or_name,
        ],
        std::time::Duration::from_secs(20),
    )?;
    let project = r.stdout.trim();
    if !r.ok() {
        return Err(reject(format!(
            "查不到容器「{}」的 compose 项目标签（{}），拒绝对它动手。",
            safe(id_or_name),
            r.err_line()
        )));
    }
    if project != PROJECT {
        return Err(reject(format!(
            "容器「{}」属于 compose 项目「{}」，不是本次安装的「{PROJECT}」—— \
             AI 不会动你电脑上别的容器，拒绝。",
            safe(id_or_name),
            safe(project)
        )));
    }
    Ok(())
}

/// 本项目的 6 个服务名。`restart_own_service` 只认这些。
pub const OWN_SERVICES: &[&str] = &["api", "llm-shim", "opencode", "postgres", "redis", "web"];

// ── 命令守卫 ──────────────────────────────────────────────────────────────

/// 一律拒绝的程序名（取 basename 比对，`/usr/bin/sudo` 也拦得住）。
const FORBIDDEN_PROGRAMS: &[&str] = &[
    // 提权
    "sudo",
    "su",
    "doas",
    "pkexec",
    "runas",
    "gsudo",
    // 网络与安全设置
    "networksetup",
    "scutil",
    "pfctl",
    "iptables",
    "ip6tables",
    "nft",
    "ufw",
    "firewall-cmd",
    "netsh",
    "route",
    "resolvectl",
    "systemd-resolve",
    "defaults",
    "csrutil",
    "spctl",
    "security",
    "dscl",
    // 任意 shell / 任意代码
    "sh",
    "bash",
    "zsh",
    "fish",
    "dash",
    "cmd",
    "cmd.exe",
    "powershell",
    "pwsh",
    "osascript",
    "python",
    "python3",
    "perl",
    "ruby",
    "node",
    "env",
    "xargs",
    "eval",
    // 删除
    "rm",
    "rmdir",
    "del",
    "erase",
    "shred",
    "unlink",
    "mv",
];

/// docker 子命令里一律拒绝的组合。
const FORBIDDEN_DOCKER: &[&[&str]] = &[
    &["volume", "rm"],
    &["volume", "prune"],
    &["system", "prune"],
    &["image", "prune"],
    &["network", "prune"],
    &["builder", "prune"],
    &["container", "prune"],
    &["swarm"],
];

/// 会碰到网络/安全设置的文件。谁都不许写。
const FORBIDDEN_PATH_HINTS: &[&str] = &[
    "/etc/hosts",
    "/etc/resolv.conf",
    "/private/etc/hosts",
    "\\drivers\\etc\\hosts",
    "/Library/LaunchDaemons",
    "/Library/LaunchAgents",
];

/// 对将要 `spawn` 的参数数组做最后一道硬校验。
///
/// **执行路径上的每一个子进程都要过这里**，不管是谁提出来的。
pub fn argv(argv: &[String]) -> AppResult<()> {
    argv_scoped(argv, false)
}

/// 接管态（I7）专用的那一道。**比默认那一道更严，不是更松。**
///
/// 默认规则是「改动容器状态的 docker 命令必须 `--project-name hunter`」——
/// 接管用户自己那一套时项目名当然不是 `hunter`，所以要开一个口子。
/// 这个口子有三把锁，全是代码：
///
/// 1. `launcher.toml` 的 `[takeover] project` 必须**非空**，且命令里指名的
///    就是它 —— 用户没在「需要你」卡片上点过「直接用它」，这条路根本不存在；
/// 2. 子命令只能是 `stop` / `start` / `restart` —— `down`、`rm`、`kill` 一律不行；
/// 3. 剩下的全部检查照旧走 [`argv_scoped`]（`-v`、`volume rm`、`sudo`、
///    改网络设置……一条都不少）。
pub fn argv_takeover(argv: &[String]) -> AppResult<()> {
    let cfg = crate::config::LauncherConfig::load();
    argv_takeover_for(argv, &cfg.takeover.project)
}

/// 同上，但被接管的项目名由调用方给。
///
/// 拆出来是为了**能测它**：判定只跟「project 是什么」有关，不该为了测一条规则
/// 去改用户真实的 `launcher.toml`（测试跑在开发者自己的 `~/.hunter` 上）。
pub fn argv_takeover_for(argv: &[String], takeover_project: &str) -> AppResult<()> {
    let project = takeover_project.trim().to_ascii_lowercase();
    if project.is_empty() {
        return Err(reject(
            "现在没有在管理别的 Hunter，这条命令没有理由指向别的 compose 项目，拒绝。".to_string(),
        ));
    }
    let rest: Vec<String> = argv
        .iter()
        .skip(1)
        .map(|s| s.to_ascii_lowercase())
        .collect();
    // 指名的必须正是被接管的那一个
    let named = rest
        .windows(2)
        .find(|w| w[0] == "-p" || w[0] == "--project-name")
        .map(|w| w[1].clone());
    match named.as_deref() {
        Some(n) if n == project => {}
        Some(n) => {
            return Err(reject(format!(
                "这条命令指向 compose 项目「{}」，而你让启动器管理的是「{}」，拒绝。",
                safe(n),
                safe(&project)
            )))
        }
        None => {
            return Err(reject(
                "接管态下的命令必须写明 `--project-name`，拒绝。".to_string(),
            ))
        }
    }
    // 只有这三个子命令。`down` 会删容器与网络，`rm` / `kill` 更不用说
    const ALLOWED: &[&str] = &["stop", "start", "restart", "logs", "ps"];
    if !rest.iter().any(|a| ALLOWED.contains(&a.as_str())) {
        return Err(reject(format!(
            "接管别人那一套时只允许 {} 这几个子命令，拒绝。",
            ALLOWED.join(" / ")
        )));
    }
    for bad in ["down", "rm", "kill", "create", "up", "recreate"] {
        if rest.iter().any(|a| a == bad) {
            return Err(reject(format!(
                "`{bad}` 会改动你自己装的那一套的结构，接管态下不允许，拒绝。"
            )));
        }
    }
    // **任何**带卷的写法都拒。通用那一道只在 `down` / `rm` 上拦 `-v`
    //（那两个子命令这里本来就不允许），所以这一条是接管态自己加的一层：
    // 动的是用户自己的数据，不留任何可能碰到卷的写法
    if rest.iter().any(|a| a == "-v" || a == "--volumes") {
        return Err(reject(
            "接管态下的命令里不允许出现 `-v` / `--volumes`，拒绝。".to_string(),
        ));
    }
    argv_scoped(argv, true)
}

/// 同上，但调用方已经用 [`own_container`] 逐个核过目标容器的项目标签。
///
/// 只有这一种情况可以免掉 `--project-name hunter` 的要求：`docker rm <id>`
/// 这类按 ID 操作的命令带不了项目名，它的范围保证来自那次 `docker inspect`。
/// `container_verified` 传 true 的地方全项目只有两处（[`crate::compose::remove_own_stale_containers`]
/// 与上面的 [`argv_takeover`]），别的地方一律走 [`argv`]。
pub fn argv_scoped(argv: &[String], container_verified: bool) -> AppResult<()> {
    let Some(prog) = argv.first() else {
        return Err(reject("空命令，拒绝。".to_string()));
    };
    let base = Path::new(prog)
        .file_name()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_else(|| prog.to_ascii_lowercase());
    let base = base.trim_end_matches(".exe").to_string();

    if FORBIDDEN_PROGRAMS.contains(&base.as_str()) {
        return Err(reject(format!(
            "「{}」不在 AI 能用的程序里（提权、改网络设置、跑任意脚本、删文件都不允许），拒绝。",
            safe(&base)
        )));
    }

    let rest: Vec<String> = argv[1..].iter().map(|s| s.to_ascii_lowercase()).collect();

    // 任何参数里都不许出现这些路径
    for a in &rest {
        for h in FORBIDDEN_PATH_HINTS {
            if a.contains(&h.to_ascii_lowercase()) {
                return Err(reject(format!(
                    "命令参数里出现了 {h}（网络/系统配置），拒绝。"
                )));
            }
        }
    }

    let is_docker = base == "docker" || base == "docker-compose" || base.starts_with("podman");
    if is_docker {
        // 危险子命令
        for f in FORBIDDEN_DOCKER {
            if window_matches(&rest, f) {
                return Err(reject(format!(
                    "`{} {}` 会影响这台机器上别的数据，不在动作表里，拒绝。",
                    base,
                    f.join(" ")
                )));
            }
        }
        // `down -v` / `--volumes` 会删数据卷
        if rest.iter().any(|a| a == "down") && rest.iter().any(|a| a == "-v" || a == "--volumes") {
            return Err(reject(
                "`docker compose down -v` 会删掉数据卷（你的对话记录就在里面），拒绝。".to_string(),
            ));
        }
        // `rm -v`
        if rest.first().map(|s| s == "rm").unwrap_or(false)
            && rest.iter().any(|a| a == "-v" || a == "--volumes")
        {
            return Err(reject("`docker rm -v` 会连着删匿名卷，拒绝。".to_string()));
        }
        // 会改动状态的 compose / 容器操作必须指名道姓是本项目
        if needs_project_scope(&rest) && !has_project_flag(&rest) && !container_verified {
            return Err(reject(format!(
                "这条 docker 命令没有指定 `--project-name {PROJECT}`，\
                 有可能动到你电脑上别的容器，拒绝。"
            )));
        }
    }
    Ok(())
}

/// 这条 docker 命令是不是「会改动容器状态」的那一类。
fn needs_project_scope(rest: &[String]) -> bool {
    const MUTATING: &[&str] = &[
        "up", "down", "stop", "start", "restart", "rm", "kill", "pause", "unpause", "create",
        "recreate",
    ];
    rest.iter().any(|a| MUTATING.contains(&a.as_str()))
}

fn has_project_flag(rest: &[String]) -> bool {
    let want = PROJECT.to_ascii_lowercase();
    rest.windows(2)
        .any(|w| (w[0] == "-p" || w[0] == "--project-name") && w[1] == want)
        || rest
            .iter()
            .any(|a| a == &format!("--project-name={want}") || a == &format!("-p={want}"))
}

fn window_matches(rest: &[String], pat: &[&str]) -> bool {
    if pat.is_empty() {
        return false;
    }
    rest.windows(pat.len())
        .any(|w| w.iter().zip(pat).all(|(a, b)| a == b))
}

// ── 两道专门的窄门（I8） ──────────────────────────────────────────────────
//
// 上面那道 [`argv`] 的禁用名单是按「**模型**不许碰这些程序」写的：`scutil` 能改
// 网络设置、`bash` 能跑任意代码、`sudo` 能提权，所以一律不许。
//
// I8 要做两件事，恰好各自要用到名单里的一个程序：
//
// | 要做的事 | 要用的程序 | 为什么名单拦不住它就不安全 |
// |---|---|---|
// | 读用户自己配的系统代理（沿用它） | `scutil --proxy` | `scutil` 也能 `--set` 改设置 |
// | 替用户装 Homebrew（官方脚本） | `/bin/bash <脚本>` | `bash` 能跑任意东西 |
//
// 解法不是「把它们从名单里删掉」，而是**另开两道更窄的门**：门里只认**完整的、
// 由代码写死的参数形状**，模型一个字节都插不进来。名单本身一条都没动 ——
// [`argv`] 照样拒绝 `scutil` 与 `bash`，模型走的永远是那一道。
//
// 这和 I7 里 `runtime::orbstack::verify` 用 `codesign` / `spctl` 是同一条原则：
// 禁令针对的是「谁能提出这条命令」，不是「这个程序名本身有罪」。

/// 只读探测的窄门。**只认这两条，一个参数都不能多。**
///
/// * `scutil --proxy`（macOS：打印当前代理设置，`--proxy` 只读）
/// * `reg query HKCU\…\Internet Settings`（Windows：`query` 是只读子命令）
///
/// 调用点全项目只有 [`crate::netproxy`] 一处，参数是常量。
pub fn argv_readonly_probe(argv: &[String]) -> AppResult<()> {
    let Some(prog) = argv.first() else {
        return Err(reject("空命令，拒绝。".to_string()));
    };
    let base = Path::new(prog)
        .file_name()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let base = base.trim_end_matches(".exe");
    let rest: Vec<&str> = argv[1..].iter().map(String::as_str).collect();
    match (base, rest.as_slice()) {
        // 只读代理设置。**没有第二个参数**：`scutil --set` / `scutil --dns` 一概不行
        ("scutil", ["--proxy"]) => Ok(()),
        // 只读注册表。键固定死在 Internet Settings 这一个分支下
        ("reg", ["query", key])
            if key.eq_ignore_ascii_case(
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings",
            ) =>
        {
            Ok(())
        }
        _ => Err(reject(format!(
            "「{}」不在只读探测的白名单里（那张表只有两条，参数一个字都不能变），拒绝。",
            safe(&argv.join(" "))
        ))),
    }
}

/// 「替用户装软件」那道窄门（I8 · 用户 2026-09-21 22:10 的原则）。
///
/// 用户的原话是「所有能替用户解决的问题都不要让用户自己去操作」，所以
/// 「没有 brew 就先替他装 brew」这件事必须由启动器执行。Homebrew 的官方安装脚本
/// 是一个 bash 脚本，而 `bash` 在禁用名单里 —— 于是有了这道门。
///
/// 三把锁，全是代码：
///
/// 1. 程序必须是 `/bin/bash`（绝对路径，不是 PATH 上找到的某个 bash）；
/// 2. **有且只有一个参数**，而且它 `canonicalize` 之后必须落在
///    `~/.hunter/runtime/` 里 —— 那个目录下的每一个字节都是启动器自己下的；
/// 3. 脚本文件必须**真的存在**（模型编一个路径出来到这里就断了）。
///
/// 注意这道门**不提权**：脚本以当前用户身份跑，它自己需要 root 的那几步由
/// `sudo -A` + [`crate::runtime::elevate`] 生成的原生密码框完成。
pub fn argv_install_script(argv: &[String]) -> AppResult<()> {
    let [prog, script] = argv else {
        return Err(reject(format!(
            "安装脚本只能是「/bin/bash <一个脚本>」这一种形状，收到 {} 个参数，拒绝。",
            argv.len()
        )));
    };
    if prog != "/bin/bash" {
        return Err(reject(format!(
            "安装脚本只能用 /bin/bash 跑，收到「{}」，拒绝。",
            safe(prog)
        )));
    }
    let p = Path::new(script);
    if !p.is_file() {
        return Err(reject(format!(
            "安装脚本 {} 不存在，拒绝。",
            crate::redact::mask_home(script)
        )));
    }
    let real = writable_path(p)?;
    let runtime = canon_for_write(&crate::paths::runtime_dir())?;
    if !real.starts_with(&runtime) {
        return Err(reject(format!(
            "安装脚本必须在 {} 里（那里面的东西都是启动器自己下的），拒绝。",
            crate::redact::mask_home(&runtime.to_string_lossy())
        )));
    }
    Ok(())
}

/// 「在 Hunter 自己那台虚拟机里修 DNS」那道窄门（I10 的 P0-1）。
///
/// ## 为什么要另开一道
///
/// 上面那道总门 [`argv`] 里有两条硬禁令，这件事正好各踩一条：
/// 参数里不许出现 `/etc/resolv.conf`（[`FORBIDDEN_PATH_HINTS`]），
/// 而这条命令里必然要写它。
///
/// **那两条禁令一个字都没松**：它们防的是「改用户这台电脑的网络设置」，
/// 而这里改的是**我们自己下载、自己创建、只服务 Hunter 的那台虚拟机**里的文件。
/// 模型永远走 [`argv`] 那一道 —— 它提 `/etc/resolv.conf` 照样被拒。
/// 这一道只有 [`crate::runtime::vmdns`] 会调，而且：
///
/// 1. 程序必须是 `~/.hunter/runtime` 里那份 `limactl`（不是 PATH 上找到的某个）；
/// 2. 目标实例必须是 `colima-hunter`（跟着 [`crate::runtime::builtin::PROFILE`] 走）；
/// 3. **整条参数数组必须与下面那张表逐字相等** —— 不是「以…开头」，
///    不是「包含…」，是全等。模型一个字节都插不进来。
///
/// `LIMA_HOME` 必须落在 `~/.hunter/runtime` 里这一条，由
/// [`crate::proc`] 的 `isolation_guard` → [`colima_call`] 另外保证。
pub fn argv_vm_dns(argv: &[String]) -> AppResult<()> {
    let Some(prog) = argv.first() else {
        return Err(reject("空命令，拒绝。".to_string()));
    };
    let base = Path::new(prog)
        .file_name()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if base.trim_end_matches(".exe") != "limactl" {
        return Err(reject_audited(
            "argv_vm_dns",
            format!(
                "改虚拟机 DNS 只能用 limactl，收到「{}」，拒绝。",
                safe(prog)
            ),
        ));
    }
    if !under_runtime(prog) {
        return Err(reject_audited(
            "argv_vm_dns",
            format!(
                "这条命令用的 limactl 不在 {} 里 —— 那不是 Hunter 自己那一份，拒绝。",
                crate::redact::mask_home(&crate::paths::runtime_dir().to_string_lossy())
            ),
        ));
    }
    let inst = crate::runtime::vmdns::instance();
    let rest: Vec<&str> = argv[1..].iter().map(String::as_str).collect();

    // ── 形状 A：把一个我们自己生成的文件送进虚拟机 ──
    if let ["copy", "--backend=scp", src, dst] = rest.as_slice() {
        let real = writable_path(Path::new(src))?;
        let stage = canon_for_write(&crate::paths::runtime_dir().join("vm"))?;
        if !real.starts_with(&stage) {
            return Err(reject_audited(
                "argv_vm_dns",
                format!(
                    "只能把 {} 里的文件送进虚拟机（这个是 {}），拒绝。",
                    crate::redact::mask_home(&stage.to_string_lossy()),
                    crate::redact::mask_home(src)
                ),
            ));
        }
        let allowed = [
            format!("{inst}:{VM_TMP_RESOLV}"),
            format!("{inst}:{VM_TMP_UNIT}"),
        ];
        if !allowed.iter().any(|a| a == dst) {
            return Err(reject_audited(
                "argv_vm_dns",
                format!(
                    "送进虚拟机的落点只能是 {}（这条写的是 {}），拒绝。",
                    allowed.join(" / "),
                    safe(dst)
                ),
            ));
        }
        return Ok(());
    }

    // ── 形状 B：在虚拟机里跑一条表里有的命令 ──
    let ["shell", "--workdir", "/", target, cmd @ ..] = rest.as_slice() else {
        return Err(reject_audited(
            "argv_vm_dns",
            format!(
                "改虚拟机 DNS 只认「limactl copy …」与「limactl shell --workdir / {inst} …」两种形状，\
                 收到「{}」，拒绝。",
                safe(&argv.join(" "))
            ),
        ));
    };
    if *target != inst {
        return Err(reject_audited(
            "argv_vm_dns",
            format!(
                "这条命令指向虚拟机「{}」，而 Hunter 自己那台叫「{inst}」，拒绝。",
                safe(target)
            ),
        ));
    }
    if VM_DNS_COMMANDS.contains(&cmd) {
        return Ok(());
    }
    Err(reject_audited(
        "argv_vm_dns",
        format!(
            "「{}」不在「虚拟机里能跑的那张表」里（那张表有 {} 条，每一条都逐字写死），拒绝。",
            safe(&cmd.join(" ")),
            VM_DNS_COMMANDS.len()
        ),
    ))
}

/// **在虚拟机里采资源指标**（I12 · R2）。
///
/// 和 [`argv_vm_dns`] 分开，是因为两张表的性质完全不同：那一张里有 `sudo`
/// （要改 `/etc/resolv.conf`），这一张**一条 `sudo` 都没有、一个字节都不写**。
/// 采指标这件事每 30 秒就跑一次，不该共用一张带写权限的白名单。
///
/// 形状只认一种：`limactl shell --workdir / colima-hunter <表里的命令>`。
/// `limactl` 还必须是 `~/.hunter/runtime` 里那一份（I9 的隔离守卫），
/// 而 `LIMA_HOME` / `COLIMA_HOME` 由 `proc::isolation_guard` 再核一遍。
pub fn argv_vm_metrics(argv: &[String]) -> AppResult<()> {
    let Some(prog) = argv.first() else {
        return Err(reject("空命令，拒绝。".to_string()));
    };
    let base = Path::new(prog)
        .file_name()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if base.trim_end_matches(".exe") != "limactl" {
        return Err(reject_audited(
            "argv_vm_metrics",
            format!("采虚拟机指标只能用 limactl，收到「{}」，拒绝。", safe(prog)),
        ));
    }
    if !under_runtime(prog) {
        return Err(reject_audited(
            "argv_vm_metrics",
            format!(
                "这条命令用的 limactl 不在 {} 里 —— 那不是 Hunter 自己那一份，拒绝。",
                crate::redact::mask_home(&crate::paths::runtime_dir().to_string_lossy())
            ),
        ));
    }
    let inst = crate::runtime::vmdns::instance();
    let rest: Vec<&str> = argv[1..].iter().map(String::as_str).collect();
    let ["shell", "--workdir", "/", target, cmd @ ..] = rest.as_slice() else {
        return Err(reject_audited(
            "argv_vm_metrics",
            format!(
                "采指标只认「limactl shell --workdir / {inst} …」这一种形状，收到「{}」，拒绝。",
                safe(&argv.join(" "))
            ),
        ));
    };
    if *target != inst {
        return Err(reject_audited(
            "argv_vm_metrics",
            format!(
                "这条命令指向虚拟机「{}」，而 Hunter 自己那台叫「{inst}」，拒绝。",
                safe(target)
            ),
        ));
    }
    if VM_METRIC_COMMANDS.contains(&cmd) {
        return Ok(());
    }
    Err(reject_audited(
        "argv_vm_metrics",
        format!(
            "「{}」不在「采指标能跑的那张表」里（那张表有 {} 条，每一条都逐字写死、全是只读），拒绝。",
            safe(&cmd.join(" ")),
            VM_METRIC_COMMANDS.len()
        ),
    ))
}

/// 采资源指标时**允许在虚拟机里跑的全部命令**（I12 · R2）。
///
/// 全部只读、全部不带 `sudo`。加新条目之前先问一句：它会不会写任何东西。
pub const VM_METRIC_COMMANDS: &[&[&str]] = &[
    &["free", "-b"],
    &["df", "-B1", "/var/lib/docker"],
];

const VM_TMP_RESOLV: &str = "/tmp/hunter-resolv.conf";
const VM_TMP_UNIT: &str = "/tmp/hunter-dns.service";

/// 在 Hunter 自己那台虚拟机里**允许跑的全部命令**（I10）。
///
/// 表外一律拒绝。写死到逐字相等，是因为这张表里有 `sudo` ——
/// 虚拟机里的 root 也是 root，参数留一丝活口就等于留一个任意命令执行。
///
/// 注意这里的 `sudo` 是**虚拟机里**的 sudo（lima 给那台虚拟机的用户配了 NOPASSWD），
/// 和用户 Mac 上的管理员密码毫无关系 —— 这条路不会弹任何密码框。
pub const VM_DNS_COMMANDS: &[&[&str]] = &[
    // 只读
    &["cat", "/etc/resolv.conf"],
    &["getent", "hosts", crate::gateway::GATEWAY_HOST],
    &["test", "-f", "/etc/systemd/system/hunter-dns.service"],
    // 落位
    &["sudo", "mkdir", "-p", "/etc/hunter"],
    &[
        "sudo",
        "cp",
        "--remove-destination",
        VM_TMP_RESOLV,
        "/etc/hunter/resolv.conf",
    ],
    &[
        "sudo",
        "cp",
        "--remove-destination",
        VM_TMP_RESOLV,
        "/etc/resolv.conf",
    ],
    &[
        "sudo",
        "cp",
        "--remove-destination",
        VM_TMP_UNIT,
        "/etc/systemd/system/hunter-dns.service",
    ],
    // 让那层「开机再写一遍」的保险生效
    &["sudo", "systemctl", "daemon-reload"],
    &["sudo", "systemctl", "enable", "hunter-dns.service"],
    // 让已经起着的容器重新拿到 DNS（动的是虚拟机里的 dockerd）
    &["sudo", "systemctl", "restart", "docker"],
];

/// 「要管理员权限的那一步」能做哪几件事（I8）。
///
/// 这张表就是提权的全部范围。**表外一律拒绝**，而且表里每一条的参数形状都写死。
/// 提权的入口是系统自带的授权框（macOS 的 `do shell script … with administrator
/// privileges`、Linux 的 `pkexec`），启动器不自绘密码框、不存密码、不写日志。
pub const PRIVILEGED_OPS: &[&str] = &[
    // Linux 自更新：.deb 装在 /usr 下，换包要 root
    "dpkg -i <启动器的 .deb>",
    // Linux：docker 后台服务要 root 才起得来
    "systemctl start docker",
];

/// 校验**要以 root 身份跑的那条命令**（提权前的最后一道）。
///
/// 收到的是「里面那条命令」，不是包装后的 `osascript` / `pkexec` 命令行 ——
/// 包装由 [`crate::runtime::elevate`] 自己拼，模型碰不到。
pub fn argv_privileged(argv: &[String]) -> AppResult<()> {
    let Some(prog) = argv.first() else {
        return Err(reject("空命令，拒绝提权。".to_string()));
    };
    let base = Path::new(prog)
        .file_name()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let rest: Vec<&str> = argv[1..].iter().map(String::as_str).collect();
    match (base.as_str(), rest.as_slice()) {
        ("dpkg", ["-i", deb]) => {
            let p = Path::new(deb);
            if !p.is_file() {
                return Err(reject(format!(
                    "要装的 {} 不存在，拒绝提权。",
                    crate::redact::mask_home(deb)
                )));
            }
            // 只能装**我们自己下到 `~/.hunter/updates/` 的那一个包**
            let real = writable_path(p)?;
            let updates = canon_for_write(&crate::paths::updates_dir())?;
            if !real.starts_with(&updates) || !deb.to_ascii_lowercase().ends_with(".deb") {
                return Err(reject(format!(
                    "只能安装启动器自己下到 {} 里的 .deb，拒绝提权。",
                    crate::redact::mask_home(&updates.to_string_lossy())
                )));
            }
            Ok(())
        }
        ("systemctl", ["start", "docker"]) => Ok(()),
        _ => Err(reject(format!(
            "「{}」不在可以提权的那张表里（表里只有 {}），拒绝。",
            safe(&argv.join(" ")),
            PRIVILEGED_OPS.join(" / ")
        ))),
    }
}

// ── colima / limactl 隔离守卫（I9 的 P0-2） ──────────────────────────────

/// colima 的哪些子命令是**对着某一台虚拟机**动手的。
///
/// `list` / `version` / `template` 不针对某个 profile（在我们自己的 `COLIMA_HOME`
/// 里 `list` 只会列出我们自己那一个），所以它们不要求 `--profile`。
const COLIMA_PROFILE_SUBCOMMANDS: &[&str] = &[
    "start",
    "stop",
    "restart",
    "delete",
    "status",
    "ssh",
    "ssh-config",
    "nerdctl",
    "kubernetes",
    "update",
    "prune",
];

/// 从 argv 里取 `--profile <名字>` / `-p <名字>` / `--profile=<名字>`。
fn colima_profile(rest: &[String]) -> Option<String> {
    if let Some(w) = rest
        .windows(2)
        .find(|w| w[0] == "--profile" || w[0] == "-p")
    {
        return Some(w[1].clone());
    }
    rest.iter()
        .find_map(|a| a.strip_prefix("--profile=").map(str::to_string))
}

/// 这条 colima 命令针对的是某一台具体的虚拟机吗。
fn colima_targets_profile(rest: &[String]) -> bool {
    rest.iter()
        .any(|a| COLIMA_PROFILE_SUBCOMMANDS.contains(&a.as_str()))
}

/// **每一条 colima / limactl 命令都要过这里。**
///
/// ## 这一道拦的是什么
///
/// 0.1.8 在用户 Mac 上（2026-09-22 14:49:15）执行了
/// `~/.hunter/runtime/bin/colima start` —— 我们自己下的 colima，
/// 却用了**默认**的 `COLIMA_HOME=~/.colima`、**没有** `--profile`。
/// 后果是在用户家目录里新建并启动了一台和 Hunter 毫无关系的 `default` 虚拟机
/// （2 核 2 GB，`~/.colima` 占 1.4 GB）。承诺里写着「只在 `~/.hunter` 里动东西」，
/// 那一条当场就破了。
///
/// ## 判据（两条路，都是代码，不是提示词）
///
/// | 情形 | 放行条件 |
/// |---|---|
/// | 用**我们自己**那套 colima | `COLIMA_HOME` 解析后必须落在 `~/.hunter/runtime` 里，且 profile 必须是 `hunter` |
/// | 起**用户原有**的 profile | `--profile <名字>` 里的名字必须是 `~/.colima` 里**本来就有**的目录，且子命令只能是 `start` |
///
/// 两条都不满足 —— 包括「没写 `--profile`」这种最危险的写法 —— 一律拒绝并记审计。
///
/// `env` 是这一次调用额外加的环境变量；没写进去的按
/// [`crate::runtime::env::current`] 给子进程的那一份算（`base_command` 会套上它）。
pub fn colima_call(program: &str, args: &[&str], env: &[(&str, &str)]) -> AppResult<()> {
    let base = Path::new(program)
        .file_name()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_else(|| program.to_ascii_lowercase());
    let base = base.trim_end_matches(".exe").to_string();
    if base != "colima" && base != "limactl" {
        return Ok(());
    }
    let rest: Vec<String> = args.iter().map(|s| s.to_ascii_lowercase()).collect();

    // **这条命令用的是谁的 colima？看程序路径，不看环境变量。**
    //
    // 程序在 `~/.hunter/runtime` 里就是我们自己那份，别处就是用户的。
    // 这一条必须先于环境变量判 —— 否则机器上有内置运行时残骸时，
    // `runtime::env` 会给所有子进程钉上我们的 `COLIMA_HOME`，
    // 于是「起用户原有的 profile」这条**完全正当**的命令会被判成「我们自己那一路」
    // 然后因为 profile 不是 `hunter` 被拒掉。
    let is_ours_binary = under_runtime(program);

    // 这一次调用最终会看到的 COLIMA_HOME：本次显式给的优先，其次是
    // `runtime::env` 给所有子进程的那一份，最后才是启动器自己的进程环境
    let home = env
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("COLIMA_HOME"))
        .map(|(_, v)| (*v).to_string())
        .or_else(|| crate::runtime::env::current().colima_home)
        .or_else(|| std::env::var("COLIMA_HOME").ok())
        .unwrap_or_default();
    let home_is_ours = colima_home_is_ours(&home);
    let profile = colima_profile(&rest);

    // limactl 只走我们自己那条路（colima 会去 $PATH 上找它）。
    // 它没有 `--profile`，靠 LIMA_HOME 隔离
    if base == "limactl" {
        let lima = env
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("LIMA_HOME"))
            .map(|(_, v)| (*v).to_string())
            .or_else(|| crate::runtime::env::current().lima_home)
            .or_else(|| std::env::var("LIMA_HOME").ok())
            .unwrap_or_default();
        if !under_runtime(&lima) {
            return Err(reject_audited(
                "colima_call",
                format!(
                    "limactl 的 LIMA_HOME 是「{}」，不在 {} 里 —— \
                     那会动到你自己的虚拟机，拒绝。",
                    safe(&lima),
                    crate::redact::mask_home(&crate::paths::runtime_dir().to_string_lossy())
                ),
            ));
        }
        return Ok(());
    }

    // ── 路 A：用我们自己那一套 ──
    if is_ours_binary {
        // 我们自己的 colima **必须**用我们自己的家。不带就会去动 `~/.colima`，
        // 那正是 0.1.8 闯的祸
        if !home_is_ours {
            return Err(reject_audited(
                "colima_call",
                format!(
                    "这条命令用的是 Hunter 自己那份 colima，`COLIMA_HOME` 却是「{}」\
                     （不是 {}）—— 它会去动你家目录里的 colima，拒绝。",
                    safe(&home),
                    crate::redact::mask_home(&crate::paths::colima_home().to_string_lossy())
                ),
            ));
        }
        if colima_targets_profile(&rest) {
            match profile.as_deref() {
                Some(p) if p == crate::runtime::builtin::PROFILE => return Ok(()),
                Some(p) => {
                    return Err(reject_audited(
                        "colima_call",
                        format!(
                            "这条 colima 命令在 Hunter 自己的 COLIMA_HOME 里指向 profile「{}」，\
                             而我们只该动「{}」，拒绝。",
                            safe(p),
                            crate::runtime::builtin::PROFILE
                        ),
                    ))
                }
                None => {
                    return Err(reject_audited(
                        "colima_call",
                        format!(
                            "这条 colima 命令没写 `--profile {}` —— \
                             不写的话 colima 会去动名叫 default 的那一台，拒绝。",
                            crate::runtime::builtin::PROFILE
                        ),
                    ))
                }
            }
        }
        // list / version 之类：在我们自己的 COLIMA_HOME 里，碰不到用户的东西
        return Ok(());
    }

    // ── 路 B：起用户原有的那一台 ──
    //
    // 只允许 `start`，而且 profile 必须是他 `~/.colima` 里**本来就有**的。
    // 「不得新建」这一条就写在这里：名字对不上目录，就是新建。
    //
    // 先挡一种反过来的搅和：用**他的** colima 去写**我们的**家。没有任何正当理由
    if home_is_ours {
        return Err(reject_audited(
            "colima_call",
            "这条命令用的是你自己的 colima，`COLIMA_HOME` 却指向 Hunter 自己的目录 —— \
             两套东西不该搅在一起，拒绝。"
                .to_string(),
        ));
    }
    let Some(p) = profile else {
        return Err(reject_audited(
            "colima_call",
            format!(
                "这条 colima 命令的 COLIMA_HOME 是「{}」（不是 Hunter 自己的 {}），\
                 又没写 `--profile` —— 它会在你家目录里新建一台虚拟机，拒绝。",
                safe(&home),
                crate::redact::mask_home(&crate::paths::runtime_dir().to_string_lossy())
            ),
        ));
    };
    let existing = crate::runtime::effective::user_colima_profiles();
    if !existing.iter().any(|x| x.eq_ignore_ascii_case(&p)) {
        return Err(reject_audited(
            "colima_call",
            format!(
                "profile「{}」不在你 ~/.colima 里已有的那几个（看到的是：{}）—— \
                 启动器不会替你新建虚拟机，拒绝。",
                safe(&p),
                if existing.is_empty() {
                    "一个都没有".to_string()
                } else {
                    existing.join("、")
                }
            ),
        ));
    }
    if !rest.iter().any(|a| a == "start") {
        return Err(reject_audited(
            "colima_call",
            format!(
                "对你自己的 colima，启动器只做 `start`，不做别的（这条是 {}），拒绝。",
                safe(&rest.join(" "))
            ),
        ));
    }
    Ok(())
}

/// 这个 `COLIMA_HOME` 是我们自己的那一个吗。
fn colima_home_is_ours(home: &str) -> bool {
    if home.trim().is_empty() {
        return false;
    }
    match (
        canon_for_write(Path::new(home)),
        canon_for_write(&crate::paths::colima_home()),
    ) {
        (Ok(a), Ok(b)) => a == b,
        // 目录还没建出来时按字符串比（第一次装的时候就是这种情况）
        _ => norm_path(home) == norm_path(&crate::paths::colima_home().to_string_lossy()),
    }
}

/// 这个路径落在 `~/.hunter/runtime` 里吗。
fn under_runtime(p: &str) -> bool {
    if p.trim().is_empty() {
        return false;
    }
    let rt = crate::paths::runtime_dir();
    match (canon_for_write(Path::new(p)), canon_for_write(&rt)) {
        (Ok(a), Ok(b)) => a.starts_with(&b),
        _ => norm_path(p).starts_with(&norm_path(&rt.to_string_lossy())),
    }
}

fn norm_path(s: &str) -> String {
    s.trim().trim_end_matches(['/', '\\']).to_string()
}

/// 拒绝 + 记一条审计。**拒绝这件事本身要留痕**，否则「它想干什么、被拦下了没有」
/// 事后查不出来（I9 任务书 P0-2 点名要求）。
fn reject_audited(action: &str, msg: String) -> AppError {
    audit(
        action,
        &std::collections::BTreeMap::new(),
        Proposer::Orchestrator,
        None,
        &format!("拒绝：{msg}"),
    );
    reject(msg)
}

// ── 审计日志 ──────────────────────────────────────────────────────────────

/// `~/.hunter/logs/assist-audit.log`
pub fn audit_path() -> PathBuf {
    crate::paths::logs_dir().join("assist-audit.log")
}

/// 谁提出了这个动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Proposer {
    /// 确定性规则层
    Rule,
    /// 模型
    Model,
    /// 总指挥自己的固定流程
    Orchestrator,
    /// 用户点的
    User,
}

impl Proposer {
    pub fn as_str(self) -> &'static str {
        match self {
            Proposer::Rule => "rule",
            Proposer::Model => "model",
            Proposer::Orchestrator => "orchestrator",
            Proposer::User => "user",
        }
    }
}

/// 写一条审计。**通过和拒绝都写** —— 只记拒绝的话，「它到底做了什么」就查不出来了。
pub fn audit(
    action: &str,
    args: &std::collections::BTreeMap<String, String>,
    by: Proposer,
    level: Option<Level>,
    result: &str,
) {
    let line = serde_json::json!({
        "at": crate::timefmt::now_shanghai(),
        "action": crate::redact::redact(action),
        "args": args.iter().map(|(k, v)| (k.clone(), crate::redact::mask_home(&crate::redact::redact(v)))).collect::<std::collections::BTreeMap<_, _>>(),
        "by": by.as_str(),
        "level": level.map(|l| format!("{l:?}")),
        "result": crate::redact::mask_home(&crate::redact::redact(result)),
    });
    let p = audit_path();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let r = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p)
        .and_then(|mut f| writeln!(f, "{line}"));
    if let Err(e) = r {
        crate::lwarn!("写审计日志 {} 失败：{e}", p.display());
        return;
    }
    let _ = crate::paths::chmod_600(&p);
}

/// 读末尾若干条审计（设置页与测试用）。
pub fn audit_tail(n: usize) -> Vec<String> {
    let Ok(s) = std::fs::read_to_string(audit_path()) else {
        return Vec::new();
    };
    let all: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    all.iter()
        .skip(all.len().saturating_sub(n))
        .map(|s| (*s).to_string())
        .collect()
}

fn reject(msg: String) -> AppError {
    crate::lwarn!("守卫拒绝：{msg}");
    AppError::new(Code::NotImplemented, msg)
}

/// 把不可信的字符串截短、脱敏之后再放进错误信息里。
fn safe(s: &str) -> String {
    let s = crate::redact::redact(s);
    s.chars().take(60).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// I8：全自动档下一个都不问（包括 `Sensitive`）；另外两档照旧。
    ///
    /// 这条断言的反面 —— 「放松的只是问不问，不是拦不拦得住」——
    /// 由本文件里那一堆守卫测试保证：它们**一条都没改**，而且不看档位。
    #[test]
    fn 全自动档下一个都不问_另外两档照旧() {
        for lv in [Level::ReadOnly, Level::Safe, Level::Sensitive] {
            assert!(!Mode::Auto.needs_confirm(lv), "全自动档不该问 {lv:?}");
        }
        for m in [Mode::Confirm, Mode::Off] {
            assert!(!m.needs_confirm(Level::ReadOnly), "{m:?} 不该问只读动作");
            assert!(m.needs_confirm(Level::Safe), "{m:?}");
            assert!(m.needs_confirm(Level::Sensitive), "{m:?}");
        }
    }

    /// 全自动档**不放松任何禁止项**：档位一改，最容易忘的就是这件事。
    /// 这里把四条红线各挑一个代表，确认它们和档位毫无关系。
    #[test]
    fn 全自动档下四条禁止项一条不放松() {
        // ① 不删用户文件：动作表外的删除请求连计划都过不了
        assert!(
            super::super::actions::plan(&super::super::actions::Call::new("rm_user_documents"))
                .is_err()
        );
        // ② 不改网络与安全设置
        assert!(super::super::actions::spec("set_system_proxy").is_none());
        assert!(super::super::actions::spec("disable_gatekeeper").is_none());
        // ③ 不动别人的容器：只有 hunter 自己项目名的 compose 动作在表里
        assert!(super::super::actions::spec("compose_down_other").is_none());
        // ④ 不执行模型自编的命令
        assert!(super::super::actions::spec("run_shell").is_none());
        // ⑤ 路径守卫不看档位：~/.hunter 外的路径永远拒绝
        assert!(writable_path(Path::new("/etc/hosts")).is_err());
    }

    #[test]
    fn 认不得的档位落到最保守的一档() {
        assert_eq!(Mode::parse("auto"), Mode::Auto);
        assert_eq!(Mode::parse("AUTO"), Mode::Auto);
        assert_eq!(Mode::parse("off"), Mode::Off);
        assert_eq!(Mode::parse("confirm"), Mode::Confirm);
        assert_eq!(Mode::parse(""), Mode::Confirm);
        assert_eq!(Mode::parse("yolo"), Mode::Confirm, "认不得就按逐步确认来");
    }

    // ── I10：虚拟机 DNS 那道窄门 ──────────────────────────────────────

    /// 在一个干净的临时 `HUNTER_HOME` 里把 limactl 与要送进去的文件摆好，
    /// 返回 `(limactl 路径, 要送进去的那个文件路径)`。
    fn vm_dns_fixture() -> (crate::paths::TestHome, String, String) {
        let h = crate::paths::test_home("vmdns");
        let bin = crate::paths::runtime_dist().join("lima").join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let limactl = bin.join("limactl");
        std::fs::write(&limactl, b"#!/bin/sh\n").unwrap();
        let stage = crate::paths::runtime_dir().join("vm");
        std::fs::create_dir_all(&stage).unwrap();
        let f = stage.join("resolv.conf");
        std::fs::write(&f, b"nameserver 192.168.5.2\n").unwrap();
        (
            h,
            limactl.to_string_lossy().into_owned(),
            f.to_string_lossy().into_owned(),
        )
    }

    /// **模型走的永远是 [`argv`] 那一道，那一道一个字都没松。**
    ///
    /// I10 给「在我们自己那台虚拟机里写 resolv.conf」另开了一道窄门，
    /// 最容易出的事就是顺手把总门上那条禁令拆了。这里钉死它没被拆。
    #[test]
    fn 总门依旧拒绝一切碰_resolv_conf_的命令() {
        for v in [
            a(&["tee", "/etc/resolv.conf"]),
            a(&["docker", "run", "-v", "/etc/resolv.conf:/x"]),
            a(&["cp", "x", "/etc/resolv.conf"]),
            a(&[
                "limactl",
                "shell",
                "vm",
                "sudo",
                "cp",
                "x",
                "/etc/resolv.conf",
            ]),
        ] {
            assert!(argv(&v).is_err(), "总门该拒：{v:?}");
        }
    }

    #[test]
    fn 虚拟机_dns_那道门只认表里的命令() {
        let (_h, limactl, _f) = vm_dns_fixture();
        let inst = crate::runtime::vmdns::instance();
        // 表里的每一条都要过
        for cmd in VM_DNS_COMMANDS {
            let mut v = vec![
                limactl.clone(),
                "shell".into(),
                "--workdir".into(),
                "/".into(),
                inst.clone(),
            ];
            v.extend(cmd.iter().map(|s| s.to_string()));
            assert!(argv_vm_dns(&v).is_ok(), "表里这条该放行：{cmd:?}");
        }
        // 表外一律拒绝 —— 尤其是那些「看起来只差一点点」的
        for bad in [
            vec!["sudo", "rm", "-rf", "/"],
            vec!["sudo", "cat", "/etc/shadow"],
            vec!["sudo", "sh", "-c", "echo x > /etc/resolv.conf"],
            vec![
                "sudo",
                "cp",
                "--remove-destination",
                "/tmp/evil",
                "/etc/resolv.conf",
            ],
            vec!["sudo", "systemctl", "stop", "docker"],
            vec!["sudo", "systemctl", "restart", "sshd"],
            vec!["cat", "/etc/resolv.conf", "/etc/shadow"],
            vec!["getent", "hosts", "evil.example.com"],
        ] {
            let mut v = vec![
                limactl.clone(),
                "shell".into(),
                "--workdir".into(),
                "/".into(),
                inst.clone(),
            ];
            v.extend(bad.iter().map(|s| s.to_string()));
            assert!(argv_vm_dns(&v).is_err(), "表外这条该拒：{bad:?}");
        }
    }

    #[test]
    fn 虚拟机_dns_那道门只认_hunter_自己那台虚拟机() {
        let (_h, limactl, _f) = vm_dns_fixture();
        for other in [
            "colima-default",
            "default",
            "colima-hunter2",
            "docker-desktop",
        ] {
            let v = a(&[
                &limactl,
                "shell",
                "--workdir",
                "/",
                other,
                "cat",
                "/etc/resolv.conf",
            ]);
            assert!(argv_vm_dns(&v).is_err(), "别人的虚拟机该拒：{other}");
        }
    }

    #[test]
    fn 虚拟机_dns_那道门只认我们自己那份_limactl() {
        let (_h, _limactl, _f) = vm_dns_fixture();
        let inst = crate::runtime::vmdns::instance();
        for prog in [
            "/usr/local/bin/limactl",
            "limactl",
            "/opt/homebrew/bin/limactl",
        ] {
            let v = a(&[
                prog,
                "shell",
                "--workdir",
                "/",
                &inst,
                "cat",
                "/etc/resolv.conf",
            ]);
            assert!(argv_vm_dns(&v).is_err(), "不是我们那份该拒：{prog}");
        }
    }

    #[test]
    fn 送进虚拟机的文件必须是我们自己生成的那一个() {
        let (_h, limactl, f) = vm_dns_fixture();
        let inst = crate::runtime::vmdns::instance();
        let dst = format!("{inst}:/tmp/hunter-resolv.conf");
        // 我们自己在 runtime/vm 里生成的 —— 放行
        assert!(argv_vm_dns(&a(&[&limactl, "copy", "--backend=scp", &f, &dst])).is_ok());
        // 别处的文件 —— 拒绝
        for src in ["/etc/passwd", "/tmp/evil.conf"] {
            assert!(
                argv_vm_dns(&a(&[&limactl, "copy", "--backend=scp", src, &dst])).is_err(),
                "{src} 该拒"
            );
        }
        // 落点只能是那两个 —— 别的一律拒绝
        for bad in [
            "/etc/passwd",
            "/etc/resolv.conf",
            "/root/.ssh/authorized_keys",
        ] {
            let d = format!("{inst}:{bad}");
            assert!(
                argv_vm_dns(&a(&[&limactl, "copy", "--backend=scp", &f, &d])).is_err(),
                "落点 {bad} 该拒"
            );
        }
    }

    /// 表里凡是带 `sudo` 的，参数必须**逐字写死**，不能留任何可变位。
    #[test]
    fn 虚拟机里能跑的那张表没有可变位() {
        for cmd in VM_DNS_COMMANDS {
            assert!(!cmd.is_empty());
            for arg in *cmd {
                assert!(
                    !arg.contains('*') && !arg.contains('{') && !arg.contains('$'),
                    "表里不该有可变位：{cmd:?}"
                );
            }
        }
        // 而且「在虚拟机里能跑的」总数是有限且很小的 —— 它变大时这条会提醒人看一眼
        assert_eq!(
            VM_DNS_COMMANDS.len(),
            10,
            "改这张表要同时改 I10 报告里的那一节"
        );
    }

    #[test]
    fn 提权一律拒绝() {
        for p in ["sudo", "/usr/bin/sudo", "pkexec", "runas", "doas"] {
            assert!(argv(&a(&[p, "docker", "restart"])).is_err(), "{p} 该被拒");
        }
    }

    #[test]
    fn 改网络与安全设置一律拒绝() {
        for p in [
            "networksetup",
            "/usr/sbin/networksetup",
            "scutil",
            "pfctl",
            "iptables",
            "netsh",
            "security",
        ] {
            assert!(
                argv(&a(&[p, "-setwebproxy", "Wi-Fi", "127.0.0.1", "8080"])).is_err(),
                "{p} 该被拒"
            );
        }
    }

    #[test]
    fn 任意_shell_与脚本解释器一律拒绝() {
        for p in [
            "sh",
            "bash",
            "zsh",
            "python3",
            "osascript",
            "powershell",
            "node",
        ] {
            assert!(argv(&a(&[p, "-c", "echo hi"])).is_err(), "{p} 该被拒");
        }
    }

    #[test]
    fn 删文件的程序一律拒绝() {
        for p in ["rm", "/bin/rm", "shred", "del", "mv"] {
            assert!(
                argv(&a(&[p, "-rf", "/Users/me/Documents"])).is_err(),
                "{p} 该被拒"
            );
        }
    }

    #[test]
    fn 碰_hosts_与_launchdaemons_的参数一律拒绝() {
        // 就算程序名是白的（docker），参数里出现这些也拒绝
        assert!(argv(&a(&["docker", "cp", "x:/a", "/etc/hosts"])).is_err());
        assert!(argv(&a(&[
            "docker",
            "cp",
            "x:/a",
            "/Library/LaunchDaemons/y.plist"
        ]))
        .is_err());
    }

    #[test]
    fn docker_的危险子命令一律拒绝() {
        assert!(argv(&a(&["docker", "volume", "rm", "hunter_pgdata"])).is_err());
        assert!(argv(&a(&["docker", "volume", "prune", "-f"])).is_err());
        assert!(argv(&a(&["docker", "system", "prune", "-a"])).is_err());
        assert!(argv(&a(&["docker", "image", "prune", "-a"])).is_err());
    }

    #[test]
    fn down_带_v_一律拒绝() {
        assert!(argv(&a(&[
            "docker",
            "compose",
            "--project-name",
            "hunter",
            "down",
            "-v"
        ]))
        .is_err());
        assert!(argv(&a(&[
            "docker",
            "compose",
            "--project-name",
            "hunter",
            "down",
            "--volumes"
        ]))
        .is_err());
        // 不带 -v 的 down 是允许的
        assert!(argv(&a(&[
            "docker",
            "compose",
            "--project-name",
            "hunter",
            "down",
            "--remove-orphans"
        ]))
        .is_ok());
    }

    #[test]
    fn 改动容器状态的命令必须指名本项目() {
        // 没有 --project-name hunter → 拒绝（否则可能停到用户别的那一套）
        assert!(argv(&a(&["docker", "compose", "stop"])).is_err());
        assert!(argv(&a(&[
            "docker",
            "compose",
            "--project-name",
            "hunter-community",
            "stop"
        ]))
        .is_err());
        // 指名了就放行
        assert!(argv(&a(&[
            "docker",
            "compose",
            "--project-name",
            "hunter",
            "up",
            "-d"
        ]))
        .is_ok());
        assert!(argv(&a(&["docker", "compose", "-p", "hunter", "restart", "api"])).is_ok());
        // 只读命令不需要
        assert!(argv(&a(&["docker", "ps", "--format", "{{.Names}}"])).is_ok());
        assert!(argv(&a(&["docker", "version"])).is_ok());
    }

    #[test]
    fn 停用户另一套_hunter_会被拒() {
        // 场景 7 的第四条：诱导模型去停 hunter-community 旧栈
        assert!(argv(&a(&[
            "docker",
            "compose",
            "--project-name",
            "hunter-community",
            "down"
        ]))
        .is_err());
        assert!(argv(&a(&["docker", "stop", "hunter-community-web-1"])).is_err());
    }

    #[test]
    fn 路径守卫解得开_双点_与软链() {
        // **这一条原来没拿 `HUNTER_HOME` 那把锁**，靠的是「跑测试这台机器上
        // 恰好有一个 ~/.hunter/app 目录」。I9 新加的几条测试会把 `HUNTER_HOME`
        // 指到临时目录，它就当场红了 —— 红在这里，肇事者在别的文件里，
        // 正是 I7 记过一次的那类「一直坏着、只是还没被看见」。顺手修掉。
        let _g = crate::paths::test_home("guard-writable");
        let root = crate::paths::root();
        std::fs::create_dir_all(root.join("app")).unwrap();
        // 正常的：~/.hunter 下的文件
        assert!(writable_path(&root.join("launcher.toml")).is_ok());
        assert!(writable_path(&root.join("app").join(".env")).is_ok());
        // 用 .. 跳出去的：拒绝
        let escape = root.join("..").join("Documents").join("x.txt");
        assert!(
            writable_path(&escape).is_err(),
            "{} 应该被拒",
            escape.display()
        );
    }

    /// I6：凭据助手找不到时，**不许**去改用户的 `~/.docker/config.json`。
    /// 这一条不是靠「代码里我们没写那一行」保证的，是守卫真的拦得住。
    #[test]
    fn 用户的_docker_配置写不进去() {
        let _g = crate::paths::test_home("guard-user-docker-cfg");
        let user_cfg = crate::paths::home().join(".docker").join("config.json");
        assert!(
            writable_path(&user_cfg).is_err(),
            "{} 是用户的文件，必须拒绝",
            user_cfg.display()
        );
        // 我们自己那一份在 ~/.hunter 里，允许
        let ours = crate::dockercfg::isolated_dir().join("config.json");
        std::fs::create_dir_all(crate::dockercfg::isolated_dir()).unwrap();
        assert!(writable_path(&ours).is_ok(), "{} 应当允许", ours.display());
        // 而且它也不在「可删清单」里 —— 我们只写不删
        assert!(deletable_path(&ours).is_err());
    }

    #[test]
    fn 删除只限启动器自己生成的文件() {
        // **要拿 `HUNTER_HOME` 那把锁**（I9 在 CI 上撞出来的）：这一条读的是
        // `paths::root()`，而别的文件里有测试会把那个变量指到临时目录。
        // 本地跑一直绿只是时序没撞上 —— Windows runner 上第一次就红了。
        let _g = crate::paths::test_home("guard-deletable-files");
        let root = crate::paths::root();
        std::fs::create_dir_all(root.join("app")).unwrap();
        // 白名单里的
        assert!(deletable_path(&crate::paths::override_file()).is_ok());
        // ~/.hunter 里但不是我们生成的（用户自己放的备份）
        assert!(deletable_path(&root.join("我的笔记.txt")).is_err());
        // launcher.toml 是设置，也不许删
        assert!(deletable_path(&crate::paths::launcher_toml()).is_err());
        // ~/.hunter 外的
        assert!(deletable_path(&crate::paths::home().join("Documents/a.txt")).is_err());
    }

    #[test]
    fn 审计能写能读() {
        // **自己的临时工作目录**：审计日志是一个追加文件，两条测试并行写同一个
        // 文件、又各自 `audit_tail(1)`，必然互相把对方的那一行顶掉。
        // 这个race 以前一直在（只是没撞上），I9 新加的几条审计把它撞出来了。
        let _g = crate::paths::test_home("guard-audit-rw");
        let mut args = std::collections::BTreeMap::new();
        args.insert("port".to_string(), "8100".to_string());
        audit(
            "remap_ports",
            &args,
            Proposer::Rule,
            Some(Level::Safe),
            "ok",
        );
        let t = audit_tail(1);
        assert_eq!(t.len(), 1);
        assert!(t[0].contains("remap_ports"), "{}", t[0]);
        assert!(t[0].contains("\"by\":\"rule\""), "{}", t[0]);
    }

    #[test]
    fn 审计里不会出现_key() {
        let _g = crate::paths::test_home("guard-audit-key");
        let mut args = std::collections::BTreeMap::new();
        args.insert(
            "x".to_string(),
            "hunt_tools_abcdefghijklmnopqrstuvwxyz012345".to_string(),
        );
        audit(
            "x",
            &args,
            Proposer::Model,
            None,
            "hunt_tools_abcdefghijklmnopqrstuvwxyz012345",
        );
        let t = audit_tail(1);
        assert!(!t[0].contains("abcdefghij"), "{}", t[0]);
    }

    // ── I7 ────────────────────────────────────────────────────────────────

    /// 整棵删的白名单里**只有内置运行时**那一个目录。
    #[test]
    fn 只有内置运行时目录可以整棵删() {
        let _g = crate::paths::test_home("guard-deletable-tree");
        let root = crate::paths::root();
        std::fs::create_dir_all(root.join("runtime")).unwrap();
        assert!(deletable_tree(&crate::paths::runtime_dir()).is_ok());
        // ~/.hunter 里别的目录一个都不行 —— 它们放着用户的东西
        for d in [
            crate::paths::app_dir(),
            crate::paths::backups_dir(),
            crate::paths::diagnostics_dir(),
            crate::paths::logs_dir(),
            root.clone(),
        ] {
            std::fs::create_dir_all(&d).unwrap();
            assert!(
                deletable_tree(&d).is_err(),
                "{} 不该在「可以整棵删」的清单里",
                d.display()
            );
        }
        // ~/.hunter 外的更不行
        assert!(deletable_tree(&crate::paths::home().join("Documents")).is_err());
    }

    /// 没接管任何东西时，指向别人 compose 项目的命令**一律拒绝**。
    #[test]
    fn 没接管时不许动别人的项目() {
        let argv = a(&[
            "docker",
            "compose",
            "--project-name",
            "hunter-community",
            "stop",
        ]);
        let e = argv_takeover_for(&argv, "").expect_err("没接管就该拒绝");
        assert!(e.msg.contains("没有在管理"), "{}", e.msg);
        // 只有空白也一样
        assert!(argv_takeover_for(&argv, "   ").is_err());
    }

    /// 接管之后：只放行 stop / start / restart / logs / ps，
    /// **而且只对那一个项目**；down / rm / kill 一概拒绝。
    #[test]
    fn 接管态只放行三个子命令且只对那一个项目() {
        const P: &str = "hunter-community";
        let ok =
            |sub: &str| argv_takeover_for(&a(&["docker", "compose", "--project-name", P, sub]), P);
        for sub in ["stop", "start", "restart", "logs", "ps"] {
            assert!(ok(sub).is_ok(), "{sub} 该放行：{:?}", ok(sub).err());
        }
        for sub in ["down", "rm", "kill", "up", "create"] {
            assert!(ok(sub).is_err(), "{sub} 该拒绝");
        }
        // 指向**别的**项目：拒绝
        assert!(argv_takeover_for(
            &a(&[
                "docker",
                "compose",
                "--project-name",
                "hunter-other",
                "stop"
            ]),
            P
        )
        .is_err());
        // 连我们自己那一套都不行 —— 那条路走 `argv`，不走这里
        assert!(argv_takeover_for(
            &a(&["docker", "compose", "--project-name", PROJECT, "stop"]),
            P
        )
        .is_err());
        // 不写项目名：拒绝
        assert!(argv_takeover_for(&a(&["docker", "compose", "stop"]), P).is_err());
        // 带 -v：拒绝（通用那一道拦的）
        assert!(argv_takeover_for(
            &a(&["docker", "compose", "--project-name", P, "stop", "-v"]),
            P
        )
        .is_err());
    }

    /// 接管态那道口子**不放松**通用规则：sudo、改网络、删文件照样拒。
    #[test]
    fn 接管态不放松通用规则() {
        const P: &str = "hunter-community";
        for prog in ["sudo", "rm", "bash", "networksetup"] {
            assert!(
                argv_takeover_for(
                    &a(&[prog, "docker", "compose", "--project-name", P, "stop"]),
                    P
                )
                .is_err(),
                "{prog} 该被通用那一道拦下"
            );
        }
        // hosts 这类路径照样拦
        assert!(argv_takeover_for(
            &a(&[
                "docker",
                "compose",
                "--project-name",
                P,
                "stop",
                "/etc/hosts"
            ]),
            P
        )
        .is_err());
    }

    // ── I8 的两道窄门 ────────────────────────────────────────────────────

    /// 只读探测这道门**只认两条**，而且参数一个字都不能变。
    #[test]
    fn 只读探测的白名单之外一律拒绝() {
        assert!(argv_readonly_probe(&a(&["/usr/sbin/scutil", "--proxy"])).is_ok());
        assert!(argv_readonly_probe(&a(&["scutil", "--proxy"])).is_ok());
        assert!(argv_readonly_probe(&a(&[
            "reg",
            "query",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings"
        ]))
        .is_ok());
        // 会**改**设置的写法：一条都不许
        for bad in [
            vec!["/usr/sbin/scutil"],
            vec!["/usr/sbin/scutil", "--set", "HostName", "x"],
            vec!["/usr/sbin/scutil", "--dns"],
            vec!["/usr/sbin/scutil", "--proxy", "--set"],
            vec!["networksetup", "-setwebproxy", "Wi-Fi", "127.0.0.1", "7897"],
            vec![
                "reg",
                "add",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings",
            ],
            vec!["reg", "query", r"HKLM\SYSTEM"],
        ] {
            assert!(
                argv_readonly_probe(&a(&bad)).is_err(),
                "这条该被拒：{bad:?}"
            );
        }
        // 通用那一道**没有被放松**：模型走的是它，scutil 照样拒
        assert!(argv(&a(&["/usr/sbin/scutil", "--proxy"])).is_err());
    }

    /// 装 Homebrew 那道门：只能跑**我们自己下到 `~/.hunter/runtime` 里**的那个脚本。
    #[test]
    fn 安装脚本只能跑自己下的那一个() {
        let _g = crate::paths::test_home("guard-install-script");
        let dir = crate::paths::runtime_dir().join("cache");
        std::fs::create_dir_all(&dir).unwrap();
        let ok = dir.join("brew-install.sh");
        std::fs::write(&ok, b"#!/bin/bash\nexit 0\n").unwrap();
        assert!(argv_install_script(&a(&["/bin/bash", &ok.to_string_lossy()])).is_ok());

        // 别的地方的脚本：拒
        let outside = std::env::temp_dir().join("hunter-guard-outside.sh");
        std::fs::write(&outside, b"#!/bin/bash\n").unwrap();
        assert!(argv_install_script(&a(&["/bin/bash", &outside.to_string_lossy()])).is_err());
        // 不存在的脚本：拒
        assert!(argv_install_script(&a(&["/bin/bash", "/绝对没有这个文件.sh"])).is_err());
        // 多一个参数：拒（`bash -c "任意代码"` 正是要挡的东西）
        assert!(argv_install_script(&a(&["/bin/bash", "-c", "rm -rf ~"])).is_err());
        // 换一个解释器：拒
        assert!(argv_install_script(&a(&["/bin/sh", &ok.to_string_lossy()])).is_err());
        // 通用那一道照样拒 bash —— 模型走的是它
        assert!(argv(&a(&["/bin/bash", &ok.to_string_lossy()])).is_err());

        let _ = std::fs::remove_file(&ok);
        let _ = std::fs::remove_file(&outside);
    }

    /// 提权那张表：**表外一律拒绝**，表里的参数形状也写死。
    #[test]
    fn 提权只允许表里那两件事() {
        let _g = crate::paths::test_home("guard-privileged");
        assert!(argv_privileged(&a(&["/usr/bin/systemctl", "start", "docker"])).is_ok());
        assert!(argv_privileged(&a(&["systemctl", "start", "docker"])).is_ok());
        for bad in [
            vec!["systemctl", "stop", "docker"],
            vec!["systemctl", "start", "sshd"],
            vec!["rm", "-rf", "/"],
            vec!["bash", "-c", "curl x | sh"],
            vec!["networksetup", "-setwebproxy"],
            vec!["dpkg", "-i", "/tmp/x.deb"],
            vec!["chown", "-R", "root", "/"],
        ] {
            assert!(argv_privileged(&a(&bad)).is_err(), "这条该被拒：{bad:?}");
        }
        // 自己下到 ~/.hunter/updates/ 里的 .deb：放行
        let up = crate::paths::updates_dir();
        std::fs::create_dir_all(&up).unwrap();
        let deb = up.join("hunter-launcher_0.0.0_amd64.deb");
        std::fs::write(&deb, b"x").unwrap();
        assert!(argv_privileged(&a(&["dpkg", "-i", &deb.to_string_lossy()])).is_ok());
        // 同一个目录里的非 .deb：拒
        let other = up.join("x.sh");
        std::fs::write(&other, b"x").unwrap();
        assert!(argv_privileged(&a(&["dpkg", "-i", &other.to_string_lossy()])).is_err());
        let _ = std::fs::remove_file(&deb);
        let _ = std::fs::remove_file(&other);
    }

    // ── I9 的 P0-2：colima / limactl 隔离守卫 ────────────────────────────

    /// 把 `COLIMA_HOME` 指到 Hunter 自己那一份的 env 对。
    fn ours_env() -> Vec<(String, String)> {
        vec![(
            "COLIMA_HOME".to_string(),
            crate::paths::colima_home().to_string_lossy().into_owned(),
        )]
    }

    fn pairs(v: &[(String, String)]) -> Vec<(&str, &str)> {
        v.iter().map(|(k, x)| (k.as_str(), x.as_str())).collect()
    }

    /// **这一条就是 0.1.8 在用户 Mac 上闯的祸**（2026-09-22 14:49:15）：
    /// 我们自己下的 colima + 默认的 `~/.colima` + 没有 `--profile`。
    /// 它在用户家目录里建了一台没人要的 `default` 虚拟机（1.4 GB）。
    #[test]
    fn 裸的_colima_start_必须被拒绝() {
        let _g = crate::paths::test_home("guard-colima-bare");
        let bin = crate::paths::runtime_bin().join("colima");
        // 显式给一个空的 COLIMA_HOME —— 模拟「谁都没设，用默认的 ~/.colima」
        let e = colima_call(&bin.to_string_lossy(), &["start"], &[("COLIMA_HOME", "")])
            .expect_err("裸 colima start 必须被拒");
        assert!(
            e.msg.contains("你家目录") || e.msg.contains("新建") || e.msg.contains("--profile"),
            "拒绝的理由要说清是为什么：{}",
            e.msg
        );
        // **拒绝要留痕**
        let tail = audit_tail(5).join("\n");
        assert!(tail.contains("colima_call"), "拒绝没写进审计：{tail}");
        assert!(tail.contains("拒绝"), "{tail}");
    }

    /// 带齐隔离参数的那一条（`start_builtin_runtime` 真正发出去的那条）必须放行。
    #[test]
    fn 带上_colima_home_与_profile_hunter_才放行() {
        let _g = crate::paths::test_home("guard-colima-ok");
        let bin = crate::paths::runtime_bin().join("colima");
        let env = ours_env();
        colima_call(
            &bin.to_string_lossy(),
            &["start", "--profile", "hunter", "--vm-type", "vz"],
            &pairs(&env),
        )
        .expect("带齐隔离参数的必须放行");
    }

    /// 在我们自己的 `COLIMA_HOME` 里指向别的 profile —— 也拒。
    /// 「只动 hunter 这一台」不因为家目录对了就放松。
    #[test]
    fn 我们自己的_colima_home_里也只许动_hunter() {
        let _g = crate::paths::test_home("guard-colima-other");
        let bin = crate::paths::runtime_bin().join("colima");
        let env = ours_env();
        let e = colima_call(
            &bin.to_string_lossy(),
            &["start", "--profile", "default"],
            &pairs(&env),
        )
        .expect_err("profile 不是 hunter 要拒");
        assert!(e.msg.contains("hunter"), "{}", e.msg);
    }

    /// 用户 `~/.colima` 里**没有**的 profile —— 拒。
    /// 「不得新建」这条红线就写在这里：名字对不上目录，就是新建。
    #[test]
    fn 不许在用户家目录里新建_profile() {
        let _g = crate::paths::test_home("guard-colima-new");
        let e = colima_call(
            "/usr/local/bin/colima",
            &["start", "--profile", "hunter-launcher-造出来的"],
            &[("COLIMA_HOME", "/home/someone/.colima")],
        )
        .expect_err("用户那边没有这个 profile，要拒");
        assert!(e.msg.contains("不会替你新建"), "{}", e.msg);
    }

    /// `list` / `version` 这种不针对某一台虚拟机的，在我们自己的家目录里放行。
    #[test]
    fn 不针对某一台虚拟机的子命令在我们自己家目录里放行() {
        let _g = crate::paths::test_home("guard-colima-list");
        let bin = crate::paths::runtime_bin().join("colima");
        let env = ours_env();
        colima_call(&bin.to_string_lossy(), &["list"], &pairs(&env)).expect("list 该放行");
        colima_call(&bin.to_string_lossy(), &["version"], &pairs(&env)).expect("version 该放行");
    }

    /// limactl 靠 `LIMA_HOME` 隔离：不指向 `~/.hunter/runtime` 就拒。
    #[test]
    fn limactl_的_lima_home_必须在我们自己的目录里() {
        let _g = crate::paths::test_home("guard-lima");
        let ok = crate::paths::lima_home().to_string_lossy().into_owned();
        colima_call("/x/limactl", &["list"], &[("LIMA_HOME", ok.as_str())]).expect("该放行");
        let e = colima_call("/x/limactl", &["list"], &[("LIMA_HOME", "/home/u/.lima")])
            .expect_err("指向用户的 ~/.lima 要拒");
        assert!(e.msg.contains("LIMA_HOME"), "{}", e.msg);
    }

    /// **机器上有内置运行时残骸时，起用户原有的 profile 照样要放行。**
    ///
    /// 这一条是发布前最后一刻抓到的：`runtime::env` 会给所有子进程钉上
    /// 我们的 `COLIMA_HOME`，于是「起他自己那台」这条完全正当的命令
    /// 会被判成「我们自己那一路」，再因为 profile 不是 `hunter` 被拒。
    /// 判「谁的 colima」要看**程序路径**，不是看环境变量。
    #[test]
    fn 有残骸时起用户原有的_profile_照样放行() {
        let _g = crate::paths::test_home("guard-colima-user-residue");
        crate::runtime::effective::make_fake_tools(); // 残骸在
        let profiles = crate::runtime::effective::user_colima_profiles();
        let Some(p) = profiles.first().cloned() else {
            // 跑测试这台机器上没有 ~/.colima —— 那就验「拒绝的理由是对的那一个」
            let e = colima_call(
                "/usr/local/bin/colima",
                &["start", "--profile", "他的"],
                &[("COLIMA_HOME", "/home/someone/.colima")],
            )
            .expect_err("他那边没有这个 profile");
            assert!(e.msg.contains("不会替你新建"), "{}", e.msg);
            return;
        };
        colima_call(
            "/usr/local/bin/colima",
            &["start", "--profile", &p],
            &[("COLIMA_HOME", "/home/someone/.colima")],
        )
        .expect("用户自己的 colima + 他原有的 profile，必须放行");
    }

    /// 反过来也不许：拿**他的** colima 去写**我们的**家。
    #[test]
    fn 不许拿用户的_colima_去写我们自己的家() {
        let _g = crate::paths::test_home("guard-colima-cross");
        let ours = crate::paths::colima_home().to_string_lossy().into_owned();
        let e = colima_call(
            "/usr/local/bin/colima",
            &["start", "--profile", "hunter"],
            &[("COLIMA_HOME", ours.as_str())],
        )
        .expect_err("两套东西不该搅在一起");
        assert!(e.msg.contains("搅在一起"), "{}", e.msg);
    }

    /// 别的程序一律不受这道守卫影响（零开销、零误伤）。
    #[test]
    fn 这道守卫只管_colima_与_limactl() {
        let _g = crate::paths::test_home("guard-colima-other-prog");
        colima_call("/usr/bin/docker", &["ps"], &[]).expect("docker 不归这道管");
        colima_call("/usr/bin/echo", &["start"], &[]).expect("echo 不归这道管");
    }

    /// **守卫挂在真正 spawn 之前**：不管谁写的这条命令，它都到不了 `fork`。
    /// 这一条走的是 [`crate::proc`] 的正门，不是直接调守卫函数。
    #[test]
    fn 裸的_colima_start_到不了_spawn() {
        let _g = crate::paths::test_home("guard-colima-proc");
        // 造一个真的能跑的假 colima：守卫要是没拦住，它会成功返回 0
        crate::runtime::effective::make_fake_tools();
        let bin = crate::paths::runtime_bin().join(if cfg!(windows) {
            "colima.exe"
        } else {
            "colima"
        });
        let e =
            crate::proc::run_with_env(&bin.to_string_lossy(), &["start"], &[("COLIMA_HOME", "")])
                .expect_err("守卫必须在 spawn 之前拦住它");
        assert!(
            e.msg.contains("你家目录") || e.msg.contains("--profile") || e.msg.contains("新建"),
            "{}",
            e.msg
        );
    }
}
