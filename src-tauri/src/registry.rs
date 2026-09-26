//! 镜像源候选与测速（技术方案 §5.5，按 M0 §4.2 与 `plan/国内镜像与下载源.md` 修正）。
//!
//! 方案原本列的三个源（阿里云 ACR / Docker Hub `agentpit` / GHCR）里前两个 M0 实测都是 `denied`；
//! 用户后来决定改用腾讯云个人版（`hkccr.ccs.tencentyun.com/agentpit`）。
//!
//! **只有真实探到的源才参与选择**（总控规则红线 1）：每个候选都走一遍标准的 registry v2
//! 匿名拉取流程（`GET /v2/` 拿 challenge → 换 token → `HEAD /v2/<repo>/manifests/<tag>`），
//! 只有返回 2xx 的才算数。探不到就如实标成「不可用」并写清 HTTP 状态 —— 不做假的测速界面。

use std::borrow::Cow;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::err::{AppError, AppResult, Code};

/// 一个镜像源候选。
///
/// 六个字段都是 [`Cow<'static, str>`] 而不是 `&'static str`（待办池 P1-22，I3 改的）。
/// 内置的两个源用 `Cow::Borrowed`，和以前一样是零成本的常量；**用户手填的自定义源
/// 用 `Cow::Owned`**，这样 [`custom`] 就不必再 `Box::leak` 一次。
///
/// 原来的写法是「字段定死 `&'static str` → 自定义源只能泄漏一个 `String` 进 `'static`」，
/// 设置页每保存一次就泄一份（`registry::custom` 一处 + `flow::prepare` 两处）。
/// 单次最多 253 字节、一个进程生命周期里以 KB 计，所以它一直排在 P1 末尾；
/// 但「泄漏」这种事只会越滚越多，换成 Cow 之后整个代码库里一个 `Box::leak` 都不剩。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// 稳定标识，写进 launcher.toml
    pub id: Cow<'static, str>,
    /// Hunter 四个自家镜像的前缀
    pub prefix: Cow<'static, str>,
    /// postgres / redis 的前缀。GHCR 上没有它们，走 Docker Hub 官方库
    pub base_prefix: Cow<'static, str>,
    /// 界面上显示的名字
    pub label: Cow<'static, str>,
    /// registry 主机名，探测用
    pub host: Cow<'static, str>,
    /// 探测用的仓库路径
    pub probe_repo: Cow<'static, str>,
}

/// 数据面探测取多少字节。256 KB：够区分「给数据」和「不给数据」，又不至于让探测本身变慢。
const PROBE_RANGE_BYTES: u64 = 256 * 1024;

/// 两个候选源。顺序即「其它条件相同时的偏好」。
///
/// 是 `static` 而不是 `const`：`Cow` 带 drop glue，`const X: &[T]` 那种写法要靠
/// 常量提升（promotion）才能拿到 `'static`，而带 drop glue 的类型不给提升。
/// `static` 本身就在静态存储区、永远不析构，正好。
pub static CANDIDATES: [Candidate; 2] = [
    Candidate {
        id: Cow::Borrowed("ghcr"),
        prefix: Cow::Borrowed("ghcr.io/agentpit-io"),
        base_prefix: Cow::Borrowed("docker.io/library"),
        label: Cow::Borrowed("GHCR · GitHub"),
        host: Cow::Borrowed("ghcr.io"),
        probe_repo: Cow::Borrowed("agentpit-io/hunter-community-web"),
    },
    Candidate {
        id: Cow::Borrowed("tencent"),
        prefix: Cow::Borrowed("hkccr.ccs.tencentyun.com/agentpit"),
        base_prefix: Cow::Borrowed("hkccr.ccs.tencentyun.com/agentpit"),
        label: Cow::Borrowed("腾讯云 · 香港"),
        host: Cow::Borrowed("hkccr.ccs.tencentyun.com"),
        probe_repo: Cow::Borrowed("agentpit/hunter-community-web"),
    },
];

pub fn by_id(id: &str) -> Option<&'static Candidate> {
    CANDIDATES.iter().find(|c| c.id == id)
}

