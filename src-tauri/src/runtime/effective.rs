//! 「当前生效运行时」（I9 的 P0-1 / P0-4）。
//!
//! ## 为什么要有这个模块
//!
//! 0.1.8 在用户 Mac 上第一次真机跑（2026-09-22 14:48，证据在
//! `plan/mac-0.1.8-20260922/`），机器上留着 0.1.7 那次**没装成功的内置运行时残骸**：
//! `~/.hunter/runtime/bin/docker` 与 `colima` 都在，但 `hunter` 这台虚拟机从没起来过。
//! 于是：
//!
//! ```text
//! 14:49:13  docker 命令在（那是我们自己的）、连不上后台服务
//!           → 判成 E_DAEMON_DOWN
//!           → 规则 daemon-down-app-installed：「Colima 装着但没在运行，把它启动起来」
//! 14:49:15  执行 ~/.hunter/runtime/bin/colima start   ← 不带 profile、不带 COLIMA_HOME
//!           → 60 秒超时 →「没能自动装好」
//! ```
//!
//! 两处都错在**同一件事上**：没有人问过「现在到底该用哪个运行时」。
//!
//! * 我们自己下的那个 `docker` 被当成了「用户装的 Docker」，于是 0.1.8 的
//!   「没有 Docker → 内置运行时」这条主线根本没走到；
//! * 我们自己下的那个 `colima` 被当成了「用户装的 Colima」，于是启动器用**默认**的
//!   `COLIMA_HOME=~/.colima` 在用户家目录里新建并起了一台与 Hunter 无关的
//!   `default` 虚拟机（2 核 2 GB，1.4 GB 磁盘）；
//! * 同一段时间里所有 `docker ps` 都打在 `/var/run/docker.sock` 上（内置运行时的
//!   socket 在 `~/.hunter/runtime/colima/hunter/docker.sock`），端口冲突检测整条被跳过。
//!
//! ## 这个模块是什么
//!
//! 一句话：**「这台机器上现在生效的容器运行时是谁」只在这里判一次**，
//! 其余所有地方（子进程环境、端口探测、规则层、动作表、诊断报文）都来这里取，
//! 不再各自去猜。
//!
//! 判定顺序按 I9 任务书 P0-1 写死：
//!
//! | 顺序 | 问什么 | 判据（**全是文件系统与 socket，不跑任何子进程**） |
//! |---|---|---|
//! | ① | 内置运行时的虚拟机在跑吗 | `~/.hunter/runtime/colima/hunter/docker.sock` 连得上 |
//! | ② | 用户自己的 Docker 在跑吗 | 已知 socket 逐个 `connect`（OrbStack / Docker Desktop / 系统 / 用户自己的 colima） |
//! | ③ | 内置运行时装了（哪怕只装了一半）吗 | `runtime/bin` 下那几个文件 |
//! | ④ | 用户自己的 Docker 装了没起吗 | 已知 socket 路径存在但连不上 / 应用目录在 |
//!
//! ①②③④ 都不成立才是「这台机器上什么都没有」。
//!
//! ## 为什么不跑子进程
//!
//! 两个理由，第二个是硬的：
//!
//! 1. 快 —— 这个函数会被 [`super::env::compute`] 调到，而那个函数在每次建
//!    `Command` 的路径上；
//! 2. **不然会无限递归**。`env::compute()` → `effective::current()` → `docker version`
//!    → `proc::base_command()` → `env::apply()` → `env::current()` → `compute()` → …
//!    I8 在 `netproxy` 与 `runtime::env` 之间就凑出过一模一样的环，
//!    CI 的 macOS runner 上直接 `stack overflow`（I8 报告 6.7）。
//!    所以这里一个 `Command` 都不建。
//!
//! 代价是「连得上 socket」不等于「`docker version` 一定成功」。这个代价是可接受的：
//! 判错的后果只是多走一遍 `docker version`（[`super::docker::detect`] 照跑不误），
//! 而判对的收益是上面那三个 P0。

use std::path::PathBuf;

/// 现在生效的是谁。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    /// 我们自己装在 `~/.hunter/runtime` 里的那一套（Colima profile `hunter`）
    Builtin,
    /// 用户自己装的（OrbStack / Docker Desktop / 系统 docker / 他自己的 colima）
    User,
    /// 这台机器上一个都没有
    None,
}

