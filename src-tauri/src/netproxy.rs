//! 沿用用户自己的网络代理（I8 · 0.1.7 真机首测的第二个根因）。
//!
//! ## 现场
//!
//! 用户 2026-09-21 22:03 在自己的 Mac 上装 0.1.7：系统设置里配着代理
//! `127.0.0.1:7897`（`scutil --proxy` 显示 HTTP / HTTPS / SOCKS 都开着），
//! 直连 github.com 十秒超时、走代理两秒 200。四个组件从腾讯云香港下好了，
//! 接着 `colima start` 去 GitHub 下虚拟机镜像 —— **那一次下载没有走代理**，
//! 于是 `connection timed out`。
//!
//! 一句话：**用户已经把路铺好了，是我们没走。**
//!
//! ## 这个模块做什么、不做什么
//!
//! | 做 | 不做 |
//! |---|---|
//! | **只读**地读出这台机器上已经配好的代理 | **绝不修改**任何网络设置（这是授权页上写死的承诺） |
//! | 把它传给子进程（colima / limactl / docker）| 不写 `~/.docker/config.json`、不碰 `networksetup` |
//! | 直连失败时用它重试一次 HTTP 请求 | 不默认改走代理 —— 直连能通就不绕路 |
//!
//! 三个平台的读法：
//!
//! | 平台 | 读哪儿 | 只读吗 |
//! |---|---|---|
//! | 任意 | `HTTPS_PROXY` / `HTTP_PROXY` / `ALL_PROXY` / `NO_PROXY` 环境变量 | 是 |
//! | macOS | `scutil --proxy`（系统设置里那一份） | 是（`--proxy` 只打印，不改） |
//! | Windows | 注册表 `HKCU\…\Internet Settings` 的 `ProxyEnable` / `ProxyServer` / `ProxyOverride`，用 `reg query` 读 | 是（`query` 子命令） |
//! | Linux | 只有环境变量（桌面环境各家一套，不猜） | 是 |
//!
//! 环境变量优先于系统设置：用户显式设了它，就是他的意思。
//!
//! ## 为什么解析函数都是纯函数
//!
//! 这台开发机与测试机都是 Linux，`scutil` 根本不存在。把「跑命令」和「解析输出」
//! 分开之后，**解析这一半在任何平台的 CI 上都能测**，而且测的就是用户 Mac 上
//! 那份真实输出（抄在单测里）。这是 I4 以来每一个「只有 mac 才炸」的缺陷
//! 教出来的写法。

use std::sync::{OnceLock, RwLock};
use std::time::Duration;

use serde::Serialize;

/// 一个代理端点。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Endpoint {
    /// `http` / `https` / `socks5`
    pub scheme: String,
    pub host: String,
    pub port: u16,
}

impl Endpoint {
    pub fn uri(&self) -> String {
        format!("{}://{}:{}", self.scheme, self.host, self.port)
    }
    /// 这个代理只监听本机吗。**虚拟机里访问不到这种代理**（见 [`Settings::for_vm`]）。
    pub fn loopback(&self) -> bool {
        is_loopback_host(&self.host)
    }
}

/// 这台机器上配着的代理。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub http: Option<Endpoint>,
    pub https: Option<Endpoint>,
    pub socks: Option<Endpoint>,
    /// 不走代理的主机清单（`NO_PROXY` / macOS 的 ExceptionsList / Windows 的 ProxyOverride）
    pub no_proxy: Vec<String>,
    /// 这份设置是从哪儿读来的：`env` / `macos-scutil` / `windows-registry` / `none`
    pub source: String,
}

impl Settings {
    pub fn any(&self) -> bool {
        self.http.is_some() || self.https.is_some() || self.socks.is_some()
    }