/// 自定义镜像源前缀的合法形状：`主机[:端口]/一段或多段路径`。
///
/// 为什么要校验（I2 自审发现）：这个字符串是用户在设置页手打的，而它会被**原样**写进
/// 两个地方 —— `.env` 的 `HUNTER_REGISTRY=` 那一行，以及覆盖文件里 postgres / redis 的
/// `image:` 那两行。里面带上换行、空格或者引号，轻则渲染出一份 compose 解析不了的 YAML，
/// 重则在 `.env` 里多定义出一个环境变量。原来这里只 `trim()` 了一下就直接用。
///
/// 规则按 OCI 的引用语法收紧（只留真正会出现的字符），并且**不允许带 tag**
/// （`:1.2.0` 由我们自己拼）—— 主机名后面那个冒号只能跟纯数字端口。
pub fn prefix_ok(prefix: &str) -> bool {
    let p = prefix.trim().trim_end_matches('/');
    if p.is_empty() || p.len() > 253 {
        return false;
    }
    let mut parts = p.split('/');
    let Some(host) = parts.next() else {
        return false;
    };
    let (name, port) = match host.split_once(':') {
        Some((h, po)) => (h, Some(po)),
        None => (host, None),
    };
    if name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
    {
        return false;
    }
    if let Some(po) = port {
        if po.is_empty() || !po.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
    }
    // 至少要有一段路径（`ghcr.io` 光一个主机名拼不出我们要的镜像名）
    let mut any = false;
    for seg in parts {
        any = true;
        if seg.is_empty()
            || !seg
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
        {
            return false;
        }
    }
    any
}