/// 内置运行时现在是什么状态。**「装了一半」是一个真实存在的中间态**
/// （0.1.7 装过一次的机器上工具齐了、虚拟机镜像还没下），不能和「没装」合并。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase", tag = "state", content = "detail")]
pub enum BuiltinState {
    /// 一个文件都没有
    Absent,
    /// 有一部分文件，但工具还不齐（列出还缺哪几个组件）
    Partial(Vec<String>),
    /// 工具齐了，虚拟机没起来（**用户 Mac 上 0.1.8 那次就是这一档**）
    Stopped,
    /// 虚拟机在跑，socket 连得上
    Running,
}

impl BuiltinState {
    /// 装了点东西吗（哪怕只有半截）。
    pub fn present(&self) -> bool {
        !matches!(self, BuiltinState::Absent)
    }
    pub fn running(&self) -> bool {
        matches!(self, BuiltinState::Running)
    }
    pub fn cn(&self) -> String {
        match self {
            BuiltinState::Absent => "没装".to_string(),
            BuiltinState::Partial(miss) => format!("装了一半（还缺 {}）", miss.join("、")),
            BuiltinState::Stopped => "装好了，虚拟机没起来".to_string(),
            BuiltinState::Running => "虚拟机在跑".to_string(),
        }
    }
}

/// 一次判定的完整结果。诊断报文、事件流、设置页都直接用它。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Effective {
    pub kind: Kind,
    /// 这个运行时**现在能用**吗（socket 连得上）
    pub running: bool,
    /// 给人看的名字
    pub label: String,
    /// 判定依据，一句人话。**读到什么写什么**，不猜
    pub why: String,
    /// 要给子进程的 `DOCKER_HOST`；`None` = 原样继承调用方的
    pub docker_host: Option<String>,
    /// 内置运行时的状态（不管现在生效的是谁，这一条都要如实给出来）
    pub builtin: BuiltinState,
    /// 用户自己那一套能连上的 socket（连不上就是 `None`）
    pub user_socket: Option<String>,
}

impl Effective {
    /// 内置运行时是主角、但虚拟机没起来 —— **这就是 I9 要认出来的那个现场**。
    ///
    /// 认出来之后走的是 0.1.8 的内置主线（校验清单 → 本地 `--disk-image` 起
    /// profile `hunter`），而不是「Colima 装着没运行，把它启动起来」。
    pub fn builtin_down(&self) -> bool {
        self.kind == Kind::Builtin && !self.running
    }

    /// 现在这台机器上有没有能用的 docker 后台服务。
    pub fn usable(&self) -> bool {
        self.running
    }

    /// 一行人话（日志、诊断报文）。
    pub fn one_line(&self) -> String {
        let host = match &self.docker_host {
            Some(h) => format!("，DOCKER_HOST={}", crate::redact::mask_home(h)),
            None => String::new(),
        };
        format!("当前生效运行时：{}（{}）{host}", self.label, self.why)
    }
}

// ── 内置运行时 ────────────────────────────────────────────────────────────

/// 内置运行时的四件工具分别在哪、叫什么。与 [`super::builtin`] 同一份事实。
fn builtin_tools() -> Vec<(&'static str, PathBuf)> {
    let bin = crate::paths::runtime_bin();
    vec![
        ("docker", bin.join(exe("docker"))),
        ("colima", bin.join(exe("colima"))),
        (
            "limactl",
            crate::paths::runtime_dist()
                .join("lima")
                .join("bin")
                .join(exe("limactl")),
        ),
        (
            "docker compose 插件",
            crate::paths::runtime_docker_config()
                .join("cli-plugins")
                .join(exe("docker-compose")),
        ),
    ]
}

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// 内置运行时现在是什么状态。**只看文件系统与 socket。**
pub fn builtin_state() -> BuiltinState {
    let tools = builtin_tools();
    let missing: Vec<String> = tools
        .iter()
        .filter(|(_, p)| !p.is_file())
        .map(|(n, _)| (*n).to_string())
        .collect();
    // **一件工具都没有就是「没装」** —— 哪怕 `runtime/` 目录在。
    //
    // 原来这里还要求 `!runtime_dir.is_dir()`，结果测试机上真实的 `~/.hunter/runtime`
    // 里只剩一个空的 `cache/`（上一次装到一半留下的），却被报成「装了一半」：
    // 停掉 docker 之后，本该走 `daemon-down-app-installed` 的机器会去走内置主线，
    // 而 Linux 上那条路根本走不通。**「有一个空目录」不是「装了一半」。**
    if missing.len() == tools.len() {
        return BuiltinState::Absent;
    }
    if !missing.is_empty() {
        return BuiltinState::Partial(missing);
    }
    if connectable(&super::builtin::socket_path()) {
        BuiltinState::Running
    } else {
        BuiltinState::Stopped
    }
}

