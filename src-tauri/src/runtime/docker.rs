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

/// 真去跑一遍检测。这是整个模块唯一会碰外部进程的函数。
pub fn detect() -> DockerInfo {
    let ran = proc::run_timeout(
        "docker",
        &["version", "--format", "json"],
        Duration::from_secs(20),
    );

    let mut info = DockerInfo {
        installed: false,
        daemon_running: false,
        runtime: RuntimeKind::Unknown,
        runtime_label: None,
        client_version: None,
        server_version: None,
        compose_version: None,
        arch: None,
        wsl: None,
        meets_minimum: false,
        problem: None,
        install_guide: None,
    };

    match ran {
        // 进程起不来 = 没装（M0 §5.4）
        Err(_) => {
            info.problem = Some("命令行里找不到 docker。".to_string());
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
                            if let Some(name) = plain_version_line() {
                                info.runtime = classify_plain(&name);
                                if matches!(info.runtime, RuntimeKind::Podman) {
                                    info.client_version =
                                        info.client_version.clone().or_else(|| Some(name.clone()));
                                }
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
        info.compose_version = compose_version();
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
        }
    }
}

/// `docker --version` 的第一行纯文本。只在「JSON 里没有 Server」那条路上跑一次。
///
/// 为什么不直接信 `version --format json`：podman 的那份 JSON 里**一个 `podman` 字样都没有**
/// （I3 实测的原文见 `podman_json_里没有任何_podman_字样` 那条测试），
/// 而纯文本那一行第一个词就是运行时的名字。
fn plain_version_line() -> Option<String> {
    let r = proc::run_timeout("docker", &["--version"], Duration::from_secs(10)).ok()?;
    let line = r.stdout.lines().next().unwrap_or("").trim().to_string();
    if line.is_empty() {
        None
    } else {
        Some(line)
    }
}

/// 从 `docker --version` 的那一行认运行时。认不出来就 Unknown —— **不猜成 Docker**。
pub fn classify_plain(line: &str) -> RuntimeKind {
    let l = line.to_lowercase();
    if l.contains("podman") {
        RuntimeKind::Podman
    } else if l.contains("orbstack") {
        RuntimeKind::Orbstack
    } else if l.contains("docker") {
        RuntimeKind::DockerEngine
    } else {
        RuntimeKind::Unknown
    }
}

/// 认出 Podman 时该说的话。**不假装支持**（红线 1）：本项目没有在 Podman 上
/// 跑通过完整安装，只验过「认得出来」这一步，所以这里把已知和未知分开写。
fn podman_hint(version_line: Option<&str>) -> String {
    let v = version_line.unwrap_or("Podman");
    format!(
        "命令行里的 docker 实际上是 {v}，不是 Docker。\
         Podman 目前是**实验支持**：启动器能认出它，但整套安装从来没有在 Podman 上跑通过 —— \
         `docker compose` 在 Podman 上要另外装一个 compose provider（podman-compose 或 docker-compose），\
         没有它 Hunter 起不来。\
         最稳的做法是装 Docker Engine；一定要用 Podman 的话，请先自己确认 `docker compose version` 有输出。"
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
    proc::run_timeout("docker", &["context", "show"], Duration::from_secs(10))
        .ok()
        .filter(|r| r.ok())
        .map(|r| r.stdout.trim().to_string())
        .filter(|s| !s.is_empty())
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

/// `docker compose version --format json` → `{"version":"v5.5.1"}`（M0 §5.4 实测）
pub fn compose_version() -> Option<String> {
    let r = proc::run_timeout(
        "docker",
        &["compose", "version", "--format", "json"],
        Duration::from_secs(20),
    )
    .ok()?;
    if !r.ok() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(r.stdout.trim()).ok()?;
    v.get("version")?.as_str().map(|s| s.to_string())
}

/// Windows：`wsl --status` 退出码 0 视为可用。
/// 注意 wsl.exe 的输出是 UTF-16，所以**只看退出码，不解析文本**。
fn wsl_available() -> bool {
    proc::run_timeout("wsl", &["--status"], Duration::from_secs(20))
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
        let r = proc::run_timeout("systemctl", &["start", "docker"], Duration::from_secs(60))?;
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

    /// 待办池 P1-5 的实测（I3 在测试机上真装了一次 podman 4.9.3）。
    ///
    /// 这是 `podman version --format json` 的**原文**。两件事一眼可见：
    ///   1. 它是**合法的 Docker 形状 JSON**，`parse_version` 解析得动 ——
    ///      所以「stdout 不是 JSON 才认 podman」那条老分支永远轮不到；
    ///   2. 整段里**一个 `podman` 字样都没有**，光看 JSON 认不出来。
    #[test]
    fn podman_json_里没有任何_podman_字样() {
        let raw = r#"{"Client":{"APIVersion":"4.9.3","Version":"4.9.3","GoVersion":"go1.22.2",
            "GitCommit":"","BuiltTime":"Thu Jan  1 00:00:00 1970","Built":0,
            "OsArch":"linux/amd64","Os":"linux"}}"#;
        let v = parse_version(raw).expect("podman 的这份 JSON 是解析得动的");
        assert!(v.client.is_some());
        assert!(v.server.is_none(), "podman 的 docker 兼容输出里没有 Server");
        assert!(
            !raw.to_lowercase().contains("podman"),
            "这正是问题所在：JSON 里认不出 podman"
        );

        let mut info = blank();
        apply_version(&mut info, v, None);
        assert!(!info.daemon_running);
        assert_eq!(
            info.runtime,
            RuntimeKind::Unknown,
            "光靠 JSON 只能是 Unknown —— 所以才要再看一眼 `docker --version`"
        );
    }

    /// `docker --version` 那一行才分得清。认不出来的一律 Unknown，**不猜成 Docker**。
    #[test]
    fn 纯文本的_version_行能分清运行时() {
        // 下面两行都是实测原文
        assert_eq!(classify_plain("podman version 4.9.3"), RuntimeKind::Podman);
        assert_eq!(
            classify_plain("Docker version 29.8.1, build 1a2b3c4"),
            RuntimeKind::DockerEngine
        );
        assert_eq!(classify_plain(""), RuntimeKind::Unknown);
        assert_eq!(classify_plain("something else 1.0"), RuntimeKind::Unknown);
    }

    /// 认出 Podman 之后说的话必须**把没验过的部分说出来**（红线 1）。
    #[test]
    fn podman_的提示不许假装支持() {
        let h = podman_hint(Some("podman version 4.9.3"));
        assert!(h.contains("podman version 4.9.3"), "{h}");
        assert!(h.contains("实验支持"), "{h}");
        assert!(h.contains("从来没有在 Podman 上跑通过"), "{h}");
        assert!(h.contains("compose"), "要说清缺的是 compose provider：{h}");
        // 不能出现「装 Docker 客户端在但连不上 daemon」那套会把人带偏的话
        assert!(!h.contains("连不上后台服务"), "{h}");
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
        DockerInfo {
            installed: false,
            daemon_running: false,
            runtime: RuntimeKind::Unknown,
            runtime_label: None,
            client_version: None,
            server_version: None,
            compose_version: None,
            arch: None,
            wsl: None,
            meets_minimum: false,
            problem: None,
            install_guide: None,
        }
    }
}
