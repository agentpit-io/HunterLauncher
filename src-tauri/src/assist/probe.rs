//! 采集一份**结构化诊断**（I4 §三）。
//!
//! 这份东西有两个去处：界面上的「复制诊断信息」按钮，以及送去网关问模型。
//! 所以它必须同时满足两件事：
//!
//! 1. **说的都是真的**：读不到就写「读不到」和原因，一个数字都不编（红线 1）。
//! 2. **一个字的隐私都不带**：key 与 token 一律不出现，路径里的用户名换成
//!    `<用户目录>`。两道都走 [`crate::redact`]，出口再整体过一遍
//!    [`assert_no_secret`]。
//!
//! 内容按 I4 点名的那几项来：系统与架构、版本、探测过哪些路径及结果、
//! 命令原始输出、错误码、日志末尾若干行、端口与磁盘状况。

use std::path::Path;
use std::time::Duration;

use serde::Serialize;

use crate::runtime::which;

/// 送去模型那边的日志行数。再多就是在烧 token —— 失败的原话基本都在最后这些行里。
pub const LOG_LINES: usize = 40;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppPresence {
    /// 动作白名单里的 id（orbstack / docker-desktop / colima / systemd），
    /// 外加 I9 新增的 `builtin` —— **它不是「用户装了什么」，是我们自己装的那一套**，
    /// 所以 `start_runtime` 永远不会收到这个 id（规则层看到它走的是内置主线）。
    pub id: String,
    pub label: String,
    /// 装没装。读不到就是 `None`
    pub installed: Option<bool>,
    /// 进程起没起。读不到就是 `None`
    pub running: Option<bool>,
    /// 怎么判断出来的（路径 / 命令），给人看
    pub evidence: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortState {
    pub service: String,
    pub port: u16,
    /// 三路都说没人听。**I11 起它不再是「这个端口可用」的同义词**
    pub free: bool,
    /// 占着这个端口的是**我们自己这一套** hunter 容器。
    ///
    /// 这不是冲突：第二次打开启动器、或者装好之后再来诊断时，
    /// 3101 本来就该被我们的 web 容器占着。不分这一档的话，规则层会对着
    /// 一套跑得好好的 Hunter 说「5 个端口被别的程序占着」（I4 取错误页截图时撞到）。
    pub ours: bool,
    /// 空闲 / 被 Hunter 自己占用 / 被其他程序占用（I11 · U4）
    pub kind: crate::ports::Kind,
    /// 占用者原话，一行人话。空闲时是空串
    pub occupied_by: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CmdOut {
    /// 跑了什么（已脱敏、用户名已换掉）
    pub command: String,
    pub status: Option<i32>,
    pub output: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub launcher_version: String,
    pub os: String,
    pub arch: String,
    /// 系统版本。读不到就是 `None`（不猜）
    pub os_version: Option<String>,
    /// 触发这次诊断的错误码
    pub error_code: Option<String>,
    /// 触发这次诊断的错误说明（已脱敏）
    pub error_message: Option<String>,
    /// 现在走到向导的哪一步
    pub stage: Option<String>,

    /// **现在生效的容器运行时是谁**（I9）。一行人话，来自
    /// [`crate::runtime::effective::current`]。
    ///
    /// 这一条是 I9 加的，因为 0.1.8 那次失败里模型拿到的证据**恰好缺了它**：
    /// 报文里写着「docker 装了、daemon 没在跑」，却没有一个字说明那个 docker
    /// 是我们自己下到 `~/.hunter/runtime` 里的、它对应的 daemon 是我们自己
    /// 那台还没起来的虚拟机。模型于是照着「用户装了 Colima」去想办法。
    pub effective_runtime: String,

    pub docker_installed: bool,
    pub daemon_running: bool,
    pub docker_runtime: String,
    pub docker_client: Option<String>,
    pub docker_server: Option<String>,
    pub compose_version: Option<String>,
    pub compose_mode: Option<String>,
    /// 最终用的 docker 路径
    pub docker_path: Option<String>,
    /// 探过的每一个位置，一行一条
    pub probe_lines: Vec<String>,
    /// PATH 环境变量本身 —— macOS 那个 P0 的关键证据就是它
    pub env_path: String,

    pub apps: Vec<AppPresence>,
    pub ports: Vec<PortState>,
    pub disk_free: Option<u64>,
    pub disk_total: Option<u64>,

    pub registry_id: String,
    pub registry_prefix: String,
    pub hunter_tag: String,

    /// **Hunter 自己那台虚拟机的 DNS 现状**（I10）。
    ///
    /// 只有内置运行时的虚拟机在跑的时候才有值 —— 别的情况这一项没有意义
    /// （用户自己的 Docker 的 DNS 是他这台电脑的事，启动器不碰）。
    ///
    /// 为什么要进这份报文：0.1.9 那次失败里，送给模型的证据**恰好缺了它**。
    /// 报文里写着「opencode 不健康」，却没有一个字说明那台虚拟机连域名都解析不了，
    /// 于是模型只能一遍遍去读容器日志（实测读了 4 次），一直没走到根上。
    pub vm_dns: Option<crate::runtime::vmdns::VmDns>,

    pub commands: Vec<CmdOut>,
    pub log_tail: Vec<String>,
}

/// 采一份。`error_code` / `error_message` / `stage` 由调用方给（它才知道用户卡在哪）。
pub fn collect(
    error_code: Option<&str>,
    error_message: Option<&str>,
    stage: Option<&str>,
) -> Report {
    let cfg = crate::config::LauncherConfig::load();
    let d = crate::runtime::docker::detect();
    let pr = which::docker_probe();

    let mut commands = Vec::new();
    // `docker version` 的原始输出 —— 版本、运行时、连不连得上 daemon 全在里面
    let bin = pr.resolved.clone().unwrap_or_else(|| "docker".into());
    match crate::proc::run_timeout(&bin, &["version"], Duration::from_secs(20)) {
        Ok(r) => commands.push(CmdOut {
            command: clean(&format!("{bin} version")),
            status: r.status,
            output: clean(&join_out(&r)),
        }),
        Err(e) => commands.push(CmdOut {
            command: clean(&format!("{bin} version")),
            status: None,
            output: clean(&format!("起不来：{}", e.msg)),
        }),
    }

    // 我们自己这一套正占着的端口（第二次打开 / 装好之后再诊断时一定有）
    let ours: Vec<u16> = crate::compose::ps()
        .unwrap_or_default()
        .iter()
        .filter_map(|s| s.port)
        .collect();
    // **现场只采一次**（I9）。原来这里是 `port_free()` 逐个端口调，而那个函数
    // 每次都自己 `Survey::collect()` —— 5 个端口就是 5 次 `docker ps` + 5 次 `lsof`。
    // 用户 Mac 上 0.1.8 的日志里那一串重复的「docker ps 查已发布端口失败」
    // （同一秒内 5 到 7 条）就是这么来的：既慢，又把日志刷得没法看。
    let survey = crate::ports::Survey::collect();
    let ports = cfg
        .hunter
        .ports
        .as_pairs()
        .iter()
        .map(|(name, port)| {
            let v = survey.verdict(*port, &[crate::config::PROJECT]);
            // `compose ps` 说这个端口是我们自己发布的 —— 这一路和 `docker ps`
            // 互为佐证。任一路说「是我们自己」就算我们自己，不能报成「空闲」：
            // 0.1.9 在用户 Mac 上导出的诊断里，五个正被 Hunter 占着的端口
            // 全印着「空闲」，而用户当时正用着那五个端口上的服务。
            let ours = ours.contains(port);
            let kind = match v.kind() {
                crate::ports::Kind::Free if ours => crate::ports::Kind::Mine,
                k => k,
            };
            let occupied_by = match kind {
                crate::ports::Kind::Free => String::new(),
                crate::ports::Kind::Mine if v.mine.is_empty() => {
                    "docker compose ps 报的本项目发布端口".to_string()
                }
                crate::ports::Kind::Mine => v
                    .mine
                    .iter()
                    .map(crate::ports::Occupant::human)
                    .collect::<Vec<_>>()
                    .join("；"),
                crate::ports::Kind::Other => v
                    .occupants
                    .iter()
                    .map(crate::ports::Occupant::human)
                    .collect::<Vec<_>>()
                    .join("；"),
            };
            PortState {
                service: (*name).to_string(),
                port: *port,
                free: v.free && !ours,
                ours,
                kind,
                occupied_by: clean(&occupied_by),
            }
        })
        .collect();

    let (disk_free, disk_total) = match disk_free(&crate::paths::root()) {
        Some((f, t)) => (Some(f), Some(t)),
        None => (None, None),
    };

    Report {
        launcher_version: env!("CARGO_PKG_VERSION").to_string(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        os_version: os_version(),
        error_code: error_code.map(str::to_string),
        error_message: error_message.map(clean),
        stage: stage.map(str::to_string),

        effective_runtime: clean(&crate::runtime::effective::current().one_line()),

        docker_installed: d.installed,
        daemon_running: d.daemon_running,
        docker_runtime: format!("{:?}", d.runtime),
        docker_client: d.client_version.clone(),
        docker_server: d.server_version.clone(),
        compose_version: d.compose_version.clone(),
        compose_mode: d.compose_mode.clone(),
        docker_path: pr.resolved.as_deref().map(clean),
        probe_lines: pr.detail_lines().iter().map(|l| clean(l)).collect(),
        env_path: clean(
            &std::env::var("PATH").unwrap_or_else(|_| "（没有 PATH 这个环境变量）".into()),
        ),

        apps: runtime_apps(),
        ports,
        disk_free,
        disk_total,

        registry_id: cfg.hunter.registry_id.clone(),
        registry_prefix: cfg.hunter.registry_prefix.clone(),
        hunter_tag: cfg.hunter.tag.clone(),

        // 只在内置运行时的虚拟机跑着的时候查 —— 别的情况这一项没有意义，
        // 而且查一次要起一个 `limactl shell` 子进程，不该每次诊断都白花
        vm_dns: crate::runtime::vmdns::applicable()
            .ok()
            .and_then(|()| crate::runtime::vmdns::probe().ok()),

        commands,
        log_tail: crate::log::tail_file(LOG_LINES)
            .iter()
            .map(|l| clean(l))
            .collect(),
    }
}

/// 脱敏 + 换掉用户名。**采集路径上的每一个字符串都要过这里**。
fn clean(s: &str) -> String {
    crate::redact::mask_home(&crate::redact::redact(s))
}

fn join_out(r: &crate::proc::Ran) -> String {
    let mut s = r.stdout.trim().to_string();
    if !r.stderr.trim().is_empty() {
        if !s.is_empty() {
            s.push('\n');
        }
        s.push_str("stderr: ");
        s.push_str(r.stderr.trim());
    }
    s
}

impl Report {
    /// 送给模型的那一份（纯文本，比 JSON 省 token，读起来也顺）。
    /// 端口一节。**分三类**（I11 · U4）。
    ///
    /// 0.1.9 那份用户导出的诊断里，五个正被 Hunter 自己占着的端口全印着
    /// 「空闲」—— 送给模型的前提一错，后面的推理全是白做的。
    ///
    /// 命令行的 `--diagnose` 与界面上的「诊断报文」用的是这同一个函数，
    /// 两边的口径因此不会再各写各的。
    pub fn ports_section(&self) -> String {
        let mut s = String::from("\n## 端口（空闲 / 被 Hunter 自己占用 / 被其他程序占用）\n");
        for p in &self.ports {
            let label = match p.kind {
                crate::ports::Kind::Free => "空闲",
                crate::ports::Kind::Mine => "被 Hunter 自己占用（正常，不是冲突）",
                crate::ports::Kind::Other => "被其他程序占用",
            };
            if p.occupied_by.is_empty() {
                s.push_str(&format!("{} {} {}\n", p.service, p.port, label));
            } else {
                s.push_str(&format!(
                    "{} {} {} · {}\n",
                    p.service, p.port, label, p.occupied_by
                ));
            }
        }
        s
    }

    pub fn to_prompt(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!(
            "系统: {} {} {}\n启动器: {}\n",
            self.os,
            self.arch,
            self.os_version.as_deref().unwrap_or("（版本读不到）"),
            self.launcher_version
        ));
        if let Some(c) = &self.error_code {
            s.push_str(&format!("错误码: {c}\n"));
        }
        if let Some(m) = &self.error_message {
            s.push_str(&format!("错误说明: {m}\n"));
        }
        if let Some(st) = &self.stage {
            s.push_str(&format!("卡在: {st}\n"));
        }
        s.push_str(&format!("\n{}\n", self.effective_runtime));
        s.push_str(&format!(
            "Docker: 装了={} daemon在跑={} 运行时={} 客户端={} 服务端={}\ncompose: {} / {}\n",
            self.docker_installed,
            self.daemon_running,
            self.docker_runtime,
            self.docker_client.as_deref().unwrap_or("—"),
            self.docker_server.as_deref().unwrap_or("—"),
            self.compose_version.as_deref().unwrap_or("—"),
            self.compose_mode.as_deref().unwrap_or("—"),
        ));
        s.push_str(&format!(
            "用的 docker 路径: {}\n本进程的 PATH: {}\n",
            self.docker_path.as_deref().unwrap_or("（一个都没找到）"),
            self.env_path
        ));
        s.push_str("\n## 按顺序探过的位置\n");
        for l in &self.probe_lines {
            s.push_str(l);
            s.push('\n');
        }
        s.push_str("\n## 容器运行时装没装、起没起\n");
        for a in &self.apps {
            s.push_str(&format!(
                "{}（{}）装了={} 在跑={} — {}\n",
                a.label,
                a.id,
                tri(a.installed),
                tri(a.running),
                a.evidence
            ));
        }
        s.push_str(&self.ports_section());
        s.push_str(&format!(
            "\n磁盘: {}\n镜像源: {}（{}） · Hunter tag {}\n",
            match (self.disk_free, self.disk_total) {
                (Some(f), Some(t)) => format!("剩 {} / 共 {}", human_bytes(f), human_bytes(t)),
                _ => "读不到".into(),
            },
            self.registry_id,
            self.registry_prefix,
            self.hunter_tag
        ));
        // ── Hunter 自己那台虚拟机的 DNS（I10）──
        if let Some(d) = &self.vm_dns {
            s.push_str(&format!(
                "\n## Hunter 自己那台虚拟机的 DNS\n{}\n",
                d.one_line()
            ));
            if !d.healthy() {
                s.push_str(
                    "这台虚拟机是启动器自己下载、自己创建的（colima profile hunter），\
                     它没有 DNS 就意味着**里面所有容器都解析不了域名**，\
                     健康检查过不去、也连不上模型网关。\
                     可以用动作 `fix_vm_dns` 修它 —— 那个动作只写这台虚拟机里的 /etc/resolv.conf，\
                     **不许、也做不到**去改用户这台电脑的 DNS / hosts / 代理 / 防火墙。\n",
                );
            }
        }
        for c in &self.commands {
            s.push_str(&format!(
                "\n## 命令原始输出：{}（退出码 {}）\n{}\n",
                c.command,
                c.status
                    .map(|x| x.to_string())
                    .unwrap_or_else(|| "起不来".into()),
                c.output
            ));
        }
        s.push_str(&format!("\n## 启动器日志最后 {} 行\n", self.log_tail.len()));
        for l in &self.log_tail {
            s.push_str(l);
            s.push('\n');
        }
        s
    }
}

fn tri(b: Option<bool>) -> &'static str {
    match b {
        Some(true) => "是",
        Some(false) => "否",
        None => "读不到",
    }
}

// ── 容器运行时装没装、起没起 ──────────────────────────────────────────────

/// 「装了但没起来」是最常见也最好修的一种（I4 测试场景 2）。
/// 这里分别判「装没装」与「进程起没起」，**两者都读不到就如实写读不到**。
pub fn runtime_apps() -> Vec<AppPresence> {
    let mut out = Vec::new();

    if cfg!(target_os = "macos") {
        for (id, label, app_dir, bin_hint) in [
            (
                "orbstack",
                "OrbStack",
                "/Applications/OrbStack.app",
                "OrbStack",
            ),
            (
                "docker-desktop",
                "Docker Desktop",
                "/Applications/Docker.app",
                "Docker Desktop",
            ),
        ] {
            let home_app = crate::paths::home().join("Applications").join(
                Path::new(app_dir)
                    .file_name()
                    .map(|x| x.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            );
            let installed = Path::new(app_dir).exists() || home_app.exists();
            out.push(AppPresence {
                id: id.into(),
                label: label.into(),
                installed: Some(installed),
                running: process_running(bin_hint),
                evidence: format!("看 {app_dir} 在不在 + pgrep {bin_hint}"),
            });
        }
    }

    // 内置运行时（I9）。**必须排在「用户自己的 Colima」前面，而且分成两条**：
    //
    // 0.1.8 在用户 Mac 上把 `~/.hunter/runtime/bin/colima`（我们自己下的那一份）
    // 当成了「用户装的 Colima」，于是规则层给出「Colima 装着但没在运行，把它启动
    // 起来」，执行的是一条裸 `colima start` —— 用默认的 `~/.colima` 在他家目录里
    // 新建了一台虚拟机。**我们自己的东西不能出现在「用户装了什么」这张表里。**
    let bs = crate::runtime::effective::builtin_state();
    if bs.present() {
        out.push(AppPresence {
            id: "builtin".into(),
            label: "Hunter 内置运行时".into(),
            installed: Some(matches!(
                bs,
                crate::runtime::effective::BuiltinState::Stopped
                    | crate::runtime::effective::BuiltinState::Running
            )),
            running: Some(bs.running()),
            evidence: format!(
                "看 ~/.hunter/runtime 下的四件工具 + {} 这个 socket 连不连得上（{}）",
                crate::redact::mask_home(&crate::runtime::builtin::socket_path().to_string_lossy()),
                bs.cn()
            ),
        });
    }

    // 用户**自己**的 Colima：只认 `~/.colima` 里本来就有的 profile。
    //
    // 判定全靠读目录与 socket，**一条 `colima status` 都不跑** ——
    // 跑它会用默认的 `COLIMA_HOME`，而 0.1.8 正是这样把 `~/.colima` 建起来的。
    let profiles = crate::runtime::effective::user_colima_profiles();
    if !profiles.is_empty() {
        let running = profiles.iter().any(|p| {
            crate::paths::home()
                .join(".colima")
                .join(p)
                .join("docker.sock")
                .exists()
        });
        out.push(AppPresence {
            id: "colima".into(),
            label: "你自己的 Colima".into(),
            installed: Some(true),
            running: Some(running),
            evidence: format!(
                "读 ~/.colima 下的 profile 目录（{}）+ 看它们的 docker.sock 在不在（不跑 colima 命令，免得碰你的 ~/.colima）",
                profiles.join("、")
            ),
        });
    }

    if cfg!(target_os = "linux") {
        let sc = which::resolve("systemctl").resolved;
        let (installed, running, evidence) = match sc {
            None => (None, None, "这台机器上没有 systemctl".to_string()),
            Some(sc) => {
                let active = crate::proc::run_timeout(
                    &sc,
                    &["is-active", "docker"],
                    Duration::from_secs(15),
                )
                .ok();
                let enabled = crate::proc::run_timeout(
                    &sc,
                    &["list-unit-files", "docker.service"],
                    Duration::from_secs(15),
                )
                .ok();
                (
                    enabled.map(|r| r.ok() && r.stdout.contains("docker.service")),
                    active.map(|r| r.stdout.trim() == "active"),
                    "systemctl is-active docker".to_string(),
                )
            }
        };
        out.push(AppPresence {
            id: "systemd".into(),
            label: "systemd 的 docker 服务".into(),
            installed,
            running,
            evidence,
        });
    }

    out
}

/// 本机上别的 Hunter 安装，一行一条人话（I5 动作表 v2 的 `list_other_hunter_installs`）。
pub fn other_installs_text() -> String {
    let pubs = crate::ports::docker_published();
    let v = crate::ports::other_hunter_installs(&pubs);
    if v.is_empty() {
        return "这台机器上没有别的 Hunter 在跑。".to_string();
    }
    let mut s = format!("这台机器上还有 {} 套 Hunter 在跑：\n", v.len());
    for i in &v {
        s.push_str(&format!(
            "compose 项目 {} · {} 个容器（{}）· 占着端口 {}\n",
            i.project,
            i.containers.len(),
            i.containers.join("、"),
            i.ports
                .iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>()
                .join("、")
        ));
    }
    s.push_str(
        "它们和这次要装的这一套是两回事，启动器不会动它们；新装的会换一组空闲端口，两套并存。\n",
    );
    s
}

pub fn runtime_apps_text() -> String {
    let apps = runtime_apps();
    if apps.is_empty() {
        return "这个平台上没有可以检查的容器运行时应用。".to_string();
    }
    apps.iter()
        .map(|a| {
            format!(
                "{}（{}）装了={} 在跑={} — {}",
                a.label,
                a.id,
                tri(a.installed),
                tri(a.running),
                a.evidence
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 进程起没起。用 `pgrep -f`；没有 pgrep 就返回 `None`（**不猜成「没起」**）。
fn process_running(name: &str) -> Option<bool> {
    let pgrep = which::resolve("pgrep").resolved?;
    let r = crate::proc::run_timeout(&pgrep, &["-f", name], Duration::from_secs(10)).ok()?;
    // pgrep 的约定：找到了 0，没找到 1，出错 ≥2
    match r.status {
        Some(0) => Some(true),
        Some(1) => Some(false),
        _ => None,
    }
}

// ── 磁盘 ──────────────────────────────────────────────────────────────────

/// 某个路径所在分区的 (剩余, 总量)。读不到就是 `None` —— **不编数字**。
///
/// 走 `df -k` 而不是 `statvfs`：这个仓库里一行 unsafe 都没有，
/// 为一个「顺带提一句盘还剩多少」的信息引入 libc 与 unsafe 不划算。
/// `df` 在 `/bin` 与 `/usr/bin`，正好在 macOS GUI 程序的默认 PATH 里。
/// Windows 上没有 df，如实返回读不到。
pub fn disk_free(path: &Path) -> Option<(u64, u64)> {
    if cfg!(windows) {
        return None;
    }
    // 目录还没建出来时往上找一层存在的
    let mut p = path.to_path_buf();
    for _ in 0..4 {
        if p.exists() {
            break;
        }
        match p.parent() {
            Some(par) => p = par.to_path_buf(),
            None => break,
        }
    }
    let df = which::resolve("df").resolved?;
    let r = crate::proc::run_timeout(&df, &["-k", &p.to_string_lossy()], Duration::from_secs(15))
        .ok()?;
    if !r.ok() {
        return None;
    }
    parse_df_k(&r.stdout)
}

/// `df -k` 的第二行：`文件系统 1K-块 已用 可用 已用% 挂载点`。
/// macOS 的 BSD df 多两列（iused / ifree），但**总量在第 2 个数字、可用在第 4 个**，
/// 这一点两边一致。认不出来就返回 None，不猜。
fn parse_df_k(stdout: &str) -> Option<(u64, u64)> {
    let line = stdout.lines().nth(1)?;
    let nums: Vec<u64> = line
        .split_whitespace()
        .filter_map(|x| x.parse::<u64>().ok())
        .collect();
    if nums.len() < 3 {
        return None;
    }
    let total = nums[0] * 1024;
    let avail = nums[2] * 1024;
    Some((avail, total))
}

pub fn human_bytes(n: u64) -> String {
    const U: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i + 1 < U.len() {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", U[i])
    }
}

/// 系统版本。读不到就是 `None`。
fn os_version() -> Option<String> {
    if cfg!(target_os = "macos") {
        let sw = which::resolve("sw_vers").resolved?;
        let r =
            crate::proc::run_timeout(&sw, &["-productVersion"], Duration::from_secs(10)).ok()?;
        return Some(r.stdout.trim().to_string()).filter(|s| !s.is_empty());
    }
    if cfg!(target_os = "linux") {
        let s = std::fs::read_to_string("/etc/os-release").ok()?;
        return s
            .lines()
            .find_map(|l| l.strip_prefix("PRETTY_NAME="))
            .map(|v| v.trim_matches('"').to_string());
    }
    None
}

// ── 出口闸 ────────────────────────────────────────────────────────────────

/// 送出去之前的最后一道闸：报文里出现 key 的形状就**不发**。
///
/// 上面每一段都已经过了 `clean`，这里再整份扫一遍 —— 挡的是
/// 「日后谁往 Report 里加了一个字段却忘了脱敏」。
pub fn assert_no_secret(text: &str) -> Option<String> {
    for (i, line) in text.lines().enumerate() {
        for pat in ["hunt_tools_", "sk-", "hk_"] {
            if let Some(at) = line.find(pat) {
                let tail = &line[at + pat.len()..];
                let n = tail
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                    .count();
                if n >= 8 {
                    return Some(format!("第 {} 行像是带着一把 key（{pat}…）", i + 1));
                }
            }
        }
        let lower = line.to_ascii_lowercase();
        if let Some(at) = lower.find("bearer ") {
            let tail = &line[at + 7..];
            let n = tail
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_' || *c == '.')
                .count();
            if n >= 8 {
                return Some(format!("第 {} 行带着 Authorization 头的原文", i + 1));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAKE: &str = "hunt_tools_q6sKaaaaaaaaaaaaaaaaaaaaaaaaQMo2";

    /// **0.1.9 那份诊断的回归**（I11 · U4）：端口一节必须分三类，
    /// 被 Hunter 自己占着的那几个绝不能再印成「空闲」。
    #[test]
    fn 端口一节分三类且自己占着的不写空闲() {
        let mut r = blank();
        r.ports = vec![
            PortState {
                service: "web".into(),
                port: 3100,
                free: false,
                ours: true,
                kind: crate::ports::Kind::Mine,
                occupied_by:
                    "Docker 容器 hunter-web-1（compose 项目 hunter · 127.0.0.1:3100->3000/tcp）"
                        .into(),
            },
            PortState {
                service: "api".into(),
                port: 8100,
                free: false,
                ours: false,
                kind: crate::ports::Kind::Other,
                occupied_by: "进程 nginx（pid 42）".into(),
            },
            PortState {
                service: "redis".into(),
                port: 6479,
                free: true,
                ours: false,
                kind: crate::ports::Kind::Free,
                occupied_by: String::new(),
            },
        ];
        let s = r.ports_section();
        assert!(s.contains("web 3100 被 Hunter 自己占用"), "{s}");
        assert!(s.contains("api 8100 被其他程序占用 · 进程 nginx"), "{s}");
        assert!(s.contains("redis 6479 空闲"), "{s}");
        // 「自己占着」那一行里一个「空闲」都不能有 —— 这正是 0.1.9 那次错的地方
        let web_line = s.lines().find(|l| l.starts_with("web ")).unwrap();
        assert!(!web_line.contains("空闲"), "{web_line}");
    }

    #[test]
    fn 出口闸认得出没打码的_key() {
        assert!(assert_no_secret(&format!("LLM_API_KEY={FAKE}")).is_some());
        assert!(assert_no_secret("Authorization: Bearer abcdefghijklmnop").is_some());
        // 打过码的要放行，否则整份报文永远发不出去
        assert!(assert_no_secret("LLM_API_KEY=hunt_tools_****").is_none());
        assert!(assert_no_secret("Authorization: Bearer ****").is_none());
        assert!(assert_no_secret("一切正常，没有秘密").is_none());
    }

    /// I4 的硬要求：送去网关的报文里 grep 不到 key。
    #[test]
    fn 报文里_grep_不到_key() {
        crate::redact::register_secret(FAKE);
        let r = Report {
            launcher_version: "0.1.4".into(),
            os: "macos".into(),
            arch: "aarch64".into(),
            os_version: Some("15.1".into()),
            effective_runtime: "当前生效运行时：测试（测试）".into(),
            error_code: Some("E_DOCKER_MISSING".into()),
            // 和 `collect` 一样走 clean —— 这里测的是「采集路径过了脱敏之后报文是干净的」
            error_message: Some(clean(&format!("用 key {FAKE} 校验过了"))),
            stage: Some("docker".into()),
            docker_installed: false,
            daemon_running: false,
            docker_runtime: "Unknown".into(),
            docker_client: None,
            docker_server: None,
            compose_version: None,
            compose_mode: None,
            docker_path: None,
            probe_lines: vec![clean(
                "· [已知位置] /Users/zhangsan/.orbstack/bin/docker — 没有这个文件",
            )],
            env_path: "/usr/bin:/bin".into(),
            apps: Vec::new(),
            ports: Vec::new(),
            disk_free: None,
            disk_total: None,
            registry_id: "ghcr".into(),
            registry_prefix: "ghcr.io/agentpit-io".into(),
            hunter_tag: "1.2.0".into(),
            commands: vec![CmdOut {
                command: "docker version".into(),
                status: None,
                output: clean(&format!("Authorization: Bearer {FAKE}")),
            }],
            vm_dns: None,
            log_tail: vec![clean(&format!("校验 key {FAKE} 通过"))],
        };
        let p = r.to_prompt();
        assert!(!p.contains("q6sK"), "报文里有 key 的正文：{p}");
        assert!(!p.contains("zhangsan"), "报文里有用户名：{p}");
        assert!(
            assert_no_secret(&p).is_none(),
            "出口闸应当放行这份已经打过码的报文：{:?}",
            assert_no_secret(&p)
        );
        // 该留的证据还在
        assert!(p.contains("E_DOCKER_MISSING"), "{p}");
        assert!(
            p.contains("/usr/bin:/bin"),
            "PATH 是关键证据，必须留着：{p}"
        );
        assert!(p.contains("<用户目录>/.orbstack/bin/docker"), "{p}");
    }

    #[test]
    fn 读不到就写读不到不编数字() {
        assert_eq!(tri(None), "读不到");
        assert_eq!(tri(Some(true)), "是");
        // 磁盘读不到时 to_prompt 里写「读不到」而不是 0
        let mut r = blank();
        r.disk_free = None;
        assert!(r.to_prompt().contains("磁盘: 读不到"), "{}", r.to_prompt());
    }

    #[test]
    fn df_的两种输出都认得() {
        // Linux（coreutils）实测形状
        let gnu = "Filesystem     1K-blocks     Used Available Use% Mounted on\n/dev/root       60285164 48551452  11717328  81% /\n";
        assert_eq!(parse_df_k(gnu), Some((11717328 * 1024, 60285164 * 1024)));
        // macOS（BSD df -k）形状：多了 iused / ifree / %iused 三列，
        // 但「总量在第 1 个数字、可用在第 3 个」是一样的
        let bsd = "Filesystem 1024-blocks      Used Available Capacity iused      ifree %iused  Mounted on\n/dev/disk3s1s1 971350180 9876543 123456789    8%  501234 4294466045    0%   /\n";
        assert_eq!(parse_df_k(bsd), Some((123456789 * 1024, 971350180 * 1024)));
        // 认不出来就是 None，不猜一个数出来（红线 1）
        assert_eq!(parse_df_k(""), None);
        assert_eq!(
            parse_df_k("Filesystem\ndf: /x: No such file or directory\n"),
            None
        );
    }

    #[test]
    fn 字节可读化() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1536), "1.5 KB");
        assert_eq!(human_bytes(3 * 1024 * 1024 * 1024), "3.0 GB");
    }

    /// 这台机器上能跑的真实采集：只确认它**不 panic**、能产出非空报文、且过得了出口闸。
    #[test]
    fn 真采一份也要过出口闸() {
        let r = collect(Some("E_DOCKER_MISSING"), Some("测试"), Some("docker"));
        let p = r.to_prompt();
        assert!(!p.is_empty());
        assert_eq!(assert_no_secret(&p), None, "真实采集出来的报文带了秘密");
    }

    fn blank() -> Report {
        Report {
            launcher_version: "0".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            os_version: None,
            effective_runtime: "当前生效运行时：测试（测试）".into(),
            error_code: None,
            error_message: None,
            stage: None,
            docker_installed: false,
            daemon_running: false,
            docker_runtime: "Unknown".into(),
            docker_client: None,
            docker_server: None,
            compose_version: None,
            compose_mode: None,
            docker_path: None,
            probe_lines: Vec::new(),
            env_path: String::new(),
            apps: Vec::new(),
            ports: Vec::new(),
            disk_free: None,
            disk_total: None,
            registry_id: String::new(),
            registry_prefix: String::new(),
            hunter_tag: String::new(),
            commands: Vec::new(),
            vm_dns: None,
            log_tail: Vec::new(),
        }
    }
}