// ── 用户自己那一套 ────────────────────────────────────────────────────────

/// 用户自己的 docker socket 可能在的位置。**顺序就是优先级。**
///
/// 一个都不在 `~/.hunter` 里 —— 这张表里的每一条都属于用户，我们只读不写。
fn user_socket_candidates() -> Vec<(String, PathBuf)> {
    let home = crate::paths::home();
    let mut v: Vec<(String, PathBuf)> = Vec::new();
    // 用户自己在环境里设过 DOCKER_HOST 的话，那就是他的选择 —— 排最前
    if let Ok(h) = std::env::var("DOCKER_HOST") {
        if let Some(p) = h.strip_prefix("unix://") {
            let p = PathBuf::from(p);
            // 指到我们自己那份的不算「用户的」
            if !p.starts_with(crate::paths::runtime_dir()) {
                v.push(("你设的 DOCKER_HOST".to_string(), p));
            }
        }
    }
    v.push((
        "系统 docker".to_string(),
        PathBuf::from("/var/run/docker.sock"),
    ));
    v.push((
        "Docker Desktop".to_string(),
        home.join(".docker/run/docker.sock"),
    ));
    v.push((
        "Docker Desktop".to_string(),
        home.join(".docker/desktop/docker.sock"),
    ));
    v.push((
        "OrbStack".to_string(),
        home.join(".orbstack/run/docker.sock"),
    ));
    v.push(("Rancher Desktop".to_string(), home.join(".rd/docker.sock")));
    // 用户自己的 colima（每个 profile 一个 socket）
    for p in user_colima_profiles() {
        v.push((
            format!("你自己的 Colima（profile {p}）"),
            home.join(".colima").join(&p).join("docker.sock"),
        ));
    }
    v
}

/// 用户**自己**的 `~/.colima` 里原本就有的 profile 名。
///
/// 只读目录名，不跑 `colima list` —— 跑它会用默认的 `COLIMA_HOME`，
/// 而 0.1.8 在用户 Mac 上正是这样把 `~/.colima` 建起来的（1.4 GB）。
/// 下划线开头的是 colima 自己的内部目录（`_lima` / `_store` / `_templates`），排除掉。
pub fn user_colima_profiles() -> Vec<String> {
    let dir = crate::paths::home().join(".colima");
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut v: Vec<String> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('_') && !n.starts_with('.'))
        .collect();
    v.sort();
    v
}

/// 一个 unix socket 连得上吗。**连得上 = 后台服务活着**，这是最直接的判据。
///
/// Windows 上 docker 走的是命名管道，这里连不了 —— 那边一律返回 false，
/// 由 [`super::docker::detect`] 那条常规路径去判（Windows 上没有内置运行时，
/// 这个模块的判定也就退化成「用户那一套」一条路）。
fn connectable(p: &std::path::Path) -> bool {
    #[cfg(unix)]
    {
        if !p.exists() {
            return false;
        }
        std::os::unix::net::UnixStream::connect(p).is_ok()
    }
    #[cfg(not(unix))]
    {
        let _ = p;
        false
    }
}

/// 用户自己那一套现在能用吗。能用就返回 (人话名字, socket 路径)。
fn user_running() -> Option<(String, PathBuf)> {
    if cfg!(windows) {
        // 命名管道连不了，这里如实返回「不知道」，交给 docker version 那条路
        return None;
    }
    user_socket_candidates()
        .into_iter()
        .find(|(_, p)| connectable(p))
}

/// 用户自己那一套装了没起吗（socket 文件在但连不上，或者 .app 在）。
fn user_installed() -> Option<String> {
    for (label, p) in user_socket_candidates() {
        if p.exists() {
            return Some(label);
        }
    }
    if cfg!(target_os = "macos") {
        for (label, dir) in [
            ("OrbStack", "/Applications/OrbStack.app"),
            ("Docker Desktop", "/Applications/Docker.app"),
        ] {
            if std::path::Path::new(dir).exists() {
                return Some(label.to_string());
            }
        }
    }
    if !user_colima_profiles().is_empty() {
        return Some("你自己的 Colima".to_string());
    }
    None
}

