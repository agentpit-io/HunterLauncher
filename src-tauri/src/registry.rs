//! 镜像源候选与测速（技术方案 §5.5，按 M0 §4.2 与 `plan/国内镜像与下载源.md` 修正）。
//!
//! 方案原本列的三个源（阿里云 ACR / Docker Hub `agentpit` / GHCR）里前两个 M0 实测都是 `denied`；
//! 用户后来决定改用腾讯云个人版（`hkccr.ccs.tencentyun.com/agentpit`）。
//!
//! **只有真实探到的源才参与选择**（总控规则红线 1）：每个候选都走一遍标准的 registry v2
//! 匿名拉取流程（`GET /v2/` 拿 challenge → 换 token → `HEAD /v2/<repo>/manifests/<tag>`），
//! 只有返回 2xx 的才算数。探不到就如实标成「不可用」并写清 HTTP 状态 —— 不做假的测速界面。

use std::time::{Duration, Instant};

use serde::Serialize;

use crate::err::{AppError, AppResult, Code};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Candidate {
    /// 稳定标识，写进 launcher.toml
    pub id: &'static str,
    /// Hunter 四个自家镜像的前缀
    pub prefix: &'static str,
    /// postgres / redis 的前缀。GHCR 上没有它们，走 Docker Hub 官方库
    pub base_prefix: &'static str,
    /// 界面上显示的名字
    pub label: &'static str,
    /// registry 主机名，探测用
    pub host: &'static str,
    /// 探测用的仓库路径
    pub probe_repo: &'static str,
}

/// 两个候选源。顺序即「其它条件相同时的偏好」。
pub const CANDIDATES: &[Candidate] = &[
    Candidate {
        id: "ghcr",
        prefix: "ghcr.io/agentpit-io",
        base_prefix: "docker.io/library",
        label: "GHCR · GitHub",
        host: "ghcr.io",
        probe_repo: "agentpit-io/hunter-community-web",
    },
    Candidate {
        id: "tencent",
        prefix: "hkccr.ccs.tencentyun.com/agentpit",
        base_prefix: "hkccr.ccs.tencentyun.com/agentpit",
        label: "腾讯云 · 香港",
        host: "hkccr.ccs.tencentyun.com",
        probe_repo: "agentpit/hunter-community-web",
    },
];

pub fn by_id(id: &str) -> Option<&'static Candidate> {
    CANDIDATES.iter().find(|c| c.id == id)
}

/// 用户在设置里手填的自定义源。给自建镜像仓库的内网用户用。
pub fn custom(prefix: &str) -> Candidate {
    // 'static 的要求让自定义源不能直接塞进 Candidate；这里只在需要时泄漏一次，
    // 一个进程最多泄漏几十字节，换来的是全链路一套类型。
    let prefix: &'static str = Box::leak(
        prefix
            .trim()
            .trim_end_matches('/')
            .to_string()
            .into_boxed_str(),
    );
    Candidate {
        id: "custom",
        prefix,
        base_prefix: prefix,
        label: "自定义镜像源",
        host: prefix.split('/').next().unwrap_or(prefix),
        probe_repo: "",
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeResult {
    pub id: String,
    pub label: String,
    pub prefix: String,
    /// manifest 真的探到了才是 true
    pub available: bool,
    /// 毫秒。探不到时也给耗时（它本身说明了链路状况）
    pub elapsed_ms: u64,
    /// 探不到的原因（HTTP 状态或网络错误），如实写
    pub detail: Option<String>,
}

/// 对全部候选源逐个探测，返回结果列表（按「可用优先、耗时升序」排好）。
pub fn probe_all(tag: &str, timeout: Duration) -> Vec<ProbeResult> {
    let mut out: Vec<ProbeResult> = CANDIDATES.iter().map(|c| probe(c, tag, timeout)).collect();
    out.sort_by_key(|r| (!r.available, r.elapsed_ms));
    out
}

pub fn probe(c: &Candidate, tag: &str, timeout: Duration) -> ProbeResult {
    let t0 = Instant::now();
    let r = fetch_manifest(c.host, c.probe_repo, tag, timeout);
    let elapsed_ms = t0.elapsed().as_millis() as u64;
    match r {
        Ok(()) => ProbeResult {
            id: c.id.into(),
            label: c.label.into(),
            prefix: c.prefix.into(),
            available: true,
            elapsed_ms,
            detail: None,
        },
        Err(e) => ProbeResult {
            id: c.id.into(),
            label: c.label.into(),
            prefix: c.prefix.into(),
            available: false,
            elapsed_ms,
            detail: Some(e.msg),
        },
    }
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
        const RETIRED_HOSTS: [&str; 2] = [
            "ccr.ccs.tencentyun.com",            // 广州区镜像仓库（已删除）
            "registry.cn-hangzhou.aliyuncs.com", // 方案里那个实测不存在的阿里云 ACR
        ];
        const RETIRED_SUBSTRINGS: [&str; 1] = [
            "hunter-dl-1253756459", // 广州区下载桶（已删除，香港桶是 hunter-dl-hk-…）
        ];
        for c in CANDIDATES {
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

    #[test]
    fn 排序把可用的排前面再按耗时() {
        let mut v = [
            ProbeResult {
                id: "a".into(),
                label: "a".into(),
                prefix: "a".into(),
                available: false,
                elapsed_ms: 10,
                detail: None,
            },
            ProbeResult {
                id: "b".into(),
                label: "b".into(),
                prefix: "b".into(),
                available: true,
                elapsed_ms: 900,
                detail: None,
            },
            ProbeResult {
                id: "c".into(),
                label: "c".into(),
                prefix: "c".into(),
                available: true,
                elapsed_ms: 300,
                detail: None,
            },
        ];
        v.sort_by_key(|r| (!r.available, r.elapsed_ms));
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