    /// 给人看的一句话。**没有代理时也要说得清**（红线 1：不编）。
    pub fn one_line(&self) -> String {
        if !self.any() {
            return "没有检测到系统代理".to_string();
        }
        let mut v = Vec::new();
        if let Some(e) = &self.https {
            v.push(format!("HTTPS {}:{}", e.host, e.port));
        }
        if let Some(e) = &self.http {
            v.push(format!("HTTP {}:{}", e.host, e.port));
        }
        if let Some(e) = &self.socks {
            v.push(format!("SOCKS {}:{}", e.host, e.port));
        }
        format!(
            "检测到你设置了网络代理（{}，来自{}），已沿用",
            v.join(" · "),
            self.source_cn()
        )
    }

    pub fn source_cn(&self) -> &'static str {
        match self.source.as_str() {
            "env" => "环境变量",
            "macos-scutil" => "系统设置",
            "windows-registry" => "系统设置",
            _ => "未知来源",
        }
    }

    /// 这个地址该用哪个代理。**本机地址一律直连**（健康检查打的就是 `127.0.0.1`，
    /// 让它绕一圈代理只会平白多一种失败方式）。
    pub fn for_url(&self, url: &str) -> Option<String> {
        let host = crate::http::host_of(url);
        let host = host.split(':').next().unwrap_or(&host).to_string();
        if is_loopback_host(&host) {
            return None;
        }
        if bypass(&host, &self.no_proxy) {
            return None;
        }
        let https = url.starts_with("https://");
        let pick = if https {
            self.https.as_ref().or(self.http.as_ref())
        } else {
            self.http.as_ref().or(self.https.as_ref())
        };
        // HTTP / HTTPS 代理都没有时才退到 SOCKS
        pick.or(self.socks.as_ref()).map(|e| e.uri())
    }

    /// 传给子进程的环境变量（colima / limactl / docker / brew 都认这几个）。
    ///
    /// 大小写各给一份：curl 认小写、Go 写的程序认大写，两边都给才不会漏。
    pub fn env_pairs(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        let mut put = |k: &str, v: &str| {
            out.push((k.to_uppercase(), v.to_string()));
            out.push((k.to_lowercase(), v.to_string()));
        };
        if let Some(e) = self.http.as_ref().or(self.https.as_ref()) {
            put("http_proxy", &e.uri());
        }
        if let Some(e) = self.https.as_ref().or(self.http.as_ref()) {
            put("https_proxy", &e.uri());
        }
        if let Some(e) = &self.socks {
            put("all_proxy", &e.uri());
        }
        // 本机地址永远不走代理 —— 我们自己的健康检查、docker 的 unix socket 都在这儿
        let mut no: Vec<String> = vec!["localhost".into(), "127.0.0.1".into(), "::1".into()];
        for n in &self.no_proxy {
            if !no.iter().any(|x| x.eq_ignore_ascii_case(n)) {
                no.push(n.clone());
            }
        }
        if self.any() {
            put("no_proxy", &no.join(","));
        }
        out
    }

    /// 传进虚拟机的那一份（`colima start --env`）。
    ///
    /// **只监听本机的代理传不进去**：虚拟机里的 `127.0.0.1` 是它自己，不是你的 Mac。
    /// 这种情况返回空，并由调用方如实说明 —— 不编一个「host.lima.internal」出来
    /// 赌它能通（Clash 这类客户端默认只绑 `127.0.0.1`，赌输了就是虚拟机里
    /// 连什么都拉不动，比不传更糟）。
    pub fn for_vm(&self) -> Vec<(String, String)> {
        if !self.any() || self.loopback_only() {
            return Vec::new();
        }
        self.env_pairs()
    }

    /// 配着的代理是不是**全都**只监听本机。
    pub fn loopback_only(&self) -> bool {
        let all = [&self.http, &self.https, &self.socks];
        let mut seen = false;
        for e in all.into_iter().flatten() {
            seen = true;
            if !e.loopback() {
                return false;
            }
        }
        seen
    }
}