/// 测试专用：在当前 `HUNTER_HOME` 里造出内置运行时的四件工具。
///
/// **文件名走 [`exe`]**（Windows 上是 `docker.exe` / `colima.exe` …）。
/// I9 第一版没这么写，CI 的 windows runner 上一口气红了 9 条 ——
/// 造出来的假工具叫 `docker`，而代码找的是 `docker.exe`，
/// 于是每一条本该是 `Stopped` 的断言都得到了 `Partial`。
/// 这个助手 `pub(crate)`，三个测试模块共用一份，不许各抄各的。
#[cfg(test)]
pub(crate) fn make_fake_tools() {
    let bin = crate::paths::runtime_bin();
    std::fs::create_dir_all(&bin).unwrap();
    let lima = crate::paths::runtime_dist().join("lima").join("bin");
    std::fs::create_dir_all(&lima).unwrap();
    let cli = crate::paths::runtime_docker_config().join("cli-plugins");
    std::fs::create_dir_all(&cli).unwrap();
    for p in [
        bin.join(exe("docker")),
        bin.join(exe("colima")),
        lima.join(exe("limactl")),
        cli.join(exe("docker-compose")),
    ] {
        write_fake_exe(&p);
    }
}

/// 同上，但**只造 docker 一个** —— 用来造「装了一半」那个中间态。
#[cfg(test)]
pub(crate) fn make_fake_tools_partial() {
    let bin = crate::paths::runtime_bin();
    std::fs::create_dir_all(&bin).unwrap();
    write_fake_exe(&bin.join(exe("docker")));
}