/// 用户在设置里手填的自定义源。给自建镜像仓库的内网用户用。
///
/// 调用前务必先过 [`prefix_ok`]。
pub fn custom(prefix: &str) -> Candidate {
    let prefix = prefix.trim().trim_end_matches('/').to_string();
    let host = prefix.split('/').next().unwrap_or(&prefix).to_string();
    Candidate {
        id: Cow::Borrowed("custom"),
        prefix: Cow::Owned(prefix.clone()),
        base_prefix: Cow::Owned(prefix),
        label: Cow::Borrowed("自定义镜像源"),
        host: Cow::Owned(host),
        // 自定义源没有探测用的仓库路径 —— 我们不知道用户把镜像放在哪个 repo 下。
        // `probe` 见到空串会如实返回「这个源没有配探测用的仓库路径」，不假装探过。
        probe_repo: Cow::Borrowed(""),
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeResult {
    pub id: String,
    pub label: String,
    pub prefix: String,
    /// **元数据与数据面都探通了**才是 true。
    ///
    /// I16 之前这里只看 manifest。2026-09-26 一位 Windows 用户踩到了那个坑：
    /// 他机器上 GHCR 的 manifest 又快又通（于是被选中），可层数据在
    /// `pkg-containers.githubusercontent.com` 上一个字节都下不来，
    /// 升级因此卡了一个多小时。**探的东西必须和真正要下载的东西是同一样东西。**
    pub available: bool,
    /// 毫秒。探不到时也给耗时（它本身说明了链路状况）
    pub elapsed_ms: u64,
    /// 探不到的原因（HTTP 状态或网络错误），如实写
    pub detail: Option<String>,
    /// 层数据真的下下来了吗
    pub data_ok: bool,
    /// 下那一小段层数据花了多少毫秒。**这是实测值**，没测成就是 None
    pub data_ms: Option<u64>,
    /// 数据面下不来的原因
    pub data_detail: Option<String>,
}

/// 对全部候选源逐个探测，返回结果列表（按「可用优先、耗时升序」排好）。
pub fn probe_all(tag: &str, timeout: Duration) -> Vec<ProbeResult> {
    let mut out: Vec<ProbeResult> = CANDIDATES.iter().map(|c| probe(c, tag, timeout)).collect();
    // 排序按「能不能用 → 层数据下得多快 → 元数据多快」。
    // 中间那一项是 I16 加的：只比 manifest 的快慢，会把「元数据飞快、层下不动」的源排到第一。
    out.sort_by_key(|r| (!r.available, r.data_ms.unwrap_or(u64::MAX), r.elapsed_ms));
    out
}

pub fn probe(c: &Candidate, tag: &str, timeout: Duration) -> ProbeResult {
    let t0 = Instant::now();
    let r = fetch_manifest(&c.host, &c.probe_repo, tag, timeout);
    let elapsed_ms = t0.elapsed().as_millis() as u64;

    if let Err(e) = r {
        return ProbeResult {
            id: c.id.to_string(),
            label: c.label.to_string(),
            prefix: c.prefix.to_string(),
            available: false,
            elapsed_ms,
            detail: Some(e.msg),
            data_ok: false,
            data_ms: None,
            data_detail: None,
        };
    }

    // manifest 通了只说明「元数据这条路通」。层数据往往在另一个域名上
    // （GHCR 的层在 pkg-containers.githubusercontent.com），那条路单独会断。
    // 所以再真下一小段层数据，下得来才算这个源可用。
    let d0 = Instant::now();
    match probe_data_plane(c, tag, timeout) {
        Ok(()) => ProbeResult {
            id: c.id.to_string(),
            label: c.label.to_string(),
            prefix: c.prefix.to_string(),
            available: true,
            elapsed_ms,
            detail: None,
            data_ok: true,
            data_ms: Some(d0.elapsed().as_millis() as u64),
            data_detail: None,
        },
        Err(e) => ProbeResult {
            id: c.id.to_string(),
            label: c.label.to_string(),
            prefix: c.prefix.to_string(),
            available: false,
            elapsed_ms,
            detail: Some(format!("元数据能拿到，但层数据下不来：{}", e.msg)),
            data_ok: false,
            data_ms: None,
            data_detail: Some(e.msg),
        },
    }
}

/// 数据面探测：**真的下一小段层数据**。
///
/// 只取前 `PROBE_RANGE_BYTES` 个字节（Range 请求）—— 目的是区分
/// 「连得上、也给数据」和「连得上、就是不给数据」，不是测带宽，所以不必下整层。
/// 挑**最小的那一层**，省流量也省时间。
fn probe_data_plane(c: &Candidate, tag: &str, timeout: Duration) -> AppResult<()> {
    let digest = smallest_layer_digest(&c.host, &c.probe_repo, tag, oci_arch(), timeout)?;
    let url = format!("https://{}/v2/{}/blobs/{}", c.host, c.probe_repo, digest);
    let range = format!("bytes=0-{}", PROBE_RANGE_BYTES - 1);

    let first = crate::http::get(&url, &[("Range", &range)], timeout)?;
    let resp = if first.status == 401 {
        let challenge = first.header("www-authenticate").unwrap_or("").to_string();
        let token = fetch_token(&c.host, &c.probe_repo, &challenge, timeout)?;
        let auth = format!("Bearer {token}");
        crate::http::get(&url, &[("Range", &range), ("Authorization", &auth)], timeout)?
    } else {
        first
    };

    // 206 是按 Range 给的那一段；有些 registry 不认 Range，整层回 200，也算通
    if resp.status != 200 && resp.status != 206 {
        return Err(AppError::new(
            Code::PullFailed,
            format!("取层数据 → HTTP {}", resp.status),
        ));
    }
    if resp.body.is_empty() {
        return Err(AppError::new(
            Code::PullFailed,
            "取层数据 → 连上了，但一个字节都没给".to_string(),
        ));
    }
    Ok(())
}

/// 挑这个镜像里**最小的一层**的 digest。探数据面用，越小越省事。
fn smallest_layer_digest(
    host: &str,
    repo: &str,
    tag: &str,
    arch: &str,
    timeout: Duration,
) -> AppResult<String> {
    let body = get_manifest(host, repo, tag, timeout)?;
    let v: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| AppError::new(Code::PullFailed, format!("manifest 不是 JSON：{e}")))?;

    // 单架构 manifest 直接有 layers；多架构要先挑出本机这一份
    let layers_owned;
    let layers = if let Some(l) = v.get("layers").and_then(|l| l.as_array()) {
        l
    } else {
        let list = v
            .get("manifests")
            .and_then(|m| m.as_array())
            .ok_or_else(|| {
                AppError::new(
                    Code::PullFailed,
                    "manifest 里既没有 layers 也没有 manifests".to_string(),
                )
            })?;
        let digest = list
            .iter()
            .find(|m| {
                let p = m.get("platform");
                let os = p
                    .and_then(|p| p.get("os"))
                    .and_then(|x| x.as_str())
                    .unwrap_or("");
                let a = p
                    .and_then(|p| p.get("architecture"))
                    .and_then(|x| x.as_str())
                    .unwrap_or("");
                os == "linux" && a == arch
            })
            .and_then(|m| m.get("digest"))
            .and_then(|d| d.as_str())
            .ok_or_else(|| {
                AppError::new(Code::PullFailed, format!("这个镜像没有 linux/{arch} 的版本"))
            })?;
        let sub = get_manifest(host, repo, digest, timeout)?;
        let sv: serde_json::Value = serde_json::from_str(&sub).map_err(|e| {
            AppError::new(Code::PullFailed, format!("子 manifest 不是 JSON：{e}"))
        })?;
        layers_owned = sv
            .get("layers")
            .and_then(|l| l.as_array())
            .cloned()
            .ok_or_else(|| {
                AppError::new(Code::PullFailed, "子 manifest 里没有 layers".to_string())
            })?;
        &layers_owned
    };

    layers
        .iter()
        .filter_map(|l| {
            let d = l.get("digest")?.as_str()?;
            let size = l.get("size")?.as_u64()?;
            Some((size, d.to_string()))
        })
        .min_by_key(|(size, _)| *size)
        .map(|(_, d)| d)
        .ok_or_else(|| AppError::new(Code::PullFailed, "这个镜像一层都没有".to_string()))
}

/// 挑一个源：可用的里面最快的。一个都探不到时返回 `Err`，**不偷偷退回某个源**
/// —— 拉取必然会失败，与其让用户等几分钟再报错，不如现在就说清楚。
pub fn choose(tag: &str, timeout: Duration) -> AppResult<(&'static Candidate, Vec<ProbeResult>)> {
    let results = probe_all(tag, timeout);
    match results
        .iter()
        .find(|r| r.available)
        .and_then(|r| by_id(&r.id))
    {
        Some(c) => Ok((c, results)),
        None => {
            let detail = results
                .iter()
                .map(|r| {
                    format!(
                        "{}（{}）",
                        r.label,
                        r.detail.clone().unwrap_or_else(|| "无响应".into())
                    )
                })
                .collect::<Vec<_>>()
                .join("；");
            Err(AppError::new(
                Code::PullFailed,
                format!("{} 这个版本在所有候选镜像源上都拉不到：{detail}", tag),
            ))
        }
    }
}

/// 标准 registry v2 匿名拉取：`GET /v2/…/manifests/<tag>` 拿 `WWW-Authenticate` → 换 token → 再取一次。
///
/// 用 GET 而不是 HEAD：manifest 只有一两 KB，多这点流量换来的是**顺手验证了内容**
/// （能解析出 `mediaType` 才算真拿到），而不是只看一个状态码。
fn fetch_manifest(host: &str, repo: &str, tag: &str, timeout: Duration) -> AppResult<()> {
    if repo.is_empty() {
        return Err(AppError::new(
            Code::PullFailed,
            "这个源没有配探测用的仓库路径".to_string(),
        ));
    }
    let accept = "application/vnd.oci.image.index.v1+json, application/vnd.docker.distribution.manifest.list.v2+json, \
                  application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.v2+json";
    let url = format!("https://{host}/v2/{repo}/manifests/{tag}");

    let first = crate::http::get(&url, &[("Accept", accept)], timeout)?;
    if first.ok() {
        return check_body(&first.body);
    }
    if first.status != 401 {
        return Err(AppError::new(
            Code::PullFailed,
            format!("HTTP {}", first.status),
        ));
    }

    let challenge = first.header("www-authenticate").unwrap_or("").to_string();
    let token = fetch_token(host, repo, &challenge, timeout)?;
    let auth = format!("Bearer {token}");
    let second = crate::http::get(
        &url,
        &[("Accept", accept), ("Authorization", &auth)],
        timeout,
    )?;
    if second.ok() {
        check_body(&second.body)
    } else if second.status == 401 || second.status == 403 {
        Err(AppError::new(
            Code::PullFailed,
            format!(
                "HTTP {} · 这个源上没有这个镜像，或者仓库不是公开的",
                second.status
            ),
        ))
    } else {
        Err(AppError::new(
            Code::PullFailed,
            format!("HTTP {}", second.status),
        ))
    }
}

/// manifest 至少要能解析成 JSON 且带 `mediaType` 或 `manifests`/`layers`。
fn check_body(body: &str) -> AppResult<()> {
    let v: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| AppError::new(Code::PullFailed, format!("manifest 不是 JSON：{e}")))?;
    if v.get("mediaType").is_some() || v.get("manifests").is_some() || v.get("layers").is_some() {
        Ok(())
    } else {
        Err(AppError::new(
            Code::PullFailed,
            "响应看起来不是一个 manifest".to_string(),
        ))
    }
}

