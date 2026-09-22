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
    /// 内置运行时（`~/.hunter/runtime` 里那一套）装着，但它的虚拟机没起来（I9）。
    ///
    /// **和 `DaemonDown` 分开是有代价的，但必须分**：0.1.8 在用户 Mac 上把这个
    /// 现场判成了 `DaemonDown`，于是规则层给出「Colima 装着但没在运行，把它启动
    /// 起来」，执行的是一条裸 `colima start`（我们自己的 colima + 用户的
    /// `~/.colima`），在他家目录里建了一台没人要的虚拟机，而内置主线一步都没走。
    /// 两个现场的解法完全不同，错误码必须也不同。
    BuiltinRuntimeDown,
    WslMissing,
    KeyInvalid,
    QuotaExhausted,
    PullFailed,
    PortInUse,
    /// **起容器时** Docker 自己报的端口冲突（I5）。原话形如
    /// `Bind for 0.0.0.0:8100 failed: port is already allocated`。
    ///
    /// 为什么不并进 `E_PORT_IN_USE`：那一条是**装之前**我们自己探出来的，
    /// 处置是「换一组端口再装」；这一条是**装到一半** Docker 拒绝的，
    /// 说明我们的探测漏判了，处置是「重新探 + remap + 重来」。
    /// 0.1.4 把它归成了 `E_START_TIMEOUT`（「180 秒没就绪」），
    /// 于是规则层的 ports-taken 根本没被匹配上 —— 这是用户 Mac 上那次失败的第 2 条根因。
    PortConflict,
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
    /// 本机的 docker 凭据助手不在子进程看得到的 PATH 上（I6）。原话形如
    /// `error getting credentials - err: exec: "docker-credential-osxkeychain":
    /// executable file not found in $PATH`。
    ///
    /// 为什么要单独一个码：0.1.5 把它归成了 `E_PULL_FAILED`，规则层于是照着
    /// 「拉不动 = 源不通」去换源，在 ghcr 与腾讯云之间来回换了三次 ——
    /// **这是本机配置的问题，换源永远修不好**。单独一个码，规则层才能给出
    /// 对症的动作（补 PATH 重试 / 另起一份不带 credsStore 的 docker 配置）。
    CredHelper,
    /// **Hunter 自己那台虚拟机没有可用的 DNS**（I10）。
    ///
    /// 0.1.9 在用户 Mac 上这个现场被报成了 `E_START_TIMEOUT`（「180 秒还有 1 个
    /// 服务没就绪」），规则层因此 `unknown`、诊断助手读了 4 次日志也没看出来。
    /// 真正的事实是：启动器自带的那份 Ubuntu 24.04 minimal 镜像里没有
    /// systemd-resolved，而 `/etc/resolv.conf` 出厂就是一条指向它的断链 ——
    /// **虚拟机和它里面所有容器都没有任何 DNS**。
    ///
    /// 为什么必须单独一个码：这一条有确定的修法（在虚拟机里把 resolv.conf 写成
    /// 普通文件，见 [`crate::runtime::vmdns`]），而「启动超时」没有。
    /// 错误码错了，对症的那条规则就永远匹配不上 —— I5 在端口冲突上踩过一模一样的坑。
    RuntimeNoDns,
    /// **容器连不上模型网关**（I10）。
    ///
    /// 和上一条分开，因为「容器不通」不等于「虚拟机没 DNS」：用户自己的 Docker
    /// 上也可能出现（代理只放行了宿主机、公司网络挡了 443），
    /// 那种情况启动器**不能**去改他机器的网络设置，只能如实说清楚。
    ContainerOffline,
    /// 网关限流（HTTP 429）。I2 实测的真实响应：
    /// `{"error":{"message":"请求太频繁了：每分钟最多 20 次…","type":"rate_limit_error",
    /// "code":"rate_limited","retry_after":60}}`，响应头 `retry-after: 60`。
    ///
    /// **这一条目前打不到启动器身上**：实测限流只作用于
    /// `POST /v1/chat/completions`，而启动器只用 `/quota` 与 `/v1/models`
    /// （30 次连打都不触发，触发之后 `/quota` 照样 200）。留着是因为
    /// 「key 好好的但这一刻问不出来」和「key 是坏的」必须分得开 ——
    /// 网关哪天把限流扩到别的路径上，这里不该显示成「key 无效」。
    RateLimited,
    NotImplemented,
    Unknown,
}

