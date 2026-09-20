//! Docker 运行时检测与三平台安装引导（技术方案 §5.1、§6）。
//!
//! 判定逻辑严格按 M0 §5.4 的实测结论，**不能只看退出码**：
//!
//! | 场景 | `docker version --format json` |
//! |---|---|
//! | 一切正常 | 退出码 0，stdout 有 `Client` 与 `Server` |
//! | daemon 没起 | 退出码 **1**，stdout **仍有** `Client`，`Server` 为 `null` |
//! | 没装 docker | 进程根本起不来（ENOENT） |
//!
//! 也就是说「退出码 1」有两种完全不同的含义，靠 `Server == null` 区分。

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::err::{AppError, AppResult, Code};
use crate::proc;
use crate::runtime::which;

/// 最低版本（方案 §5.1）：Docker Engine 24 + Compose v2.20。
pub const MIN_ENGINE_MAJOR: u32 = 24;
pub const MIN_COMPOSE: (u32, u32) = (2, 20);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeKind {
    DockerEngine,
    DockerDesktop,
    Orbstack,
    Colima,
    Podman,
    Unknown,
}

impl RuntimeKind {
    fn label(self, version: Option<&str>) -> String {
        let name = match self {
            RuntimeKind::DockerEngine => "Docker Engine",
            RuntimeKind::DockerDesktop => "Docker Desktop",
            RuntimeKind::Orbstack => "OrbStack",
            RuntimeKind::Colima => "Colima",
            RuntimeKind::Podman => "Podman（实验支持）",
            RuntimeKind::Unknown => "未知运行时",
        };
        match version {
            Some(v) => format!("{name} {v}"),
            None => name.to_string(),
        }
    }
}