/// 一个主机名在不在「不走代理」名单里。
///
/// 支持三种写法（三个平台的写法交集）：完全相同、`.suffix`、`*.suffix`。
/// `169.254/16` 这种 CIDR 写法**不支持**，会被当成普通字符串比 —— 与其猜一个
/// 半对的 CIDR 解析，不如让它不匹配（不匹配的后果只是多走一次代理）。
pub fn bypass(host: &str, list: &[String]) -> bool {
    let h = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if h.is_empty() {
        return false;
    }
    for raw in list {
        let p = raw.trim().trim_end_matches('.').to_ascii_lowercase();
        if p.is_empty() {
            continue;
        }
        if p == "*" {
            return true;
        }
        let suffix = p.strip_prefix("*.").or_else(|| p.strip_prefix('.'));
        match suffix {
            Some(s) => {
                if h == s || h.ends_with(&format!(".{s}")) {
                    return true;
                }
            }
            None => {
                if h == p || h.ends_with(&format!(".{p}")) {
                    return true;
                }
            }
        }
    }
    false
}

fn is_loopback_host(h: &str) -> bool {
    let h = h.trim().trim_matches(['[', ']']).to_ascii_lowercase();
    h == "localhost" || h == "127.0.0.1" || h == "::1" || h.starts_with("127.")
}

// ── 「代理救了一次场」的计数 ──────────────────────────────────────────────
//
// 顶部那一行「已自动解决 N 个问题」要把**自动完成的修复**如实算进去
// （I8 任务书一.4）。「直连不通、改走你的系统代理之后成功了」正是一次自动修复：
// 用户什么都没做，启动器自己换了一条路。
//
// 只记**主机名**，不记完整地址（地址里可能有 query 形式的凭证）。

fn rescued() -> &'static RwLock<Vec<String>> {
    static R: OnceLock<RwLock<Vec<String>>> = OnceLock::new();
    R.get_or_init(|| RwLock::new(Vec::new()))
}

/// 「直连不通、走代理成功了」记一笔。同一个主机只记一次。
pub fn mark_rescued(host: &str) {
    if let Ok(mut g) = rescued().write() {
        if !g.iter().any(|h| h == host) {
            g.push(host.to_string());
        }
    }
}

/// 这次安装里有几个站点是靠代理才通的。
pub fn rescued_hosts() -> Vec<String> {
    rescued().read().map(|g| g.clone()).unwrap_or_default()
}

/// 计数清零（每次安装开始时调一次）。
pub fn reset_rescued() {
    if let Ok(mut g) = rescued().write() {
        g.clear();
    }
}

// ── 读出来 ────────────────────────────────────────────────────────────────

fn cache() -> &'static RwLock<Option<Settings>> {
    static C: OnceLock<RwLock<Option<Settings>>> = OnceLock::new();
    C.get_or_init(|| RwLock::new(None))
}

/// 这台机器上的代理设置（进程内缓存一次）。
pub fn current() -> Settings {
    if let Ok(g) = cache().read() {
        if let Some(s) = g.as_ref() {
            return s.clone();
        }
    }
    let s = detect();
    if s.any() {
        crate::linfo!("{}（只读取，不修改你的网络设置）", s.one_line());
    }
    if let Ok(mut g) = cache().write() {
        *g = Some(s.clone());
    }
    s
}

pub fn invalidate() {
    if let Ok(mut g) = cache().write() {
        *g = None;
    }
}

/// 真去读一次。**环境变量优先**，没有再问系统。
pub fn detect() -> Settings {
    if let Some(s) = from_env() {
        return s;
    }
    if cfg!(target_os = "macos") {
        if let Some(text) = read_scutil() {
            let s = parse_scutil(&text);
            if s.any() {
                return s;
            }
        }
    }
    if cfg!(target_os = "windows") {
        if let Some(s) = read_windows() {
            if s.any() {
                return s;
            }
        }
    }
    Settings {
        source: "none".into(),
        ..Default::default()
    }
}

