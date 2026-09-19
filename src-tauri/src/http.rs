//! 薄薄一层 HTTP 客户端（基于 ureq，阻塞 I/O）。
//!
//! 为什么是阻塞的：启动器绝大部分工作是「跑一个 docker 子进程再读它的输出」，本来就是阻塞的；
//! 为了几个 HTTP 请求把整套代码染成 async 不划算。Tauri command 那边统一用
//! `spawn_blocking` 把这些调用挪到工作线程，界面不会被卡住。
//!
//! 这一层把 ureq 的类型全挡在里面，上层只看到 [`Resp`]。**非 2xx 不算错误**
//! —— registry 的 401 challenge、网关的 402 都是要读状态码与响应体来分类的。

use std::collections::HashMap;
use std::time::Duration;

use crate::err::{AppError, AppResult, Code};

pub struct Resp {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: String,
}

impl Resp {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
    pub fn json(&self) -> Option<serde_json::Value> {
        serde_json::from_str(&self.body).ok()
    }
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(|s| s.as_str())
    }
}

fn agent(timeout: Duration) -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        // 非 2xx 不当错误：状态码本身就是我们要的信息
        .http_status_as_error(false)
        .user_agent(concat!("HunterLauncher/", env!("CARGO_PKG_VERSION")))
        .build();
    config.into()
}

pub fn get(url: &str, headers: &[(&str, &str)], timeout: Duration) -> AppResult<Resp> {
    let a = agent(timeout);
    let mut req = a.get(url);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    finish(req.call(), url)
}

/// 下一个二进制文件（自更新下 `.deb` 用）。上限 200 MB —— 我们的安装包最大的
/// AppImage 也只有 75 MB 出头，收到比这还大的东西说明拿错了地址。
pub fn get_bytes(url: &str, timeout: Duration) -> AppResult<Vec<u8>> {
    const MAX: u64 = 200 * 1024 * 1024;
    let a = agent(timeout);
    let mut resp = a
        .get(url)
        .call()
        .map_err(|e| AppError::new(Code::Unknown, format!("请求 {} 失败：{e}", host_of(url))))?;
    let status = resp.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(AppError::new(
            Code::Unknown,
            format!("{} 返回 HTTP {status}", host_of(url)),
        ));
    }
    resp.body_mut()
        .with_config()
        .limit(MAX)
        .read_to_vec()
        .map_err(|e| {
            AppError::new(
                Code::Unknown,
                format!("读 {} 的响应体失败：{e}", host_of(url)),
            )
        })
}

pub fn post_json(
    url: &str,
    headers: &[(&str, &str)],
    body: &serde_json::Value,
    timeout: Duration,
) -> AppResult<Resp> {
    let a = agent(timeout);
    let mut req = a.post(url);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    finish(req.send_json(body), url)
}

fn finish(r: Result<http::Response<ureq::Body>, ureq::Error>, url: &str) -> AppResult<Resp> {
    let mut resp =
        r.map_err(|e| AppError::new(Code::Unknown, format!("请求 {} 失败：{e}", host_of(url))))?;
    let status = resp.status().as_u16();
    let mut headers = HashMap::new();
    for (k, v) in resp.headers().iter() {
        if let Ok(s) = v.to_str() {
            headers.insert(k.as_str().to_ascii_lowercase(), s.to_string());
        }
    }
    let body = resp.body_mut().read_to_string().unwrap_or_default();
    Ok(Resp {
        status,
        headers,
        body,
    })
}

/// 报错时只说主机名，不带完整 URL —— URL 里可能有 query 参数形式的凭证。
pub fn host_of(url: &str) -> String {
    url.split("://")
        .nth(1)
        .and_then(|s| s.split('/').next())
        .unwrap_or(url)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 主机名提取() {
        assert_eq!(host_of("https://ghcr.io/v2/x/manifests/1.2.0"), "ghcr.io");
        assert_eq!(
            host_of("https://hunter.agentpit.io/api/saas/llm/quota"),
            "hunter.agentpit.io"
        );
    }

    #[test]
    fn 响应头按小写查() {
        let mut headers = HashMap::new();
        headers.insert(
            "www-authenticate".to_string(),
            "Bearer realm=\"x\"".to_string(),
        );
        let r = Resp {
            status: 401,
            headers,
            body: String::new(),
        };
        assert!(!r.ok());
        assert_eq!(r.header("WWW-Authenticate"), Some("Bearer realm=\"x\""));
    }
}
