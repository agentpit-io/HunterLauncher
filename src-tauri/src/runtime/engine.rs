//! 机器当前容器引擎是否可用的统一判据（I20 · P0-1）。
//!
//! ## 背景
//!
//! * `runtime::effective::current()` 的设计目标是零子进程、纯查文件/Socket，以避免与
//!   `env::compute()` / `env::apply()` 发生循环调用（I8 曾因子进程引发 stack overflow）。
//! * 但在 Windows 平台上，Docker Desktop 走命名管道（`npipe://`），`effective.rs` 的
//!   Socket 探测恒为 false，导致 `effective::current().running` 在 Windows 上恒为 `false`。
//! * 在探测真实的 Docker 守护进程是否在跑时，Windows 走的是 `runtime::docker::detect().daemon_running`
//!   （即 `docker version --format json` 带有 Server 段）。
//! * 因此，本模块建立统一的容器引擎可用性判定：
//!   - Windows：以 `runtime::docker::detect().daemon_running` 为准；
//!   - 非 Windows（Mac/Linux）：以 `runtime::effective::current().running` 为准。

use super::{Os, OS};

/// 纯函数版本：用于单测在跨平台（包括 Linux/Mac/Windows runner）上无歧义地覆盖判定矩阵。
pub fn is_engine_usable(eff_running: bool, docker_daemon_running: bool, os: Os) -> bool {
    match os {
        Os::Windows => docker_daemon_running,
        Os::Mac | Os::Linux => eff_running,
    }
}

/// 用于状态文案格式化的 Docker 核心状态。
#[derive(Debug, Clone, Copy, Default)]
pub struct DockerStatus<'a> {
    pub installed: bool,
    pub daemon_running: bool,
    pub client_version: Option<&'a str>,
    pub server_version: Option<&'a str>,
    pub compose_mode: Option<&'a str>,
}

/// 纯函数版本：生成容器引擎状态的人话描述与可用性（I20 · P0-4）。
///
/// 遵循真实原则：Windows 下不许把「未安装」说成「装了没跑」。
pub fn format_engine_status(
    docker: DockerStatus<'_>,
    eff_running: bool,
    eff_one_line: &str,
    os: Os,
) -> (bool, String) {
    let u = is_engine_usable(eff_running, docker.daemon_running, os);
    let detail = match os {
        Os::Windows => {
            if docker.daemon_running {
                format!(
                    "Windows: Docker 后台正在运行（客户端: {}，服务端: {}，模式: {}）",
                    docker.client_version.unwrap_or("未知"),
                    docker.server_version.unwrap_or("未知"),
                    docker.compose_mode.unwrap_or("未知")
                )
            } else if docker.installed {
                "Windows: Docker 已安装但后台未运行".to_string()
            } else {
                "Windows: 未检测到已安装的 Docker".to_string()
            }
        }
        Os::Mac | Os::Linux => {
            format!(
                "{}: {}",
                if eff_running {
                    "生效中"
                } else {
                    "未生效"
                },
                eff_one_line
            )
        }
    };
    (u, detail)
}

/// 运行时真实环境判定：当前平台上的容器引擎能否正常工作。
pub fn usable() -> bool {
    match OS {
        Os::Windows => super::docker::detect().daemon_running,
        Os::Mac | Os::Linux => super::effective::current().running,
    }
}

/// 引擎可用性简要诊断说明（用于排查与诊断包），返回 (是否可用, 人话依据)。
pub fn summary() -> (bool, String) {
    let d = super::docker::detect();
    let eff = super::effective::current();
    format_engine_status(
        DockerStatus {
            installed: d.installed,
            daemon_running: d.daemon_running,
            client_version: d.client_version.as_deref(),
            server_version: d.server_version.as_deref(),
            compose_mode: d.compose_mode.as_deref(),
        },
        eff.running,
        &eff.one_line(),
        OS,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_engine_status_windows() {
        // 1. 没装：必须如实说未检测到，不许说「装了但未运行」
        let (u, s) = format_engine_status(DockerStatus::default(), false, "", Os::Windows);
        assert!(!u);
        assert_eq!(s, "Windows: 未检测到已安装的 Docker");

        // 2. 装了没跑
        let (u, s) = format_engine_status(
            DockerStatus {
                installed: true,
                daemon_running: false,
                client_version: Some("27.0"),
                ..Default::default()
            },
            false,
            "",
            Os::Windows,
        );
        assert!(!u);
        assert_eq!(s, "Windows: Docker 已安装但后台未运行");

        // 3. 在跑
        let (u, s) = format_engine_status(
            DockerStatus {
                installed: true,
                daemon_running: true,
                client_version: Some("27.0"),
                server_version: Some("27.0"),
                compose_mode: Some("plugin"),
            },
            false,
            "",
            Os::Windows,
        );
        assert!(u);
        assert!(s.contains("Windows: Docker 后台正在运行"));
    }

    #[test]
    fn test_engine_usable_windows_running() {
        // Windows 上即使 effective 恒为 false，只要 docker daemon 在跑即判定为可用
        assert!(is_engine_usable(false, true, Os::Windows));
        assert!(is_engine_usable(true, true, Os::Windows));
    }

    #[test]
    fn test_engine_usable_windows_down() {
        // Windows 上 docker daemon 没在跑即判定不可用
        assert!(!is_engine_usable(false, false, Os::Windows));
        assert!(!is_engine_usable(true, false, Os::Windows));
    }

    #[test]
    fn test_engine_usable_non_windows_running() {
        // 非 Windows（Linux/Mac）上按 effective::running 判定，与 docker_daemon_running 无关
        assert!(is_engine_usable(true, false, Os::Linux));
        assert!(is_engine_usable(true, true, Os::Linux));
        assert!(is_engine_usable(true, false, Os::Mac));
        assert!(is_engine_usable(true, true, Os::Mac));
    }

    #[test]
    fn test_engine_usable_non_windows_down() {
        // 非 Windows（Linux/Mac）上 effective 为 false 时恒为不可用
        assert!(!is_engine_usable(false, true, Os::Linux));
        assert!(!is_engine_usable(false, false, Os::Linux));
        assert!(!is_engine_usable(false, true, Os::Mac));
        assert!(!is_engine_usable(false, false, Os::Mac));
    }
}