/// 环境变量那一份。一个都没有就返回 `None`（不是空 `Settings`）。
pub fn from_env() -> Option<Settings> {
    let get = |names: &[&str]| -> Option<String> {
        names
            .iter()
            .find_map(|n| std::env::var(n).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let http = get(&["HTTP_PROXY", "http_proxy"]);
    let https = get(&["HTTPS_PROXY", "https_proxy"]);
    let all = get(&["ALL_PROXY", "all_proxy"]);
    let no = get(&["NO_PROXY", "no_proxy"]).unwrap_or_default();
    if http.is_none() && https.is_none() && all.is_none() {
        return None;
    }
    let mut s = Settings {
        http: http.as_deref().and_then(|u| parse_uri(u, "http")),
        https: https.as_deref().and_then(|u| parse_uri(u, "http")),
        socks: None,
        no_proxy: split_list(&no),
        source: "env".into(),
    };
    if let Some(a) = all.as_deref().and_then(|u| parse_uri(u, "socks5")) {
        if a.scheme.starts_with("socks") {
            s.socks = Some(a);
        } else {
            if s.http.is_none() {
                s.http = Some(a.clone());
            }
            if s.https.is_none() {
                s.https = Some(a);
            }
        }
    }
    Some(s)
}

/// 把 `http://user:pass@host:port` / `host:port` 解析成端点。
///
/// **用户名密码原样留在 URI 里**（代理可能要认证），但这个结构不会进日志：
/// [`Settings::one_line`] 只打主机与端口。
pub fn parse_uri(raw: &str, default_scheme: &str) -> Option<Endpoint> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let (scheme, rest) = match raw.split_once("://") {
        Some((s, r)) => (s.to_ascii_lowercase(), r),
        None => (default_scheme.to_ascii_lowercase(), raw),
    };
    // 去掉路径与认证信息
    let rest = rest.split('/').next().unwrap_or(rest);
    let hostport = rest.rsplit('@').next().unwrap_or(rest);
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().ok()?),
        None => (
            hostport,
            match scheme.as_str() {
                "https" => 443u16,
                "socks5" | "socks5h" | "socks4" | "socks" => 1080,
                _ => 80,
            },
        ),
    };
    let host = host.trim().trim_matches(['[', ']']);
    if host.is_empty() || port == 0 {
        return None;
    }
    Some(Endpoint {
        scheme,
        host: host.to_string(),
        port,
    })
}

fn split_list(s: &str) -> Vec<String> {
    s.split([',', ';'])
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
        .collect()
}

/// 跑一次 `scutil --proxy`。**只读**：这个子命令只打印当前设置。
///
/// 走的是 [`crate::assist::guard::argv_readonly_probe`] 那道专门的门 ——
/// `scutil` 在守卫的禁用名单里（模型不许碰它），而这里的参数完全由代码写死。
fn read_scutil() -> Option<String> {
    let argv = vec!["/usr/sbin/scutil".to_string(), "--proxy".to_string()];
    if let Err(e) = crate::assist::guard::argv_readonly_probe(&argv) {
        crate::lwarn!("读系统代理被守卫拦下了：{}", e.msg);
        return None;
    }
    let r =
        crate::proc::run_timeout("/usr/sbin/scutil", &["--proxy"], Duration::from_secs(10)).ok()?;
    r.ok().then_some(r.stdout)
}

