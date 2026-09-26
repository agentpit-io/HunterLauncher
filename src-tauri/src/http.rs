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

/// 和 [`Resp`] 一样，只是正文是**字节**。
///
/// 为什么要有它（I16 追加，真机上撞出来的）：[`get`] 用
/// `read_to_string().unwrap_or_default()` 读正文 —— 对着一个 gzip 压缩的镜像层，
/// 那一行永远返回**空串**。于是 `registry::probe_data_plane` 里那句
/// 「连上了，但一个字节都没给」对**每一个真实镜像源**都成立，
/// 选源会判定「所有候选源都拉不到」，一台干净机器上什么都装不了。
///
/// 现场：测试机 2026-09-26，`curl -r 0-262143` 从 ghcr 取同一层是
/// `http=206 size=32`（一个 32 字节的 gzip 空层），启动器却说一个字节都没给。
pub struct RespBytes {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

impl RespBytes {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(|s| s.as_str())
    }
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

/// 造一个 agent。`proxy` 是 `None` 时**显式不走代理**（不是「随 ureq 去读环境变量」）——
/// 直连与走代理是两次不同的尝试，哪一次在做什么必须是确定的（见 [`try_direct_then_proxy`]）。
fn agent(timeout: Duration, proxy: Option<&str>) -> ureq::Agent {
    let p = proxy.and_then(|u| match ureq::Proxy::new(u) {
        Ok(p) => Some(p),
        Err(e) => {
            crate::lwarn!("系统里配的代理地址解析不了（{e}），这一次按直连处理");
            None
        }
    });
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        // 非 2xx 不当错误：状态码本身就是我们要的信息
        .http_status_as_error(false)
        .user_agent(concat!("HunterLauncher/", env!("CARGO_PKG_VERSION")))
        .proxy(p)
        .build();
    config.into()
}

/// **先直连，直连不通再沿用用户自己的系统代理重试一次**（I8）。
///
/// 0.1.7 在用户 Mac 上就死在这条缺口上：他在系统设置里配了 `127.0.0.1:7897`，
/// 直连 github.com 十秒超时、走代理两秒 200，而启动器（以及它拉起来的 colima）
/// 谁都没有去看一眼那份设置。
///
/// 为什么是「先直连」而不是「有代理就走代理」：系统里留着一个**已经不工作**的
/// 代理地址是很常见的（换了网络、关了客户端）。直连能通就不绕路，绕路只在
/// 直连真的失败时发生 —— 这样两种现场都能装上。
///
/// 只有**传输层失败**才重试：HTTP 404 / 401 是答案，不是故障，不会触发重试。
///
/// ## 同一个主机只交一次学费
///
/// 实测（测试机，2026-09-21 23:15，直连 GitHub 被 DROP + 环境里配着代理）：
/// 探一次 GHCR 花了 **60.7 秒** —— 取 token、取 manifest 各自先直连等满 20 秒
/// 超时再走代理。第一次那 20 秒是必要的（得先知道直连不通），后面每一次都交
/// 就纯是浪费用户的时间。
///
/// 所以主机一旦被记进 [`crate::netproxy::mark_rescued`]，这一次安装里对它
/// **不再试直连**，直接走代理。作用域只有这一次安装。
fn try_direct_then_proxy<T>(
    url: &str,
    timeout: Duration,
    mut f: impl FnMut(&ureq::Agent) -> AppResult<T>,
) -> AppResult<T> {
    let host = host_of(url);
    // 这一次安装里已经证明过「直连不通、代理能通」的主机：直接走代理
    if crate::netproxy::prefer_proxy_for(&host) {
        if let Some(proxy) = crate::netproxy::current().for_url(url) {
            return f(&agent(timeout, Some(&proxy)));
        }
    }
    let direct = f(&agent(timeout, None));
    let Err(e) = direct else {
        return direct;
    };
    let Some(proxy) = crate::netproxy::current().for_url(url) else {
        return Err(e);
    };
    crate::linfo!(
        "直连 {host} 失败（{}），改用你在系统里设置的网络代理重试一次",
        e.msg
    );
    let r = f(&agent(timeout, Some(&proxy)));
    match r {
        Ok(v) => {
            crate::netproxy::mark_rescued(&host);
            Ok(v)
        }
        Err(e2) => {
            // 两条路都走不通时，把**两次**的原话都给出来 —— 只报后一次会让人
            // 以为「代理坏了」，而多数时候是两边都不通
            Err(AppError::new(
                e2.code,
                format!("直连失败：{}；走你的系统代理也失败：{}", e.msg, e2.msg),
            ))
        }
    }
}

pub fn get(url: &str, headers: &[(&str, &str)], timeout: Duration) -> AppResult<Resp> {
    try_direct_then_proxy(url, timeout, |a| {
        let mut req = a.get(url);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        finish(req.call(), url)
    })
}

/// **按字节**取一段（探层数据用）。带自定义请求头，上限由调用方给。
///
/// 和 [`get`] 的差别只有一处，但那一处是要命的：正文读成 `Vec<u8>`。
/// 二进制内容用 [`get`] 取，正文会静悄悄变成空串（见 [`RespBytes`] 的注释）。
///
/// 非 2xx **不算错误** —— registry 的 401 challenge 要读状态码与响应头来换 token。
pub fn get_bytes_headers(
    url: &str,
    headers: &[(&str, &str)],
    timeout: Duration,
    max: u64,
) -> AppResult<RespBytes> {
    try_direct_then_proxy(url, timeout, |a| {
        let mut req = a.get(url);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let mut resp = req.call().map_err(|e| {
            AppError::new(Code::Unknown, format!("请求 {} 失败：{e}", host_of(url)))
        })?;
        let status = resp.status().as_u16();
        let mut hs = HashMap::new();
        for (k, v) in resp.headers().iter() {
            if let Ok(s) = v.to_str() {
                hs.insert(k.as_str().to_ascii_lowercase(), s.to_string());
            }
        }
        let body = resp
            .body_mut()
            .with_config()
            .limit(max)
            .read_to_vec()
            .map_err(|e| {
                AppError::new(
                    Code::Unknown,
                    format!("读 {} 的响应体失败：{e}", host_of(url)),
                )
            })?;
        Ok(RespBytes {
            status,
            headers: hs,
            body,
        })
    })
}

/// 下一个二进制文件（自更新下 `.deb` 用）。上限 200 MB —— 我们的安装包最大的
/// AppImage 也只有 75 MB 出头，收到比这还大的东西说明拿错了地址。
pub fn get_bytes(url: &str, timeout: Duration) -> AppResult<Vec<u8>> {
    const MAX: u64 = 200 * 1024 * 1024;
    try_direct_then_proxy(url, timeout, |a| {
        let mut resp = a.get(url).call().map_err(|e| {
            AppError::new(Code::Unknown, format!("请求 {} 失败：{e}", host_of(url)))
        })?;
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
    })
}

/// 只问头、不取正文。测速与「这个地址上的文件多大」都用它。
///
/// 为什么不拿 `get` 凑合：`get` 会把正文读成 `String`，对着一个几十 MB 的二进制文件
/// 既浪费带宽又没有意义（而且读不成 UTF-8 时正文会被当成空的，看起来还「成功」了）。
pub fn head(url: &str, timeout: Duration) -> AppResult<Resp> {
    try_direct_then_proxy(url, timeout, |a| {
        let mut resp = a.head(url).call().map_err(|e| {
            AppError::new(Code::Unknown, format!("请求 {} 失败：{e}", host_of(url)))
        })?;
        let status = resp.status().as_u16();
        let mut headers = HashMap::new();
        for (k, v) in resp.headers().iter() {
            if let Ok(s) = v.to_str() {
                headers.insert(k.as_str().to_ascii_lowercase(), s.to_string());
            }
        }
        let _ = resp.body_mut();
        Ok(Resp {
            status,
            headers,
            body: String::new(),
        })
    })
}

/// 把一个大文件流式下到磁盘，边下边报**真实字节数**。
///
/// 为什么不复用 [`get_bytes`]：内置运行时的四个包加起来将近 100 MB，
/// 整个读进内存再写盘是白白占一份内存；而且进度条要的是「现在下到第几个字节」，
/// 一次性读完只能在结束时跳一下（红线 1 意义上的假进度）。
///
/// `max` 是上限，超了就**中断并报错**（清单里写着每个文件多大，收到更大的东西
/// 说明拿错了地址）。`on_progress(已下字节, 总字节或 None)` 由调用方节流。
pub fn download_to_file(
    url: &str,
    dest: &std::path::Path,
    timeout: Duration,
    max: u64,
    cancel: &dyn Fn() -> bool,
    on_progress: &mut dyn FnMut(u64, Option<u64>),
) -> AppResult<u64> {
    use std::io::{Read, Write};

    // 下载这一条也走「先直连、不通再沿用系统代理」那条路（I8）。
    // 重试时**整个文件重下**：断点续传要上游支持 Range，而这里的每个文件
    // 最后都要按清单里的 sha256 / sha512 对一遍 —— 拼出来的半截文件过不了那一关。
    try_direct_then_proxy(url, timeout, move |a| {
        let mut resp = a.get(url).call().map_err(|e| {
            AppError::new(Code::Unknown, format!("请求 {} 失败：{e}", host_of(url)))
        })?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(AppError::new(
                Code::Unknown,
                format!("{} 返回 HTTP {status}", host_of(url)),
            ));
        }
        let total = resp
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok());
        if let Some(t) = total {
            if t > max {
                return Err(AppError::new(
                    Code::Unknown,
                    format!(
                        "{} 说这个文件有 {t} 字节，比清单里写的上限 {max} 还大，不下了。",
                        host_of(url)
                    ),
                ));
            }
        }
        if let Some(d) = dest.parent() {
            std::fs::create_dir_all(d).map_err(|e| {
                AppError::new(
                    Code::ConfigWrite,
                    format!("建目录 {} 失败：{e}", d.display()),
                )
            })?;
        }
        let mut f = std::fs::File::create(dest).map_err(|e| {
            AppError::new(
                Code::ConfigWrite,
                format!("建文件 {} 失败：{e}", dest.display()),
            )
        })?;
        let mut reader = resp.body_mut().with_config().limit(max + 1).reader();
        let mut buf = vec![0u8; 256 * 1024];
        let mut got: u64 = 0;
        loop {
            if cancel() {
                let _ = std::fs::remove_file(dest);
                return Err(AppError::new(Code::Unknown, "下载被取消了。".to_string()));
            }
            let n = reader.read(&mut buf).map_err(|e| {
                AppError::new(
                    Code::Unknown,
                    format!("读 {} 的响应体失败：{e}", host_of(url)),
                )
            })?;
            if n == 0 {
                break;
            }
            got += n as u64;
            if got > max {
                let _ = std::fs::remove_file(dest);
                return Err(AppError::new(
                    Code::Unknown,
                    format!(
                        "{} 传回来的内容超过了清单上限 {max} 字节，已中断。",
                        host_of(url)
                    ),
                ));
            }
            f.write_all(&buf[..n]).map_err(|e| {
                AppError::new(
                    Code::ConfigWrite,
                    format!("写 {} 失败：{e}", dest.display()),
                )
            })?;
            on_progress(got, total);
        }
        f.flush().ok();
        Ok(got)
    })
}

pub fn post_json(
    url: &str,
    headers: &[(&str, &str)],
    body: &serde_json::Value,
    timeout: Duration,
) -> AppResult<Resp> {
    try_direct_then_proxy(url, timeout, |a| {
        let mut req = a.post(url);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        finish(req.send_json(body), url)
    })
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