#[cfg(test)]
pub(crate) fn write_fake_exe(p: &std::path::Path) {
    std::fs::write(p, b"#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

// ── 判定 ──────────────────────────────────────────────────────────────────

/// **这台机器上现在生效的运行时是谁。**
///
/// 不缓存：判定只做几次 `stat` 与一次 `connect`，微秒量级；而缓存下来的
/// 「虚拟机在跑吗」一旦过期，就会变成最难查的那一类 bug
/// （0.1.8 那次日志里 `DOCKER_HOST` 时有时无，根子就在缓存上）。
pub fn current() -> Effective {
    decide(builtin_state(), user_running(), user_installed())
}

/// 判定本身，**做成纯函数**：三个输入进去，一个结论出来。
///
/// 拆出来只有一个理由，但它很硬：**不然这条判定在任何一台装着 Docker 的机器上
/// 都测不了**。开发机与测试机都是 Linux + 常驻 docker，`user_running()` 永远有值，
/// 于是「内置运行时是主角」那几条分支在 CI 上一次都跑不到 —— 测试会绿，
/// 但绿的是「这台机器上 docker 在跑」，不是我们要验的那件事。
/// I9 之前两轮 P0 的教训都是同一句话：**把那台机器的差异写成一条会红的测试。**
pub fn decide(
    builtin: BuiltinState,
    user_running: Option<(String, PathBuf)>,
    user_installed: Option<String>,
) -> Effective {
    // ① 内置运行时在跑 —— 它就是主角，没有任何可商量的
    if builtin.running() {
        let sock = super::builtin::socket_path();
        return Effective {
            kind: Kind::Builtin,
            running: true,
            label: format!("内置运行时（Colima profile {}）", super::builtin::PROFILE),
            why: format!(
                "{} 连得上",
                crate::redact::mask_home(&sock.to_string_lossy())
            ),
            docker_host: Some(format!("unix://{}", sock.to_string_lossy())),
            builtin,
            user_socket: None,
        };
    }

    // ② 用户自己的 Docker 在跑 —— 一个字节都不用下，直接用他的
    if let Some((label, sock)) = user_running {
        return Effective {
            kind: Kind::User,
            running: true,
            label: label.clone(),
            why: format!(
                "{} 连得上",
                crate::redact::mask_home(&sock.to_string_lossy())
            ),
            // 用户那一套走它自己的默认 socket，**不要替他设 DOCKER_HOST**
            docker_host: None,
            builtin,
            user_socket: Some(sock.to_string_lossy().into_owned()),
        };
    }

    // ③ 内置运行时装了（哪怕只装了一半）但没起来 —— **I9 的主角现场**。
    //    这里 `DOCKER_HOST` 照样指向我们自己那个还不存在的 socket：
    //    让 docker 去连 `~/.hunter/runtime/colima/hunter/docker.sock` 然后如实报
    //    「连不上」，远好过让它悄悄打到 `/var/run/docker.sock` 上（P0-4）。
    if builtin.present() {
        let sock = super::builtin::socket_path();
        return Effective {
            kind: Kind::Builtin,
            running: false,
            label: format!("内置运行时（Colima profile {}）", super::builtin::PROFILE),
            why: format!(
                "{}，socket {} 连不上",
                builtin.cn(),
                crate::redact::mask_home(&sock.to_string_lossy())
            ),
            docker_host: Some(format!("unix://{}", sock.to_string_lossy())),
            builtin,
            user_socket: None,
        };
    }

    // ④ 用户装了但没起
    if let Some(label) = user_installed {
        return Effective {
            kind: Kind::User,
            running: false,
            label: label.clone(),
            why: "装着，但它的后台服务连不上".to_string(),
            docker_host: None,
            builtin,
            user_socket: None,
        };
    }

    Effective {
        kind: Kind::None,
        running: false,
        label: "没有容器运行时".to_string(),
        why: "内置运行时没装，也没找到你自己装的 Docker".to_string(),
        docker_host: None,
        builtin,
        user_socket: None,
    }
}

/// 内置运行时是当前生效的那个吗（不管起没起）。
///
/// 动作表与守卫靠这一条决定 `colima` 该怎么调：是我们自己那一套就必须
/// 带 `COLIMA_HOME` + `--profile hunter`，是用户的就只允许起他原有的 profile。
pub fn builtin_is_effective() -> bool {
    current().kind == Kind::Builtin
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Windows 上路径分隔符是反斜杠。断言只关心「指到哪儿」，不关心用哪个斜杠。
    fn slashes(s: &str) -> String {
        s.replace('\\', "/")
    }

    fn user(label: &str) -> Option<(String, PathBuf)> {
        Some((label.to_string(), PathBuf::from("/var/run/docker.sock")))
    }

    /// 干净机器（`HUNTER_HOME` 指向一个空临时目录）上，内置运行时必须是「没装」。
    #[test]
    fn 空目录上内置运行时是没装() {
        let _g = crate::paths::test_home("eff-absent");
        assert_eq!(builtin_state(), BuiltinState::Absent);
    }

    /// **`runtime/` 目录在、但一件工具都没有 —— 还是「没装」。**
    ///
    /// 测试机上真实的 `~/.hunter/runtime` 就是这样（只剩一个空的 `cache/`）。
    /// 判成「装了一半」的后果：停掉 docker 之后，本该走「你的 Docker 没开」的机器
    /// 会去走内置主线，而 Linux 上那条路根本走不通。
    #[test]
    fn 只有空的_runtime_目录不算装了一半() {
        let _g = crate::paths::test_home("eff-empty-runtime");
        std::fs::create_dir_all(crate::paths::runtime_cache()).unwrap();
        assert_eq!(builtin_state(), BuiltinState::Absent);
        assert!(!builtin_state().present());
    }

    /// **0.1.7 那种「装了一半」的机器**：工具缺几个 → Partial，不能报成 Absent，
    /// 更不能报成 Stopped（Stopped 会让上层直接去 `colima start`，而文件还没下齐）。
    #[test]
    fn 工具缺几个时是装了一半() {
        let _g = crate::paths::test_home("eff-partial");
        make_fake_tools_partial();
        let st = builtin_state();
        match &st {
            BuiltinState::Partial(miss) => {
                assert!(miss.iter().any(|m| m == "colima"), "{miss:?}");
                assert!(!miss.iter().any(|m| m == "docker"), "{miss:?}");
            }
            other => panic!("该是 Partial，实际 {other:?}"),
        }
        assert!(st.present());
        assert!(!st.running());
    }

    /// 四件工具都在、socket 连不上 → Stopped。
    /// **这就是用户 Mac 上 0.1.8 那次的现场。**
    #[test]
    fn 工具齐了虚拟机没起来时是_stopped() {
        let _g = crate::paths::test_home("eff-stopped");
        make_fake_tools();
        assert_eq!(builtin_state(), BuiltinState::Stopped);
    }

    /// **I9 的主角判定，逐条钉死。**
    ///
    /// 走纯函数，所以在**任何**机器上结果都一样 —— 开发机和测试机都常驻着
    /// 一个能连的 `/var/run/docker.sock`，走 `current()` 的话下面这几条
    /// 一条都跑不到。
    #[test]
    fn 判定顺序是内置在先用户在后() {
        let _g = crate::paths::test_home("eff-decide");

        // ① 内置在跑 —— 哪怕用户的 Docker 也在跑，主角还是内置那一套
        let e = decide(BuiltinState::Running, user("OrbStack"), None);
        assert_eq!(e.kind, Kind::Builtin);
        assert!(e.running && e.usable());
        assert!(!e.builtin_down());
        let host = slashes(&e.docker_host.expect("内置运行时必须给 DOCKER_HOST"));
        assert!(host.contains("runtime/colima/hunter/docker.sock"), "{host}");

        // ② 内置没起、用户的在跑 —— 用户的优先，而且**不替他设 DOCKER_HOST**
        let e = decide(BuiltinState::Stopped, user("OrbStack"), None);
        assert_eq!(e.kind, Kind::User);
        assert!(e.running);
        assert_eq!(e.docker_host, None, "用户那一套走他自己的默认 socket");
        assert_eq!(e.builtin, BuiltinState::Stopped, "内置的状态照样如实报出来");

        // ③ 内置装着没起、用户那边什么都没有 —— **这就是 0.1.8 在用户 Mac 上的现场**
        let e = decide(BuiltinState::Stopped, None, None);
        assert_eq!(e.kind, Kind::Builtin);
        assert!(e.builtin_down(), "{}", e.one_line());
        assert!(!e.usable());
        let host = slashes(&e.docker_host.expect("P0-4：这时也必须给 DOCKER_HOST"));
        assert!(
            host.contains("runtime/colima/hunter/docker.sock"),
            "绝不能让 docker 悄悄打到 /var/run/docker.sock：{host}"
        );

        // ③′ 装了一半同样走内置主线（补齐 → 起）
        let e = decide(BuiltinState::Partial(vec!["colima".into()]), None, None);
        assert_eq!(e.kind, Kind::Builtin);
        assert!(e.builtin_down());
        assert!(e.why.contains("还缺 colima"), "{}", e.why);

        // ④ 内置没装、用户装了没起
        let e = decide(BuiltinState::Absent, None, Some("OrbStack".into()));
        assert_eq!(e.kind, Kind::User);
        assert!(!e.running);
        assert_eq!(e.docker_host, None);

        // ⑤ 什么都没有
        let e = decide(BuiltinState::Absent, None, None);
        assert_eq!(e.kind, Kind::None);
        assert!(!e.usable());
        assert_eq!(e.docker_host, None);
    }

    /// **P0-4 的那一条**：内置运行时是主角时，不管虚拟机起没起，
    /// `DOCKER_HOST` 都指向我们自己那个 socket。
    ///
    /// 0.1.8 的写法是「socket 文件存在才给」，于是虚拟机没起来的那 5 分钟里
    /// 每一条 `docker ps` 都打在 `/var/run/docker.sock` 上，端口冲突检测整条被跳过。
    #[test]
    fn 内置运行时是主角时_docker_host_一定指向我们自己的_socket() {
        let _g = crate::paths::test_home("eff-dockerhost");
        for st in [
            BuiltinState::Running,
            BuiltinState::Stopped,
            BuiltinState::Partial(vec!["colima".into()]),
        ] {
            let e = decide(st.clone(), None, None);
            assert_eq!(e.kind, Kind::Builtin, "{st:?}");
            let h = slashes(&e.docker_host.unwrap_or_default());
            assert!(h.starts_with("unix://"), "{st:?} → {h}");
            assert!(!h.contains("/var/run/docker.sock"), "{st:?} → {h}");
            assert!(h.contains("runtime/colima/hunter"), "{st:?} → {h}");
        }
    }

    /// `~/.colima` 里的内部目录（`_lima` / `_store` / `_templates`）不是 profile。
    #[test]
    fn 下划线开头的不算用户的_colima_profile() {
        let _g = crate::paths::test_home("eff-profiles");
        // 只验过滤规则本身，不碰真实家目录
        let names = ["_lima", "_store", "_templates", "default", "work"];
        let kept: Vec<&str> = names
            .iter()
            .copied()
            .filter(|n| !n.starts_with('_') && !n.starts_with('.'))
            .collect();
        assert_eq!(kept, vec!["default", "work"]);
    }

    /// 候选 socket 里**一个都不许**落在 `~/.hunter` 里 —— 那是我们自己的地盘，
    /// 混进「用户那一套」的候选表里会让判定顺序失去意义。
    #[test]
    fn 用户那一套的候选_socket_不许指向我们自己的目录() {
        let _g = crate::paths::test_home("eff-cands");
        let ours = crate::paths::runtime_dir();
        for (label, p) in user_socket_candidates() {
            assert!(
                !p.starts_with(&ours),
                "「{label}」指向了我们自己的目录：{}",
                p.display()
            );
        }
    }
}