impl Code {
    pub fn as_str(self) -> &'static str {
        match self {
            Code::DockerMissing => "E_DOCKER_MISSING",
            Code::DaemonDown => "E_DAEMON_DOWN",
            Code::BuiltinRuntimeDown => "E_BUILTIN_DOWN",
            Code::WslMissing => "E_WSL_MISSING",
            Code::KeyInvalid => "E_KEY_INVALID",
            Code::QuotaExhausted => "E_QUOTA_EXHAUSTED",
            Code::PullFailed => "E_PULL_FAILED",
            Code::PortInUse => "E_PORT_IN_USE",
            Code::PortConflict => "E_PORT_CONFLICT",
            Code::CredHelper => "E_CRED_HELPER",
            Code::RuntimeNoDns => "E_RUNTIME_NO_DNS",
            Code::ContainerOffline => "E_CONTAINER_OFFLINE",
            Code::StartTimeout => "E_START_TIMEOUT",
            Code::ProxyBlock => "E_PROXY_BLOCK",
            Code::UpdateFailed => "E_UPDATE_FAILED",
            Code::ComposeFetch => "E_COMPOSE_FETCH",
            Code::ConfigWrite => "E_CONFIG_WRITE",
            Code::ProjectConflict => "E_PROJECT_CONFLICT",
            Code::RateLimited => "E_RATE_LIMITED",
            Code::NotImplemented => "E_NOT_IMPLEMENTED",
            Code::Unknown => "E_UNKNOWN",
        }
    }

    /// 错误页上的标题。具体细节由 `AppError::msg` 承担，这里只给一句话。
    pub fn title(self) -> &'static str {
        match self {
            Code::DockerMissing => "没有找到 Docker",
            Code::DaemonDown => "Docker 装了，但没在运行",
            Code::BuiltinRuntimeDown => "Hunter 自己那台虚拟机没起来",
            Code::WslMissing => "Windows 缺少 WSL2",
            Code::KeyInvalid => "这把 key 网关不认",
            Code::QuotaExhausted => "今日免费额度已用完",
            Code::PullFailed => "镜像拉取失败",
            Code::PortInUse => "端口被占用",
            Code::PortConflict => "Docker 说端口已经被占了",
            Code::CredHelper => "Docker 找不到它自己的凭据助手",
            Code::RuntimeNoDns => "Hunter 自己那台虚拟机没有可用的 DNS",
            Code::ContainerOffline => "容器连不上 Hunter 的模型网关",
            Code::StartTimeout => "服务在 180 秒内没有全部就绪",
            Code::ProxyBlock => "代理挡住了容器的网络",
            Code::UpdateFailed => "升级失败，已回滚",
            Code::ComposeFetch => "取不到 Hunter 的 compose 文件",
            Code::ConfigWrite => "写配置失败",
            Code::ProjectConflict => "另一个工作目录正占着 hunter 这个项目名",
            Code::RateLimited => "请求太频繁，网关暂时挡了一下",
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

impl Code {
    /// **全部错误码，一份清单**（I9）。
    ///
    /// 在这之前同一张表在三个地方各抄了一份：这里的单测、`headless::code_from_str`、
    /// 前端的 `ERROR_CODES`。I9 加 `E_BUILTIN_DOWN` 时漏改了 `code_from_str`，
    /// 结果命令行上打出来的是 `E_UNKNOWN: Hunter 自己那台虚拟机没起来` ——
    /// 一句话里两个互相矛盾的说法。Rust 这一侧现在只有这一份。
    pub const ALL: &'static [Code] = &[
        Code::DockerMissing,
        Code::DaemonDown,
        Code::BuiltinRuntimeDown,
        Code::WslMissing,
        Code::KeyInvalid,
        Code::QuotaExhausted,
        Code::RateLimited,
        Code::PullFailed,
        Code::PortInUse,
        Code::PortConflict,
        Code::CredHelper,
        Code::RuntimeNoDns,
        Code::ContainerOffline,
        Code::StartTimeout,
        Code::ProxyBlock,
        Code::UpdateFailed,
        Code::ComposeFetch,
        Code::ConfigWrite,
        Code::ProjectConflict,
        Code::NotImplemented,
        Code::Unknown,
    ];

    /// 从 `E_XXX` 反查。认不得就是 `None`（**不猜成 Unknown**，让调用方自己决定）。
    ///
    /// 名字不叫 `from_str`：那会和 `std::str::FromStr::from_str` 撞脸
    /// （clippy 的 `should_implement_trait`）。而实现 `FromStr` 在这里是过度设计 ——
    /// 我们不需要 `"E_X".parse::<Code>()` 那套。
    pub fn parse_code(s: &str) -> Option<Code> {
        Code::ALL.iter().copied().find(|c| c.as_str() == s)
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
        let mut seen = std::collections::HashSet::new();
        for c in Code::ALL.iter().copied() {
            assert!(!c.title().is_empty());
            assert!(seen.insert(c.as_str()), "错误码重复：{}", c.as_str());
        }
    }

    /// **每一个错误码都要能从字符串反查回来。**
    ///
    /// I9 加 `E_BUILTIN_DOWN` 时漏改了 `headless::code_from_str` 里那份手抄的清单，
    /// 于是命令行上打出来的是 `E_UNKNOWN: Hunter 自己那台虚拟机没起来`。
    /// 现在只有 `Code::ALL` 一份表，这条测试保证反查不会再漏。
    #[test]
    fn 每个错误码都能从字符串反查回来() {
        for c in Code::ALL.iter().copied() {
            assert_eq!(Code::parse_code(c.as_str()), Some(c), "{}", c.as_str());
        }
        assert_eq!(Code::parse_code("E_NOT_A_REAL_CODE"), None);
    }
}