/// 解析 `scutil --proxy` 的输出。**纯函数**，任何平台的 CI 上都能测。
///
/// 真实输出长这样（用户 Mac 上那一份，端口与开关都是他的真实值）：
///
/// ```text
/// <dictionary> {
///   ExceptionsList : <array> {
///     0 : *.local
///     1 : 169.254/16
///   }
///   HTTPEnable : 1
///   HTTPPort : 7897
///   HTTPProxy : 127.0.0.1
///   HTTPSEnable : 1
///   HTTPSPort : 7897
///   HTTPSProxy : 127.0.0.1
///   SOCKSEnable : 1
///   SOCKSPort : 7897
///   SOCKSProxy : 127.0.0.1
/// }
/// ```
///
/// **`*Enable` 不是 1 的一律当没配**：系统里留着一个关掉的代理地址是常见的。
pub fn parse_scutil(text: &str) -> Settings {
    let mut map = std::collections::BTreeMap::<String, String>::new();
    let mut exceptions: Vec<String> = Vec::new();
    let mut in_exceptions = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("ExceptionsList") {
            in_exceptions = true;
            continue;
        }
        if in_exceptions {
            if t == "}" {
                in_exceptions = false;
                continue;
            }
            if let Some((_, v)) = t.split_once(':') {
                let v = v.trim();
                if !v.is_empty() {
                    exceptions.push(v.to_string());
                }
            }
            continue;
        }
        if let Some((k, v)) = t.split_once(':') {
            let (k, v) = (k.trim(), v.trim());
            if !k.is_empty() && !v.is_empty() && !v.starts_with('<') {
                map.insert(k.to_string(), v.to_string());
            }
        }
    }
    let pick = |prefix: &str, scheme: &str| -> Option<Endpoint> {
        if map.get(&format!("{prefix}Enable")).map(|s| s.as_str()) != Some("1") {
            return None;
        }
        let host = map.get(&format!("{prefix}Proxy"))?.trim().to_string();
        let port: u16 = map.get(&format!("{prefix}Port"))?.trim().parse().ok()?;
        if host.is_empty() || port == 0 {
            return None;
        }
        Some(Endpoint {
            scheme: scheme.to_string(),
            host,
            port,
        })
    };
    Settings {
        http: pick("HTTP", "http"),
        https: pick("HTTPS", "http"),
        socks: pick("SOCKS", "socks5"),
        no_proxy: exceptions,
        source: "macos-scutil".into(),
    }
}

/// Windows：`reg query` 读 Internet Settings。**query 是只读子命令。**
fn read_windows() -> Option<Settings> {
    const KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings";
    let argv = vec!["reg".to_string(), "query".to_string(), KEY.to_string()];
    if let Err(e) = crate::assist::guard::argv_readonly_probe(&argv) {
        crate::lwarn!("读系统代理被守卫拦下了：{}", e.msg);
        return None;
    }
    let r = crate::proc::run_timeout("reg", &["query", KEY], Duration::from_secs(10)).ok()?;
    r.ok().then(|| parse_windows_reg(&r.stdout))
}