/// 安装引导的一条。`url` 有值时界面渲染成可点的链接。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GuideStep {
    pub text: String,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallGuide {
    /// 这段引导是给哪个平台的：windows / macos / linux
    pub platform: String,
    pub title: String,
    pub steps: Vec<GuideStep>,
    /// Docker Desktop 的商业授权提示（方案 §6 明确要求，对私募客户是必要的合规提示）
    pub license_note: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerInfo {
    pub installed: bool,
    pub daemon_running: bool,
    pub runtime: RuntimeKind,
    pub runtime_label: Option<String>,
    pub client_version: Option<String>,
    pub server_version: Option<String>,
    pub compose_version: Option<String>,
    pub arch: Option<String>,
    /// 实际用的 docker 可执行文件的**绝对路径**（I4 的 P0）。
    ///
    /// 界面上要把它显示出来：用户在 mac 上同时装过 Docker Desktop 与 OrbStack 是常事，
    /// 「启动器到底在用哪一个」不写出来谁也说不清。
    pub docker_path: Option<String>,
    /// 这条路径是从哪找到的：`配置` / `PATH` / `已知位置`
    pub docker_path_source: Option<String>,
    /// 软链指向哪里（`/usr/local/bin/docker` → OrbStack 的 xbin 就靠这一条看出来）
    pub docker_link_target: Option<String>,
    /// 按顺序探过的每一个位置与结果。没找到 docker 时界面会把它整份列出来
    pub docker_probe: Vec<crate::runtime::which::Step>,
    /// compose 是插件还是独立可执行文件，以及它在哪
    pub compose_mode: Option<String>,
    /// 仅 Windows 有意义：WSL2 是否可用。其它平台为 `null`
    pub wsl: Option<bool>,
    pub meets_minimum: bool,
    /// 版本过低 / 没装 / daemon 没起时，具体是哪一条不满足
    pub problem: Option<String>,
    /// 没装或版本过低时给的安装引导；一切正常时为 `null`
    pub install_guide: Option<InstallGuide>,
}

impl DockerInfo {
    /// 检测结果能不能继续往下走。
    pub fn ready(&self) -> bool {
        self.installed && self.daemon_running && self.meets_minimum
    }

    /// 检测结果对应的错误码；就绪时为 `None`。
    pub fn error_code(&self) -> Option<Code> {
        if !self.installed {
            Some(Code::DockerMissing)
        } else if self.wsl == Some(false) {
            Some(Code::WslMissing)
        } else if !self.daemon_running {
            Some(Code::DaemonDown)
        } else if !self.meets_minimum {
            Some(Code::DockerMissing)
        } else {
            None
        }
    }
}

/// `docker version --format json` 的响应，只取用得上的字段。
#[derive(Debug, Deserialize)]
struct VersionJson {
    #[serde(rename = "Client")]
    client: Option<Side>,
    #[serde(rename = "Server")]
    server: Option<Side>,
}

#[derive(Debug, Deserialize)]
struct Side {
    #[serde(rename = "Version")]
    version: Option<String>,
    #[serde(rename = "Arch")]
    arch: Option<String>,
    /// **podman 独有**的键（`"OsArch":"linux/amd64"`）。Docker 的客户端 JSON 里
    /// 对应的是分开的 `Os` + `Arch`，从来没有 `OsArch`。
    /// I3 实测的两份原文见 `podman_与_docker_的_client_json_形状不同` 那条测试。
    #[serde(rename = "OsArch")]
    os_arch: Option<String>,
    #[serde(rename = "Platform")]
    platform: Option<Platform>,
    #[serde(rename = "Components")]
    components: Option<Vec<Component>>,
}

#[derive(Debug, Deserialize)]
struct Platform {
    #[serde(rename = "Name")]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Component {
    #[serde(rename = "Name")]
    pub name: Option<String>,
}

impl DockerInfo {
    /// 全空的一份，`detect` 与测试共用。
    pub fn empty() -> Self {
        Self {
            installed: false,
            daemon_running: false,
            runtime: RuntimeKind::Unknown,
            runtime_label: None,
            client_version: None,
            server_version: None,
            compose_version: None,
            arch: None,
            docker_path: None,
            docker_path_source: None,
            docker_link_target: None,
            docker_probe: Vec::new(),
            compose_mode: None,
            wsl: None,
            meets_minimum: false,
            problem: None,
            install_guide: None,
        }
    }
}

/// 真去跑一遍检测。这是整个模块唯一会碰外部进程的函数。
///
/// **第一步是定位可执行文件，不是跑命令**（I4 的 P0）：macOS 的 GUI 程序拿不到
/// 用户 shell 的 PATH，基于 PATH 的查找在那里必然失败。定位逻辑与已知位置清单
/// 全在 [`crate::runtime::which`]，这里只消费它的结果。
pub fn detect() -> DockerInfo {
    let mut info = DockerInfo::empty();

    let probe = which::docker_probe();
    info.docker_path = probe.resolved.clone();
    info.docker_path_source = probe.source.clone();
    info.docker_link_target = probe.link_target.clone();
    info.docker_probe = probe.steps.clone();

    // 定位不到时仍然用裸名字跑一次：最坏情况与 I4 之前一致（由系统自己去 PATH 上找），
    // 不多出一种「启动器自己不肯试」的新失败模式。
    let bin = probe
        .resolved
        .clone()
        .unwrap_or_else(|| "docker".to_string());
    let ran = proc::run_timeout(
        &bin,
        &["version", "--format", "json"],
        Duration::from_secs(20),
    );

    match ran {
        // 进程起不来 = 没装（M0 §5.4）
        Err(_) => {
            info.problem = Some(missing_hint(&probe));
        }
        Ok(r) => {
            let parsed = parse_version(&r.stdout);
            match parsed {
                Some(v) => {
                    info.installed = v.client.is_some();
                    let ctx = context_name();
                    apply_version(&mut info, v, ctx.as_deref());
                    if !info.daemon_running {
                        // I3 实测（待办池 P1-5）：**podman 4.9.3 的 `version --format json`
                        // 是能解析的 Docker 形状的 JSON**（只有 Client 没有 Server），
                        // 于是上面那条「stdout 不是 JSON 才认 podman」的分支根本轮不到，
                        // 用户看到的是「Docker 客户端在，但连不上 daemon」加一整页装 Docker 的指引 ——
                        // 而这台机器上 podman 好好的，只是没有 docker 那个 socket。
                        //
                        // 纯文本的 `docker --version` 一句话就能分清（podman 打的是
                        // `podman version 4.9.3`），而且**只在这条失败路径上多跑一个进程**。
                        if matches!(info.runtime, RuntimeKind::Unknown) {
                            if let Some(text) = plain_version_text() {
                                info.runtime = classify_plain(&text);
                            }
                        }
                        info.problem = Some(if matches!(info.runtime, RuntimeKind::Podman) {
                            podman_hint(info.client_version.as_deref())
                        } else {
                            daemon_hint(&r.stderr)
                        });
                    }
                }
                None => {
                    // stdout 不是 JSON：可能是 podman 的 docker 兼容层，也可能是别的什么。
                    // 不猜，如实报出来。
                    if r.stdout.to_lowercase().contains("podman")
                        || r.stderr.to_lowercase().contains("podman")
                    {
                        info.installed = true;
                        info.runtime = RuntimeKind::Podman;
                        info.problem = Some(
                            "检测到 Podman 的 docker 兼容层。Podman 是实验支持，没有实测通过。"
                                .to_string(),
                        );
                    } else {
                        info.problem = Some(format!(
                            "docker version 的输出看不懂（退出码 {:?}）：{}",
                            r.status,
                            r.err_line()
                        ));
                    }
                }
            }
        }
    }

    if info.daemon_running {
        let c = which::compose_info();
        info.compose_version = c.version.clone();
        info.compose_mode = Some(c.label());
    }
    if cfg!(target_os = "windows") {
        info.wsl = Some(wsl_available());
    }

    evaluate_minimum(&mut info);
    info.runtime_label = if info.installed {
        Some(
            info.runtime.label(
                info.server_version
                    .as_deref()
                    .or(info.client_version.as_deref()),
            ),
        )
    } else {
        None
    };
    if !info.ready() {
        info.install_guide = Some(install_guide());
    }
    info
}

fn parse_version(stdout: &str) -> Option<VersionJson> {
    serde_json::from_str::<VersionJson>(stdout.trim()).ok()
}

fn apply_version(info: &mut DockerInfo, v: VersionJson, context: Option<&str>) {
    if let Some(c) = &v.client {
        info.client_version = c.version.clone();
    }
    match &v.server {
        Some(s) => {
            info.daemon_running = true;
            info.server_version = s.version.clone();
            info.arch = s.arch.clone();
            info.runtime = classify(
                s.platform.as_ref().and_then(|p| p.name.as_deref()),
                s.components.as_ref(),
                context,
            );
        }
        None => {
            info.daemon_running = false;
            // daemon 没起时也能从客户端侧与 context 名猜出运行时，引导文案会更准
            info.runtime = classify(
                v.client
                    .as_ref()
                    .and_then(|c| c.platform.as_ref())
                    .and_then(|p| p.name.as_deref()),
                None,
                context,
            );
            // 上面那几个来源对 podman 全是空的（它的 Client 里没有 Platform）。
            // `OsArch` 是 podman 独有的键，**不用多跑一个进程**就能分出来（I3 实测）。
            if matches!(info.runtime, RuntimeKind::Unknown)
                && v.client.as_ref().is_some_and(|c| c.os_arch.is_some())
            {
                info.runtime = RuntimeKind::Podman;
            }
        }
    }
}

/// `docker version`（**不带 `--format`**）的头几行纯文本。只在「JSON 里没有 Server」
/// 那条路上跑一次。
///
/// 为什么是这一条命令：
/// * `version --format json` 认不出 podman —— 那份 JSON 里一个 `podman` 字样都没有；
/// * `docker --version`（两道杠）**也认不出** —— podman 用 `argv[0]` 当程序名，
///   通过一个叫 `docker` 的软链调用时它打的是 `docker version 4.9.3`（I3 实测，
///   第一版就栽在这里）；
/// * 而 `docker version` 的**第一行**是 `Client:       Podman Engine`，
///   和 argv[0] 无关，这才是靠得住的那一个。
fn plain_version_text() -> Option<String> {
    let r = proc::run_timeout(&which::docker_bin(), &["version"], Duration::from_secs(10)).ok()?;
    let head: String = r.stdout.lines().take(3).collect::<Vec<_>>().join(" ");
    if head.trim().is_empty() {
        None
    } else {
        Some(head)
    }
}

/// 从 `docker version` 的头几行认运行时。认不出来就 Unknown —— **不猜成 Docker**。
pub fn classify_plain(text: &str) -> RuntimeKind {
    let l = text.to_lowercase();
    if l.contains("podman") {
        RuntimeKind::Podman
    } else if l.contains("orbstack") {
        RuntimeKind::Orbstack
    } else if l.contains("colima") {
        RuntimeKind::Colima
    } else if l.contains("docker engine") {
        RuntimeKind::DockerEngine
    } else {
        RuntimeKind::Unknown
    }
}

/// 认出 Podman 时该说的话。**不假装支持**（红线 1）：本项目没有在 Podman 上
/// 跑通过完整安装，只验过「认得出来」这一步，所以这里把已知和未知分开写。
fn podman_hint(client_version: Option<&str>) -> String {
    // 传进来的是**版本号**（"4.9.3"），不是整行。不补上「Podman」三个字的话
    // 界面上会写成「实际上是 4.9.3，不是 Docker」—— I3 第一次真机验证时就是这样，
    // 那句话读起来不知所云
    let v = match client_version.map(str::trim) {
        Some(v) if !v.is_empty() => format!("Podman {v}"),
        _ => "Podman".to_string(),
    };
    format!(
        "命令行里的 docker 实际上是 {v}，不是 Docker，而且现在连不上它的服务。\
         Podman 目前是**实验支持**：启动器认得出它，但整套安装从来没有在 Podman 上跑通过 —— \
         `docker compose` 在 Podman 上要另外装一个 compose provider（podman-compose 或 docker-compose），\
         没有它 Hunter 起不来。\
         最稳的做法是装 Docker Engine；一定要用 Podman 的话，请先确认 `docker compose version` 有输出，\
         再用 `podman system service` 把服务起起来。"
    )
}

/// 从 `Server.Platform.Name` / 组件名 / context 名判断运行时种类。
/// 三个来源都用上是因为它们各自都会漏：OrbStack 只在 context 名里露馅的情况是有的。
pub fn classify(
    platform_name: Option<&str>,
    components: Option<&Vec<Component>>,
    context: Option<&str>,
) -> RuntimeKind {
    let mut hay = String::new();
    if let Some(p) = platform_name {
        hay.push_str(&p.to_lowercase());
        hay.push(' ');
    }
    if let Some(cs) = components {
        for c in cs {
            if let Some(n) = &c.name {
                hay.push_str(&n.to_lowercase());
                hay.push(' ');
            }
        }
    }
    if let Some(c) = context {
        hay.push_str(&c.to_lowercase());
    }

    if hay.contains("orbstack") {
        RuntimeKind::Orbstack
    } else if hay.contains("colima") {
        RuntimeKind::Colima
    } else if hay.contains("podman") {
        RuntimeKind::Podman
    } else if hay.contains("desktop") {
        RuntimeKind::DockerDesktop
    } else if hay.contains("docker engine") || hay.contains("engine") {
        RuntimeKind::DockerEngine
    } else if hay.is_empty() {
        RuntimeKind::Unknown
    } else {
        RuntimeKind::DockerEngine
    }
}

fn context_name() -> Option<String> {
    proc::run_timeout(
        &which::docker_bin(),
        &["context", "show"],
        Duration::from_secs(10),
    )
    .ok()
    .filter(|r| r.ok())
    .map(|r| r.stdout.trim().to_string())
    .filter(|s| !s.is_empty())
}

/// 一个位置都没探到时说的话。**必须说清探过哪里** ——
/// 0.1.3 在用户 mac 上就只有一句「命令行里找不到 docker」，
/// 而他的 OrbStack 好好装着，这句话把他引到「是不是我没装」这条死路上去了。
fn missing_hint(probe: &which::Probe) -> String {
    let n = probe.steps.len();
    let head = format!("按顺序探了 {n} 个位置，都没有可执行的 docker。");
    if cfg!(target_os = "macos") {
        format!(
            "{head}\n\n             提醒一句：macOS 上「从访达或程序坞启动的程序」拿不到你终端里的 PATH\n             （GUI 程序默认只有 /usr/bin:/bin:/usr/sbin:/sbin），\n             所以「终端里 docker version 有输出」和「启动器找得到 docker」是两回事。\n             启动器已经把 /usr/local/bin、/opt/homebrew/bin、~/.orbstack/bin、\n             OrbStack 与 Docker Desktop 的 app 内目录都探过了（明细见下）。\n             如果你的 docker 装在别处，可以在 ~/.hunter/launcher.toml 的 [runtime] 段里写 docker_path 指给它。"
        )
    } else {
        format!(
            "{head}\n             装在别处的话，可以在 ~/.hunter/launcher.toml 的 [runtime] 段里写 docker_path 指给它。"
        )
    }
}

/// daemon 没起时，stderr 里的原话往往已经说清了原因（socket 路径、权限）。
/// 与其自己编一句，不如把它整理一下端给用户。
fn daemon_hint(stderr: &str) -> String {
    let s = stderr.trim();
    if s.contains("permission denied") {
        return "连不上 Docker：当前用户没有访问 docker socket 的权限。Linux 上把自己加进 docker 组后重新登录：sudo usermod -aG docker $USER".to_string();
    }
    if s.is_empty() {
        return "Docker 客户端在，但连不上后台服务（daemon）。".to_string();
    }
    format!(
        "Docker 客户端在，但连不上后台服务（daemon）。Docker 的原话：{}",
        s.lines().next().unwrap_or(s)
    )
}

/// compose 版本。两种形态都认（`docker compose` 插件 / 独立 `docker-compose`），
/// 实际判断在 [`which::compose_info`]。
pub fn compose_version() -> Option<String> {
    which::compose_info().version
}

/// Windows：`wsl --status` 退出码 0 视为可用。
/// 注意 wsl.exe 的输出是 UTF-16，所以**只看退出码，不解析文本**。
fn wsl_available() -> bool {
    let wsl = which::resolve("wsl")
        .resolved
        .unwrap_or_else(|| "wsl".to_string());
    proc::run_timeout(&wsl, &["--status"], Duration::from_secs(20))
        .map(|r| r.ok())
        .unwrap_or(false)
}

fn evaluate_minimum(info: &mut DockerInfo) {
    if !info.installed || !info.daemon_running {
        info.meets_minimum = false;
        return;
    }
    let engine_ok = info
        .server_version
        .as_deref()
        .and_then(major)
        .map(|m| m >= MIN_ENGINE_MAJOR)
        .unwrap_or(false);
    let compose_ok = info
        .compose_version
        .as_deref()
        .and_then(compose_pair)
        .map(|p| p >= MIN_COMPOSE)
        .unwrap_or(false);
    info.meets_minimum = engine_ok && compose_ok;
    if !engine_ok {
        info.problem = Some(format!(
            "Docker Engine 版本太低（需要 {MIN_ENGINE_MAJOR} 及以上，现在是 {}）。",
            info.server_version.clone().unwrap_or_else(|| "未知".into())
        ));
    } else if !compose_ok {
        info.problem = Some(format!(
            "Docker Compose 版本太低（需要 v{}.{} 及以上，现在是 {}）。启动器用的 --progress json 与 !override 都要 v2.20+。",
            MIN_COMPOSE.0,
            MIN_COMPOSE.1,
            info.compose_version.clone().unwrap_or_else(|| "读不到".into())
        ));
    }
}

/// `"29.8.1"` → 29；`"v5.5.1"` → 5
pub fn major(v: &str) -> Option<u32> {
    v.trim_start_matches('v')
        .split('.')
        .next()?
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .ok()
}

/// `"v5.5.1"` → (5, 5)
pub fn compose_pair(v: &str) -> Option<(u32, u32)> {
    let v = v.trim_start_matches('v');
    let mut it = v.split('.');
    let a: u32 = it.next()?.parse().ok()?;
    let b: u32 = it
        .next()
        .unwrap_or("0")
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .unwrap_or(0);
    Some((a, b))
}

/// 三平台的安装引导（方案 §6）。文案与链接都写在这里，界面只负责渲染。
pub fn install_guide() -> InstallGuide {
    let license_note = "提示：Docker Desktop 对「员工 250 人以上或年收入超过 1000 万美元」的企业是收费的。\
        个人与小团队免费。不想用它的话，mac 上可以换 OrbStack，Linux 上直接用 Docker Engine（本来就免费），\
        Podman 也能跑但属于实验支持。"
        .to_string();

    if cfg!(target_os = "windows") {
        InstallGuide {
            platform: "windows".into(),
            title: "在 Windows 上装 Docker".into(),
            steps: vec![
                GuideStep {
                    text: "先确认系统是 Windows 10 21H2 及以上或 Windows 11，并在 BIOS 里打开虚拟化（Intel VT-x / AMD-V）。".into(),
                    url: None,
                },
                GuideStep {
                    text: "以管理员身份打开 PowerShell，执行 wsl --install 装 WSL2。**装完需要重启电脑**。".into(),
                    url: Some("https://learn.microsoft.com/zh-cn/windows/wsl/install".into()),
                },
                GuideStep {
                    text: "下载并安装 Docker Desktop for Windows，安装完按提示注销或重启一次。".into(),
                    url: Some("https://www.docker.com/products/docker-desktop/".into()),
                },
                GuideStep {
                    text: "如果 WSL2 吃内存太多，在 %USERPROFILE%\\.wslconfig 里写 [wsl2] 段的 memory=4GB 限一下。".into(),
                    url: Some("https://learn.microsoft.com/zh-cn/windows/wsl/wsl-config".into()),
                },
                GuideStep {
                    text: "用 Clash / v2ray 等代理的话，TUN 模式会让容器连不上网。把 localhost、127.0.0.1 加进 NO_PROXY，或者拉镜像时先关掉 TUN。".into(),
                    url: None,
                },
            ],
            license_note,
        }
    } else if cfg!(target_os = "macos") {
        InstallGuide {
            platform: "macos".into(),
            title: "在 macOS 上装 Docker".into(),
            steps: vec![
                GuideStep {
                    text: "推荐 OrbStack：比 Docker Desktop 轻、启动快，Apple Silicon 上表现更好。".into(),
                    url: Some("https://orbstack.dev/download".into()),
                },
                GuideStep {
                    text: "也可以用 Docker Desktop for Mac（注意按芯片选 Apple Silicon 还是 Intel 版本）。".into(),
                    url: Some("https://www.docker.com/products/docker-desktop/".into()),
                },
                GuideStep {
                    text: "命令行党可以用 Colima：brew install colima docker docker-compose，然后 colima start。".into(),
                    url: Some("https://github.com/abiosoft/colima".into()),
                },
                GuideStep {
                    text: "Hunter 的六个镜像都有原生 arm64 版本，Apple Silicon 上不需要 Rosetta。".into(),
                    url: None,
                },
            ],
            license_note,
        }
    } else {
        InstallGuide {
            platform: "linux".into(),
            title: "在 Linux 上装 Docker".into(),
            steps: vec![
                GuideStep {
                    text: "官方便利脚本：curl -fsSL https://get.docker.com | sh（国内可以加 --mirror Aliyun）。".into(),
                    url: Some("https://docs.docker.com/engine/install/".into()),
                },
                GuideStep {
                    text: "装完把自己加进 docker 组，免得每条命令都要 sudo：sudo usermod -aG docker $USER，然后**重新登录**一次。".into(),
                    url: None,
                },
                GuideStep {
                    text: "确认 compose 插件也在：docker compose version，要 v2.20 及以上。".into(),
                    url: Some("https://docs.docker.com/compose/install/linux/".into()),
                },
                GuideStep {
                    text: "服务器没有桌面环境的话，用 hunter-launcher --headless 走纯命令行安装，流程和界面版完全一样。".into(),
                    url: None,
                },
            ],
            license_note,
        }
    }
}

/// 尝试把 daemon 拉起来。**只在 Linux 上做**（systemd），而且失败不算错 ——
/// mac / Windows 上启动 Docker Desktop 要用户自己点，启动器替他点不了。
pub fn try_start_daemon() -> AppResult<String> {
    if cfg!(target_os = "linux") {
        // systemctl 在 /usr/bin，各发行版的 GUI 会话里都在默认 PATH 上；
        // 仍然走一遍定位器，口径统一（找不到就退回裸名字，行为和以前一样）
        let sc = which::resolve("systemctl")
            .resolved
            .unwrap_or_else(|| "systemctl".to_string());
        let r = proc::run_timeout(&sc, &["start", "docker"], Duration::from_secs(60))?;
        if r.ok() {
            return Ok("已请求 systemd 启动 docker 服务。".to_string());
        }
        return Err(AppError::new(
            Code::DaemonDown,
            format!("启动 docker 服务失败（可能需要 sudo）：{}", r.err_line()),
        ));
    }
    Err(AppError::new(
        Code::DaemonDown,
        "请手动启动 Docker Desktop / OrbStack，启动完点「重新检测」。".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// M0 §5.4 记下来的真实输出（已裁掉用不上的字段）
    const OK_JSON: &str = r#"{"Client":{"Version":"29.8.1","Arch":"amd64","Platform":{"Name":"Docker Engine - Community"}},
      "Server":{"Version":"29.8.1","Arch":"amd64","Platform":{"Name":"Docker Engine - Community"},
      "Components":[{"Name":"Engine"},{"Name":"containerd"},{"Name":"runc"}]}}"#;

    const DAEMON_DOWN_JSON: &str = r#"{"Client":{"Version":"29.8.1","Arch":"amd64","Platform":{"Name":"Docker Engine - Community"}},"Server":null}"#;

    #[test]
    fn 正常时解析出两侧版本() {
        let v = parse_version(OK_JSON).expect("应当能解析");
        let mut info = blank();
        info.installed = true;
        apply_version(&mut info, v, Some("default"));
        assert!(info.daemon_running);
        assert_eq!(info.server_version.as_deref(), Some("29.8.1"));
        assert_eq!(info.client_version.as_deref(), Some("29.8.1"));
        assert_eq!(info.arch.as_deref(), Some("amd64"));
        assert_eq!(info.runtime, RuntimeKind::DockerEngine);
    }

    /// M0 §5.4 最关键的一条：退出码 1 + Client 还在 + Server 为 null = daemon 没起，**不是**没装
    #[test]
    fn server_为_null_时判定为_daemon_没起而不是没装() {
        let v = parse_version(DAEMON_DOWN_JSON).expect("应当能解析");
        let mut info = blank();
        info.installed = v.client.is_some();
        apply_version(&mut info, v, None);
        assert!(info.installed, "Client 段还在就说明 docker 是装了的");
        assert!(!info.daemon_running);
        assert_eq!(info.error_code(), Some(Code::DaemonDown));
    }

    #[test]
    fn 运行时种类的识别() {
        assert_eq!(
            classify(Some("Docker Desktop 4.34.2 (167172)"), None, None),
            RuntimeKind::DockerDesktop
        );
        assert_eq!(
            classify(Some("OrbStack"), None, None),
            RuntimeKind::Orbstack
        );
        assert_eq!(
            classify(Some("Docker Engine - Community"), None, None),
            RuntimeKind::DockerEngine
        );
        assert_eq!(classify(None, None, Some("colima")), RuntimeKind::Colima);
        assert_eq!(classify(Some("podman"), None, None), RuntimeKind::Podman);
        assert_eq!(classify(None, None, None), RuntimeKind::Unknown);
        // context 名能救回 Platform.Name 认不出的情况
        assert_eq!(
            classify(Some("Docker Engine"), None, Some("orbstack")),
            RuntimeKind::Orbstack
        );
    }

    /// 待办池 P1-5 的实测（I3 在测试机上真装了一次 podman 4.9.3，
    /// 用一个叫 `docker` 的软链指过去）。下面两段 JSON 都是**原文**。
    ///
    /// 三件事一眼可见：
    ///   1. podman 那份是**合法的 Docker 形状 JSON**，`parse_version` 解析得动 ——
    ///      所以「stdout 不是 JSON 才认 podman」那条老分支永远轮不到；
    ///   2. 整段里**一个 `podman` 字样都没有**；
    ///   3. 但它有一个 Docker 从来不发的键：`OsArch`（Docker 那边是分开的 `Os` + `Arch`）。
    ///      这就是不用多跑一个进程就能分出来的那个记号。
    #[test]
    fn podman_与_docker_的_client_json_形状不同() {
        let podman = r#"{"Client":{"APIVersion":"4.9.3","Version":"4.9.3","GoVersion":"go1.22.2",
            "GitCommit":"","BuiltTime":"Thu Jan  1 00:00:00 1970","Built":0,
            "OsArch":"linux/amd64","Os":"linux"}}"#;
        assert!(
            !podman.to_lowercase().contains("podman"),
            "这正是问题所在：JSON 里一个 podman 字样都没有"
        );
        let v = parse_version(podman).expect("podman 的这份 JSON 是解析得动的");
        assert!(v.server.is_none(), "podman 的 docker 兼容输出里没有 Server");
        let mut info = blank();
        apply_version(&mut info, v, None);
        assert!(!info.daemon_running);
        assert_eq!(
            info.runtime,
            RuntimeKind::Podman,
            "靠 OsArch 这个键就该认出来，不必多跑一个进程"
        );

        // 对照：同一台机器上真实 docker 29.8.1 的 Client 块，没有 OsArch
        let docker = r#"{"Client":{"Platform":{"Name":""},"Version":"29.8.1",
            "ApiVersion":"1.56","DefaultAPIVersion":"1.56","GitCommit":"4a63305",
            "GoVersion":"go1.26.8","Os":"linux","Arch":"amd64","Context":"default"}}"#;
        let v2 = parse_version(docker).expect("docker 的 JSON");
        let mut info2 = blank();
        apply_version(&mut info2, v2, None);
        assert_ne!(
            info2.runtime,
            RuntimeKind::Podman,
            "真 docker 不能被认成 podman"
        );
    }

    /// 纯文本那一条是第二道保险。**不能用 `docker --version`（两道杠）** ——
    /// podman 用 `argv[0]` 当程序名，通过一个叫 `docker` 的软链调用时它打的是
    /// `docker version 4.9.3`，一个 podman 字样都没有（I3 第一版就栽在这里）。
    /// `docker version`（不带 `--format`）的第一行才靠得住。
    #[test]
    fn 纯文本的_version_能分清运行时() {
        // 实测原文（podman 通过名为 docker 的软链调用）
        assert_eq!(
            classify_plain("Client:       Podman Engine Version:      4.9.3 API Version:  4.9.3"),
            RuntimeKind::Podman
        );
        // 实测原文（同一台机器上真实的 docker）
        assert_eq!(
            classify_plain("Client: Docker Engine - Community  Version:           29.8.1"),
            RuntimeKind::DockerEngine
        );
        // 这一行是 `docker --version` 在 podman 上的输出 —— 正是那个陷阱：
        // 既不含 podman，也不含「docker engine」，所以只能是 Unknown，
        // **绝不能被当成 Docker Engine**
        assert_eq!(
            classify_plain("docker version 4.9.3"),
            RuntimeKind::Unknown,
            "认不出来就说认不出来，不许猜成 Docker"
        );
        assert_eq!(classify_plain(""), RuntimeKind::Unknown);
    }

    /// 认出 Podman 之后说的话必须**把没验过的部分说出来**（红线 1）。
    #[test]
    fn podman_的提示不许假装支持() {
        // 传进来的是版本号，不是整行 —— 这里补出来的必须是「Podman 4.9.3」。
        // I3 第一次真机验证时这里漏了，界面上写成「实际上是 4.9.3，不是 Docker」
        let h = podman_hint(Some("4.9.3"));
        assert!(h.contains("实际上是 Podman 4.9.3，"), "{h}");
        assert!(h.contains("实验支持"), "{h}");
        assert!(h.contains("从来没有在 Podman 上跑通过"), "{h}");
        assert!(h.contains("compose"), "要说清缺的是 compose provider：{h}");
        // 不能出现「Docker 客户端在但连不上 daemon」那套会把人带偏的话
        assert!(!h.contains("连不上后台服务"), "{h}");

        // 版本号读不到时也得是一句通顺的话，不能留一个空洞
        let h2 = podman_hint(None);
        assert!(h2.contains("实际上是 Podman，"), "{h2}");
        let h3 = podman_hint(Some("   "));
        assert!(h3.contains("实际上是 Podman，"), "{h3}");
    }

    #[test]
    fn 版本比较() {
        assert_eq!(major("29.8.1"), Some(29));
        assert_eq!(major("v5.5.1"), Some(5));
        assert_eq!(major("24.0.0-beta"), Some(24));
        assert_eq!(compose_pair("v5.5.1"), Some((5, 5)));
        assert_eq!(compose_pair("v2.19.1"), Some((2, 19)));
        assert!(compose_pair("v2.19.1").unwrap() < MIN_COMPOSE);
        assert!(compose_pair("v2.20.0").unwrap() >= MIN_COMPOSE);
    }

    #[test]
    fn 版本太低时给出具体是哪一项不满足() {
        let mut info = blank();
        info.installed = true;
        info.daemon_running = true;
        info.server_version = Some("23.0.1".into());
        info.compose_version = Some("v5.5.1".into());
        evaluate_minimum(&mut info);
        assert!(!info.meets_minimum);
        assert!(
            info.problem.as_ref().unwrap().contains("Engine"),
            "{:?}",
            info.problem
        );

        let mut info = blank();
        info.installed = true;
        info.daemon_running = true;
        info.server_version = Some("29.8.1".into());
        info.compose_version = Some("v2.19.0".into());
        evaluate_minimum(&mut info);
        assert!(!info.meets_minimum);
        assert!(
            info.problem.as_ref().unwrap().contains("Compose"),
            "{:?}",
            info.problem
        );
    }

    #[test]
    fn 三平台的安装引导都有内容且带链接() {
        let g = install_guide();
        assert!(!g.steps.is_empty());
        assert!(
            g.steps.iter().any(|s| s.url.is_some()),
            "至少要有一个能点的官方链接"
        );
        assert!(
            g.license_note.contains("250"),
            "方案 §6 要求的 Docker Desktop 授权提示"
        );
    }

    #[test]
    fn 没装时的错误码是_docker_missing() {
        let info = blank();
        assert_eq!(info.error_code(), Some(Code::DockerMissing));
        assert!(!info.ready());
    }

    #[test]
    fn 权限不足时的提示要说清怎么办() {
        let h =
            daemon_hint("permission denied while trying to connect to the Docker daemon socket");
        assert!(h.contains("usermod -aG docker"), "{h}");
    }

    fn blank() -> DockerInfo {
        DockerInfo::empty()
    }
}