fn fetch_token(host: &str, repo: &str, challenge: &str, timeout: Duration) -> AppResult<String> {
    let realm =
        parse_challenge(challenge, "realm").unwrap_or_else(|| format!("https://{host}/token"));
    let service = parse_challenge(challenge, "service").unwrap_or_else(|| host.to_string());
    let url = format!("{realm}?service={service}&scope=repository:{repo}:pull");
    let resp = crate::http::get(&url, &[], timeout)?;
    if !resp.ok() {
        return Err(AppError::new(
            Code::PullFailed,
            format!("换 token 失败 HTTP {}", resp.status),
        ));
    }
    let v = resp
        .json()
        .ok_or_else(|| AppError::new(Code::PullFailed, "token 响应不是 JSON".to_string()))?;
    v.get("token")
        .or_else(|| v.get("access_token"))
        .and_then(|t| t.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| AppError::new(Code::PullFailed, "token 响应里没有 token 字段".to_string()))
}

/// 从 `Bearer realm="https://…",service="…"` 里取一个键。
pub fn parse_challenge(challenge: &str, key: &str) -> Option<String> {
    let needle = format!("{key}=\"");
    let start = challenge.find(&needle)? + needle.len();
    let rest = &challenge[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

// ── manifest 与镜像大小 ───────────────────────────────────────────────────

/// 取一次 manifest（带匿名 token 流程），返回响应体。
pub fn get_manifest(
    host: &str,
    repo: &str,
    reference: &str,
    timeout: Duration,
) -> AppResult<String> {
    let accept = "application/vnd.oci.image.index.v1+json, application/vnd.docker.distribution.manifest.list.v2+json, \
                  application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.v2+json";
    let url = format!("https://{host}/v2/{repo}/manifests/{reference}");
    let first = crate::http::get(&url, &[("Accept", accept)], timeout)?;
    if first.ok() {
        return Ok(first.body);
    }
    if first.status != 401 {
        return Err(AppError::new(
            Code::PullFailed,
            format!("{host}/{repo}:{reference} → HTTP {}", first.status),
        ));
    }
    let challenge = first.header("www-authenticate").unwrap_or("").to_string();
    let token = fetch_token(host, repo, &challenge, timeout)?;
    let auth = format!("Bearer {token}");
    let second = crate::http::get(
        &url,
        &[("Accept", accept), ("Authorization", &auth)],
        timeout,
    )?;
    if second.ok() {
        Ok(second.body)
    } else {
        Err(AppError::new(
            Code::PullFailed,
            format!("{host}/{repo}:{reference} → HTTP {}", second.status),
        ))
    }
}

/// 一个镜像在指定架构上的**压缩后**总字节数（各层 size 求和）。
///
/// 这是拉取进度条的分母：从第一秒起就是对的，不用等所有层的 `total` 都报上来（M0 §5.1 定的方案）。
/// OCI image index 里除 amd64/arm64 外还有 buildx 塞的 `unknown/unknown` attestation 条目，
/// 按 `platform.os == "linux"` 过滤掉（M0 §4.1 踩过）。
pub fn compressed_size(
    host: &str,
    repo: &str,
    tag: &str,
    arch: &str,
    timeout: Duration,
) -> AppResult<u64> {
    let body = get_manifest(host, repo, tag, timeout)?;
    let v: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| AppError::new(Code::PullFailed, format!("manifest 不是 JSON：{e}")))?;

    // 单架构 manifest：直接求和
    if let Some(layers) = v.get("layers").and_then(|l| l.as_array()) {
        return Ok(sum_layers(layers));
    }

    // 多架构 index：先挑出本机架构那一份，再取一次
    let list = v
        .get("manifests")
        .and_then(|m| m.as_array())
        .ok_or_else(|| {
            AppError::new(
                Code::PullFailed,
                "manifest 里既没有 layers 也没有 manifests".to_string(),
            )
        })?;
    let digest = list
        .iter()
        .find(|m| {
            let p = m.get("platform");
            let os = p
                .and_then(|p| p.get("os"))
                .and_then(|x| x.as_str())
                .unwrap_or("");
            let a = p
                .and_then(|p| p.get("architecture"))
                .and_then(|x| x.as_str())
                .unwrap_or("");
            os == "linux" && a == arch
        })
        .and_then(|m| m.get("digest"))
        .and_then(|d| d.as_str())
        .ok_or_else(|| {
            AppError::new(
                Code::PullFailed,
                format!("这个镜像没有 linux/{arch} 的版本"),
            )
        })?;

    let sub = get_manifest(host, repo, digest, timeout)?;
    let sv: serde_json::Value = serde_json::from_str(&sub)
        .map_err(|e| AppError::new(Code::PullFailed, format!("子 manifest 不是 JSON：{e}")))?;
    let layers = sv
        .get("layers")
        .and_then(|l| l.as_array())
        .ok_or_else(|| AppError::new(Code::PullFailed, "子 manifest 里没有 layers".to_string()))?;
    Ok(sum_layers(layers))
}

fn sum_layers(layers: &[serde_json::Value]) -> u64 {
    layers
        .iter()
        .filter_map(|l| l.get("size").and_then(|s| s.as_u64()))
        .sum()
}

/// 本机架构映射到 OCI 的写法。
pub fn oci_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "arm" => "arm",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// I2 自审：自定义源会**原样**进 `.env`（`HUNTER_REGISTRY=`）与覆盖文件的
    /// `image:` 两行。原来这里只 trim 了一下就用，带上换行或引号就能把 `.env` 写坏。
    #[test]
    fn 自定义镜像源的形状要校验() {
        for ok in [
            "registry.example.com/agentpit",
            "ghcr.io/agentpit-io",
            "10.0.0.2:5000/hunter",
            "hkccr.ccs.tencentyun.com/agentpit",
            "registry.example.com/team/sub",
            "registry.example.com/agentpit/", // 末尾斜杠会被 trim 掉
            // 末尾的空白（粘贴时最常见的那种）也放行：prefix_ok 与 custom()
            // 用的是同一句 trim，存下来的值里不会留着它
            "registry.example.com/agentpit\r",
            "  registry.example.com/agentpit  ",
        ] {
            assert!(prefix_ok(ok), "应当放行：{ok}");
        }
        // 上面那条「同一句 trim」必须是真的，否则校验过了、存下来的却是带空白的
        assert_eq!(
            custom("  registry.example.com/agentpit\r ").prefix,
            "registry.example.com/agentpit"
        );
        for bad in [
            "",
            "ghcr.io",                                  // 光一个主机名拼不出镜像名
            "registry.example.com/a b",                 // 空格
            "registry.example.com/a\nHUNTER_API_KEY=x", // 换行 —— 正是要挡的那一条
            "registry.example\r.com/a",                 // 中间的回车挡得住
            "registry.example.com/agentpit:1.2.0",      // 带 tag
            "registry.example.com:abc/a",               // 端口不是数字
            "registry.example.com//a",                  // 空路径段
            "registry.example.com/A",                   // 大写（OCI 的仓库名只许小写）
            "registry.example.com/a\"b",
        ] {
            assert!(!prefix_ok(bad), "应当拒绝：{bad:?}");
        }
    }

    #[test]
    fn 候选源的前缀与标签() {
        let ghcr = by_id("ghcr").expect("ghcr 应当在候选里");
        assert_eq!(ghcr.prefix, "ghcr.io/agentpit-io");
        // GHCR 上没有 postgres/redis，基础镜像要回 Docker Hub 官方库（M0 §4.1）
        assert_eq!(ghcr.base_prefix, "docker.io/library");

        let cn = by_id("tencent").expect("腾讯云应当在候选里");
        assert_eq!(cn.prefix, "hkccr.ccs.tencentyun.com/agentpit");
        // 国内源下 postgres/redis 也走镜像（plan/国内镜像与下载源.md）
        assert_eq!(cn.base_prefix, "hkccr.ccs.tencentyun.com/agentpit");
        assert_eq!(
            cn.label, "腾讯云 · 香港",
            "视觉稿写的「阿里云 · 杭州」已作废"
        );
    }

    /// 广州区的镜像仓库与下载桶**已于 2026-09-19 删除**
    /// （`plan/国内镜像与下载源.md` 第一节的警告框），代码里一处都不许再引用。
    ///
    /// 这条测试盯的是一个真实发生过的事故形状：M2 开工时读到的还是广州地址，
    /// 中途用户把基础设施换到了香港区 —— 地址写错的结果不是报错，而是
    /// 「国内用户拉镜像永远失败，还不知道为什么」。
    #[test]
    fn 候选源里不许出现已停用的广州区地址() {
        // 注意「广州区是 `ccr.`，香港区是 `hkccr.`」—— 前者是后者的后缀，
        // 用 `contains` 判会把正确的香港地址也误判成停用地址。所以这里按**主机名整体**比。
        // 下面三行带 `retired-mirror-ok` 标记：scripts/check-retired-mirrors.sh 会跳过它们。
        // 守卫自己必须把违规字符串原样写出来，否则没法比。
        const RETIRED_HOSTS: [&str; 2] = [
            "ccr.ccs.tencentyun.com", // 广州区镜像仓库（已删除）· retired-mirror-ok
            "registry.cn-hangzhou.aliyuncs.com", // 方案里那个实测不存在的阿里云 ACR · retired-mirror-ok
        ];
        const RETIRED_SUBSTRINGS: [&str; 1] = [
            "hunter-dl-1253756459", // 广州区下载桶（已删除，香港桶是 hunter-dl-hk-…）· retired-mirror-ok
        ];
        for c in CANDIDATES.iter() {
            for h in RETIRED_HOSTS {
                assert_ne!(c.host, h, "候选源 {} 用的是已停用的主机 {h}", c.id);
                assert!(
                    !c.prefix.starts_with(&format!("{h}/"))
                        && !c.base_prefix.starts_with(&format!("{h}/")),
                    "候选源 {} 的前缀落在已停用的主机 {h} 上",
                    c.id
                );
            }
            for r in RETIRED_SUBSTRINGS {
                assert!(
                    !c.prefix.contains(r) && !c.base_prefix.contains(r) && !c.host.contains(r),
                    "候选源 {} 里出现了已停用的地址 {r}",
                    c.id
                );
            }
        }
        // 腾讯云那个源的主机名必须是香港区的 hk 前缀，不能是广州区的裸 ccr
        let cn = by_id("tencent").expect("腾讯云应当在候选里");
        assert!(cn.host.starts_with("hkccr."), "实际是 {}", cn.host);
        assert!(!cn.label.contains("广州"), "文案里不许写广州：{}", cn.label);
        assert!(
            !cn.label.contains("阿里云"),
            "文案里不许写阿里云：{}",
            cn.label
        );
    }

    #[test]
    fn 解析_www_authenticate() {
        let c =
            r#"Bearer realm="https://ghcr.io/token",service="ghcr.io",scope="repository:x:pull""#;
        assert_eq!(
            parse_challenge(c, "realm").as_deref(),
            Some("https://ghcr.io/token")
        );
        assert_eq!(parse_challenge(c, "service").as_deref(), Some("ghcr.io"));
        assert_eq!(parse_challenge(c, "nope"), None);
        assert_eq!(
            parse_challenge(r#"Bearer realm="https://hkccr.ccs.tencentyun.com/service/token",service="token-service""#, "service").as_deref(),
            Some("token-service")
        );
    }

    #[test]
    fn 自定义源() {
        let c = custom("registry.example.com/team/ ");
        assert_eq!(c.prefix, "registry.example.com/team");
        assert_eq!(c.host, "registry.example.com");
        assert_eq!(c.base_prefix, "registry.example.com/team");
    }

    /// 造一个探测结果。`data_ms` 给 `None` 表示数据面没探成。
    fn pr(id: &str, available: bool, elapsed_ms: u64, data_ms: Option<u64>) -> ProbeResult {
        ProbeResult {
            id: id.into(),
            label: id.into(),
            prefix: id.into(),
            available,
            elapsed_ms,
            detail: None,
            data_ok: data_ms.is_some(),
            data_ms,
            data_detail: None,
        }
    }

    /// I16 的那个坑：GHCR 的 manifest 又快又通，层数据却一个字节都下不来。
    /// 光比 manifest 的快慢会把它排到第一，于是每次都选中它、每次都卡死。
    #[test]
    fn 元数据再快_层下不来也不能排第一() {
        let mut v = [
            // 元数据 20 毫秒就回了，但层数据没探成 —— 就是客户那台 Windows 上的 GHCR
            pr("ghcr", false, 20, None),
            // 元数据慢得多，可层是真下得下来
            pr("tencent", true, 800, Some(1200)),
        ];
        v.sort_by_key(|r| (!r.available, r.data_ms.unwrap_or(u64::MAX), r.elapsed_ms));
        assert_eq!(v[0].id, "tencent", "层下得下来的那个必须排第一");
        assert!(!v[1].available, "层下不来的源不算可用");
    }

    #[test]
    fn 排序把可用的排前面再按耗时() {
        let mut v = [
            pr("a", false, 10, None),
            pr("b", true, 900, Some(900)),
            pr("c", true, 300, Some(300)),
        ];
        v.sort_by_key(|r| (!r.available, r.data_ms.unwrap_or(u64::MAX), r.elapsed_ms));
        assert_eq!(
            v.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            vec!["c", "b", "a"]
        );
    }

    #[test]
    fn 按本机架构挑出多架构_index_里的那一份() {
        // M0 §4.1 记的真实结构：除 amd64/arm64 外还有两个 unknown/unknown 的 attestation 条目
        let idx = serde_json::json!({
            "mediaType": "application/vnd.oci.image.index.v1+json",
            "manifests": [
                {"digest": "sha256:amd", "platform": {"os": "linux", "architecture": "amd64"}},
                {"digest": "sha256:arm", "platform": {"os": "linux", "architecture": "arm64"}},
                {"digest": "sha256:att", "platform": {"os": "unknown", "architecture": "unknown"}}
            ]
        });
        let list = idx.get("manifests").unwrap().as_array().unwrap();
        let pick = |arch: &str| {
            list.iter()
                .find(|m| {
                    let p = m.get("platform").unwrap();
                    p["os"] == "linux" && p["architecture"] == arch
                })
                .map(|m| m["digest"].as_str().unwrap())
        };
        assert_eq!(pick("amd64"), Some("sha256:amd"));
        assert_eq!(pick("arm64"), Some("sha256:arm"));
        assert_eq!(
            pick("riscv64"),
            None,
            "没有的架构要如实返回 None，不能退而求其次拿 unknown 那条"
        );
    }

    /// 待办池 P1-22：自定义源原来会 `Box::leak` 一份字符串进 `'static`，
    /// 设置页每保存一次泄一点。换成 Cow 之后，自定义源的字段必须是 **Owned**
    /// （自己持有、跟着 Candidate 一起析构），内置两个源仍然是 **Borrowed**（零分配）。
    #[test]
    fn 自定义源自己持有字符串_内置源仍然是借用的() {
        let c = custom("registry.example.com/agentpit");
        assert!(
            matches!(c.prefix, Cow::Owned(_)),
            "自定义源的 prefix 必须是 Owned，否则又回到 Box::leak 那条路上了"
        );
        assert!(matches!(c.base_prefix, Cow::Owned(_)));
        assert!(matches!(c.host, Cow::Owned(_)));
        assert_eq!(c.prefix, "registry.example.com/agentpit");
        assert_eq!(c.host, "registry.example.com");
        assert_eq!(c.id, "custom");

        for b in CANDIDATES.iter() {
            assert!(
                matches!(b.prefix, Cow::Borrowed(_)),
                "内置源 {} 不该分配堆内存",
                b.id
            );
            assert!(matches!(b.label, Cow::Borrowed(_)));
        }
    }

    /// 尾部斜杠与首尾空白在构造时就吃掉 —— 它会原样进 `.env` 与覆盖文件。
    #[test]
    fn 自定义源的尾斜杠与空白在构造时就去掉() {
        let c = custom("  registry.example.com/agentpit/  ");
        assert_eq!(c.prefix, "registry.example.com/agentpit");
        assert_eq!(c.base_prefix, "registry.example.com/agentpit");
    }

    /// 自定义源没有探测用的 repo，`probe` 必须如实说「探不了」而不是假装可用（红线 1）。
    #[test]
    fn 自定义源探测时如实报告探不了() {
        let c = custom("registry.example.com/agentpit");
        let r = probe(&c, "1.2.0", Duration::from_millis(1));
        assert!(!r.available);
        assert!(
            r.detail.unwrap_or_default().contains("探测用的仓库路径"),
            "要说清是「没配探测路径」，不能含糊成一个网络错误"
        );
    }

    #[test]
    fn 层大小求和() {
        let layers = serde_json::json!([{"size": 100}, {"size": 250}, {"nosize": 1}]);
        assert_eq!(sum_layers(layers.as_array().unwrap()), 350);
    }

    #[test]
    fn 架构映射() {
        // 只断言映射表本身，不断言跑在哪台机器上
        assert!(matches!(oci_arch(), "amd64" | "arm64" | "arm" | _));
        assert_eq!(std::env::consts::ARCH == "x86_64", oci_arch() == "amd64");
    }
}