/// 解析 `reg query …\Internet Settings` 的输出。**纯函数**。
///
/// ```text
///     ProxyEnable    REG_DWORD    0x1
///     ProxyServer    REG_SZ    http=127.0.0.1:7890;https=127.0.0.1:7890
///     ProxyOverride  REG_SZ    localhost;127.*;<local>
/// ```
///
/// `ProxyServer` 也可能是不带协议前缀的 `127.0.0.1:7890`，那种写法对所有协议生效。
pub fn parse_windows_reg(text: &str) -> Settings {
    let mut enable = false;
    let mut server = String::new();
    let mut over = String::new();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(name), Some(_ty)) = (it.next(), it.next()) else {
            continue;
        };
        let value = it.collect::<Vec<_>>().join(" ");
        match name {
            "ProxyEnable" => {
                enable = matches!(value.trim().to_ascii_lowercase().as_str(), "0x1" | "1")
            }
            "ProxyServer" => server = value.trim().to_string(),
            "ProxyOverride" => over = value.trim().to_string(),
            _ => {}
        }
    }
    let mut s = Settings {
        no_proxy: split_list(&over),
        source: "windows-registry".into(),
        ..Default::default()
    };
    if !enable || server.is_empty() {
        return s;
    }
    if server.contains('=') {
        for part in server.split(';') {
            let Some((k, v)) = part.split_once('=') else {
                continue;
            };
            match k.trim().to_ascii_lowercase().as_str() {
                "http" => s.http = parse_uri(v, "http"),
                "https" => s.https = parse_uri(v, "http"),
                "socks" => s.socks = parse_uri(v, "socks5"),
                _ => {}
            }
        }
    } else if let Some(e) = parse_uri(&server, "http") {
        s.http = Some(e.clone());
        s.https = Some(e);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 用户 Mac 上 2026-09-21 22:03 那份真实输出（端口 7897 是他的真实值）。
    const REAL_MAC: &str = "<dictionary> {
  ExceptionsList : <array> {
    0 : *.local
    1 : 169.254/16
  }
  FTPPassive : 1
  HTTPEnable : 1
  HTTPPort : 7897
  HTTPProxy : 127.0.0.1
  HTTPSEnable : 1
  HTTPSPort : 7897
  HTTPSProxy : 127.0.0.1
  SOCKSEnable : 1
  SOCKSPort : 7897
  SOCKSProxy : 127.0.0.1
}";

    #[test]
    fn 解析用户_mac_上那份真实输出() {
        let s = parse_scutil(REAL_MAC);
        assert_eq!(
            s.http,
            Some(Endpoint {
                scheme: "http".into(),
                host: "127.0.0.1".into(),
                port: 7897
            })
        );
        assert_eq!(s.https.as_ref().map(|e| e.port), Some(7897));
        assert_eq!(s.socks.as_ref().map(|e| e.scheme.as_str()), Some("socks5"));
        assert!(s.no_proxy.contains(&"*.local".to_string()));
        assert!(s.any());
        assert!(s.one_line().contains("7897"), "{}", s.one_line());
        assert!(s.one_line().contains("已沿用"), "{}", s.one_line());
    }

    /// **关掉的代理不算配着。** 系统里留着一个旧地址但开关是 0 是很常见的，
    /// 把它当成「配着」的后果是每个请求都往一个没人监听的端口上撞。
    #[test]
    fn 开关是_0_的一律当没配() {
        let text = "<dictionary> {
  HTTPEnable : 0
  HTTPPort : 7897
  HTTPProxy : 127.0.0.1
  HTTPSEnable : 0
  HTTPSProxy : 127.0.0.1
  HTTPSPort : 7897
}";
        let s = parse_scutil(text);
        assert!(!s.any(), "{s:?}");
        assert_eq!(s.one_line(), "没有检测到系统代理");
    }

    #[test]
    fn 端口读不到就不当成配着() {
        let s = parse_scutil("HTTPEnable : 1\nHTTPProxy : 127.0.0.1\n");
        assert!(!s.any(), "没有端口就不是一个能用的代理：{s:?}");
    }

    #[test]
    fn windows_注册表两种写法都认() {
        let per_proto = "
    ProxyEnable    REG_DWORD    0x1
    ProxyServer    REG_SZ    http=127.0.0.1:7890;https=10.0.0.5:8080
    ProxyOverride    REG_SZ    localhost;*.corp.example.com
";
        let s = parse_windows_reg(per_proto);
        assert_eq!(s.http.as_ref().unwrap().port, 7890);
        assert_eq!(s.https.as_ref().unwrap().host, "10.0.0.5");
        assert!(s.no_proxy.contains(&"*.corp.example.com".to_string()));

        let plain = "
    ProxyEnable    REG_DWORD    0x1
    ProxyServer    REG_SZ    127.0.0.1:7890
";
        let s = parse_windows_reg(plain);
        assert_eq!(s.http, s.https);
        assert_eq!(s.http.as_ref().unwrap().port, 7890);

        let off = "
    ProxyEnable    REG_DWORD    0x0
    ProxyServer    REG_SZ    127.0.0.1:7890
";
        assert!(!parse_windows_reg(off).any());
    }

    #[test]
    fn uri_解析() {
        assert_eq!(
            parse_uri("http://127.0.0.1:7897", "http"),
            Some(Endpoint {
                scheme: "http".into(),
                host: "127.0.0.1".into(),
                port: 7897
            })
        );
        // 没有协议前缀时用默认的
        assert_eq!(parse_uri("proxy.corp:3128", "http").unwrap().scheme, "http");
        // 认证信息不进 host
        assert_eq!(
            parse_uri("http://u:p@proxy.corp:3128", "http")
                .unwrap()
                .host,
            "proxy.corp"
        );
        // 没有端口时按协议给默认端口
        assert_eq!(parse_uri("https://proxy.corp", "http").unwrap().port, 443);
        assert_eq!(parse_uri("socks5://h", "http").unwrap().port, 1080);
        assert_eq!(parse_uri("", "http"), None);
        assert_eq!(parse_uri("http://:0", "http"), None);
    }

    /// **本机地址永远直连。** 健康检查打的就是 `http://127.0.0.1:3100`，
    /// 让它绕一圈代理只会平白多一种失败方式。
    #[test]
    fn 本机地址不走代理() {
        let s = parse_scutil(REAL_MAC);
        assert_eq!(s.for_url("http://127.0.0.1:3100/api/health"), None);
        assert_eq!(s.for_url("http://localhost:8100"), None);
        assert_eq!(
            s.for_url("https://github.com/x/y"),
            Some("http://127.0.0.1:7897".to_string())
        );
    }

    #[test]
    fn 例外清单里的主机不走代理() {
        let mut s = parse_scutil(REAL_MAC);
        s.no_proxy = vec![
            "*.local".into(),
            ".corp.example.com".into(),
            "foo.io".into(),
        ];
        assert_eq!(s.for_url("https://a.corp.example.com/x"), None);
        assert_eq!(s.for_url("https://foo.io/x"), None);
        assert_eq!(s.for_url("https://sub.foo.io/x"), None);
        assert!(s.for_url("https://foo.io.evil.com/x").is_some());
    }

    #[test]
    fn 环境变量里的_no_proxy_会被带进子进程() {
        let s = parse_scutil(REAL_MAC);
        let pairs = s.env_pairs();
        let get = |k: &str| {
            pairs
                .iter()
                .find(|(a, _)| a == k)
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        assert_eq!(get("HTTPS_PROXY"), "http://127.0.0.1:7897");
        assert_eq!(get("https_proxy"), "http://127.0.0.1:7897");
        assert_eq!(get("ALL_PROXY"), "socks5://127.0.0.1:7897");
        assert!(get("NO_PROXY").contains("127.0.0.1"), "{}", get("NO_PROXY"));
        assert!(get("NO_PROXY").contains("*.local"));
    }

    /// 只监听本机的代理**不往虚拟机里传** —— 虚拟机里的 127.0.0.1 是它自己。
    #[test]
    fn 只监听本机的代理不传进虚拟机() {
        let s = parse_scutil(REAL_MAC);
        assert!(s.loopback_only());
        assert!(s.for_vm().is_empty(), "传进去也连不上，不如不传");

        let lan = Settings {
            http: Some(Endpoint {
                scheme: "http".into(),
                host: "192.168.1.9".into(),
                port: 3128,
            }),
            source: "env".into(),
            ..Default::default()
        };
        assert!(!lan.loopback_only());
        assert!(!lan.for_vm().is_empty(), "局域网里的代理虚拟机能访问到");
    }

    #[test]
    fn 没有代理时什么都不给() {
        let s = Settings::default();
        assert!(!s.any());
        assert!(s.env_pairs().is_empty());
        assert!(s.for_vm().is_empty());
        assert_eq!(s.for_url("https://github.com"), None);
    }

    #[test]
    fn 名单匹配只认后缀不认包含() {
        let list = vec!["example.com".to_string()];
        assert!(bypass("example.com", &list));
        assert!(bypass("a.example.com", &list));
        assert!(!bypass("notexample.com", &list));
        assert!(!bypass("example.com.cn", &list));
        assert!(!bypass("", &list));
    }
}
