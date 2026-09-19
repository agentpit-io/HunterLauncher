//! 错误码与中文文案（技术方案 §18）。
//!
//! 约定：Tauri command 一律返回 `Result<T, String>`，失败时的字符串形如 `"<错误码>: <中文说明>"`，
//! 前端 `src/lib/ipc.ts` 按这个格式拆成 `IpcError { code, message }`。
//! headless 模式直接把同一个结构打到终端上，两条路径共用一套文案。

use std::fmt;

/// 方案 §18 的十个错误码 + 本地补充的四个。
/// 前端 `src/state/machine.ts` 的 `ERROR_CODES` 与这里逐个对应。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    DockerMissing,
    DaemonDown,
    WslMissing,
    KeyInvalid,
    QuotaExhausted,
    PullFailed,
    PortInUse,
    StartTimeout,
    ProxyBlock,
    UpdateFailed,
    /// 取 compose 文件失败（方案没有这一条：方案假设从 Release 资产下载，
    /// 而 M0 §3.2 实测 Release 没有任何资产，改走 raw.githubusercontent + 内置兜底）
    ComposeFetch,
    /// 写 `~/.hunter` 下的配置失败（权限、磁盘满）
    ConfigWrite,
    /// compose 项目名 `hunter` 已经被**另一个工作目录**的那一套占着（待办池 P0-5）。
    /// 方案没有这一条：它假设一台机器上只有一个 `~/.hunter`，而 M2 实测换过
    /// `HUNTER_HOME` 之后 `up -d` 会把先装好的那一套连配置带端口一起顶掉。
    ProjectConflict,
    NotImplemented,
    Unknown,
}

impl Code {
    pub fn as_str(self) -> &'static str {
        match self {
            Code::DockerMissing => "E_DOCKER_MISSING",
            Code::DaemonDown => "E_DAEMON_DOWN",
            Code::WslMissing => "E_WSL_MISSING",
            Code::KeyInvalid => "E_KEY_INVALID",
            Code::QuotaExhausted => "E_QUOTA_EXHAUSTED",
            Code::PullFailed => "E_PULL_FAILED",
            Code::PortInUse => "E_PORT_IN_USE",
            Code::StartTimeout => "E_START_TIMEOUT",
            Code::ProxyBlock => "E_PROXY_BLOCK",
            Code::UpdateFailed => "E_UPDATE_FAILED",
            Code::ComposeFetch => "E_COMPOSE_FETCH",
            Code::ConfigWrite => "E_CONFIG_WRITE",
            Code::ProjectConflict => "E_PROJECT_CONFLICT",
            Code::NotImplemented => "E_NOT_IMPLEMENTED",
            Code::Unknown => "E_UNKNOWN",
        }
    }

    /// 错误页上的标题。具体细节由 `AppError::msg` 承担，这里只给一句话。
    pub fn title(self) -> &'static str {
        match self {
            Code::DockerMissing => "没有找到 Docker",
            Code::DaemonDown => "Docker 装了，但没在运行",
            Code::WslMissing => "Windows 缺少 WSL2",
            Code::KeyInvalid => "这把 key 网关不认",
            Code::QuotaExhausted => "今日免费额度已用完",
            Code::PullFailed => "镜像拉取失败",
            Code::PortInUse => "端口被占用",
            Code::StartTimeout => "服务在 180 秒内没有全部就绪",
            Code::ProxyBlock => "代理挡住了容器的网络",
            Code::UpdateFailed => "升级失败，已回滚",
            Code::ComposeFetch => "取不到 Hunter 的 compose 文件",
            Code::ConfigWrite => "写配置失败",
            Code::ProjectConflict => "另一个工作目录正占着 hunter 这个项目名",
            Code::NotImplemented => "这个功能还没实现",
            Code::Unknown => "出了点意外",
        }
    }
}

impl fmt::Display for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone)]
pub struct AppError {
    pub code: Code,
    pub msg: String,
}

impl AppError {
    pub fn new(code: Code, msg: impl Into<String>) -> Self {
        Self {
            code,
            msg: msg.into(),
        }
    }
    pub fn unknown(msg: impl Into<String>) -> Self {
        Self::new(Code::Unknown, msg)
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 这一行会同时进日志与界面，所以走脱敏（红线 2：key 不进日志）
        write!(f, "{}: {}", self.code, crate::redact::redact(&self.msg))
    }
}

impl std::error::Error for AppError {}

impl From<AppError> for String {
    fn from(e: AppError) -> String {
        e.to_string()
    }
}

pub type AppResult<T> = std::result::Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 错误串的格式是前端能解析的() {
        let e = AppError::new(Code::KeyInvalid, "格式不对");
        assert_eq!(e.to_string(), "E_KEY_INVALID: 格式不对");
    }

    #[test]
    fn 错误串本身也要脱敏() {
        let e = AppError::new(
            Code::Unknown,
            "请求 hunt_tools_abcdefghijklmnopqrstuvwxyz012345 失败",
        );
        assert!(!e.to_string().contains("abcdefghij"), "{}", e);
        assert!(e.to_string().contains("hunt_tools_****"));
    }

    #[test]
    fn 每个错误码都有标题且不重复() {
        let all = [
            Code::DockerMissing,
            Code::DaemonDown,
            Code::WslMissing,
            Code::KeyInvalid,
            Code::QuotaExhausted,
            Code::PullFailed,
            Code::PortInUse,
            Code::StartTimeout,
            Code::ProxyBlock,
            Code::UpdateFailed,
            Code::ComposeFetch,
            Code::ConfigWrite,
            Code::ProjectConflict,
            Code::NotImplemented,
            Code::Unknown,
        ];
        let mut seen = std::collections::HashSet::new();
        for c in all {
            assert!(!c.title().is_empty());
            assert!(seen.insert(c.as_str()), "错误码重复：{}", c.as_str());
        }
    }
}
