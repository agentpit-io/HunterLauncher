//! 离线包导入 / 导出（技术方案 §9「支持『离线包』：`hunter-images-<tag>.tar`（`docker save`），
//! 启动器提供『从文件导入』入口，给内网私募用」）。
//!
//! ## 导入这一步到底做了什么
//!
//! `docker load -i <tar>` 把镜像灌进本机的镜像存储，然后**从 docker 自己报出来的镜像名**
//! 反推出这一包是哪个源、哪个版本的：
//!
//! ```text
//! Loaded image: ghcr.io/agentpit-io/hunter-community-web:1.2.0
//!               └────────┬────────┘└────────┬────────┘ └─┬─┘
//!                  registry 前缀        服务名          tag
//! ```
//!
//! 六个服务都齐了，就把 `HUNTER_REGISTRY` 与 `HUNTER_VERSION` 写成包里的那一套，
//! 安装流程**跳过拉取**直接去 `up -d`。
//!
//! ## 三条约束
//!
//! 1. **不猜**。包里少哪个服务就如实列出来（红线 1），不会"看起来差不多就当它行"。
//!    少了就老实回到在线拉取，缺的那几个照样要从网上下。
//! 2. **导入后要复核**。`docker load` 说成功不等于六个镜像都在：用
//!    `docker image inspect` 逐个确认，确认不了的算缺。
//! 3. **参数数组**（红线 3）。tar 路径由用户选，绝不进 shell 字符串。
//!
//! ## 导出
//!
//! 反过来的 `docker save` 也做了（[`export`]）—— 没有它的话，"离线包"这个功能
//! 只有消费端没有生产端，内网用户拿不到那个 tar。有网的机器上跑一次
//! `hunter-launcher --export-images hunter-images-1.2.0.tar`，拷进内网就能用。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::config::{self, ImageSpec};
use crate::err::{AppError, AppResult, Code};
use crate::runtime::which;
use crate::{linfo, lwarn, proc};

/// `docker load` 的上限。几百 MB 到几 GB 的 tar，慢盘上也够。
const LOAD_TIMEOUT: Duration = Duration::from_secs(1800);
/// `docker save` 的上限。
const SAVE_TIMEOUT: Duration = Duration::from_secs(1800);
const INSPECT_TIMEOUT: Duration = Duration::from_secs(30);

/// 六个服务名。顺序与 [`config::images`] 一致。
pub const SERVICES: [&str; 6] = ["web", "api", "opencode", "llm-shim", "postgres", "redis"];

/// 一次导入的结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    /// tar 文件路径
    pub path: String,
    /// tar 有多大
    pub bytes: u64,
    pub seconds: u64,
    /// `docker load` 报出来的全部镜像引用（原样）
    pub loaded: Vec<String>,
    /// 六个服务里，**导入后经 `docker image inspect` 确认在本机**的那些
    pub matched: Vec<MatchedImage>,
    /// 还缺的服务名。非空就说明这个包不完整，缺的还得联网拉
    pub missing: Vec<String>,
    /// 从包里反推出来的镜像前缀（四个 hunter 镜像的）
    pub registry_prefix: Option<String>,
    /// postgres / redis 的前缀
    pub base_prefix: Option<String>,
    /// 从包里反推出来的 Hunter 版本
    pub tag: Option<String>,
    /// 够不够跳过拉取
    pub complete: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchedImage {
    pub service: String,
    pub reference: String,
    /// 本机镜像的字节数（`docker image inspect` 的 Size，未压缩）
    pub bytes: Option<u64>,
}

/// 从 `docker load` 的输出里认出镜像引用。
///
/// docker 的输出有两种形状，都要认：
/// ```text
/// Loaded image: ghcr.io/agentpit-io/hunter-community-web:1.2.0
/// Loaded image ID: sha256:abc…            ← 这种没有名字，认不出来，跳过
/// ```
pub fn parse_loaded(output: &str) -> Vec<String> {
    let mut v = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Loaded image: ") {
            let r = rest.trim();
            if !r.is_empty() && !v.contains(&r.to_string()) {
                v.push(r.to_string());
            }
        }
    }
    v
}

/// 一个镜像引用属于哪个服务。
///
/// `ghcr.io/agentpit-io/hunter-community-api:1.2.0` → `Some(("api", "ghcr.io/agentpit-io", "1.2.0"))`
/// `postgres:16-alpine` → `Some(("postgres", "docker.io/library", "16-alpine"))`
pub fn classify(reference: &str) -> Option<(String, String, String)> {
    let (name, tag) = split_tag(reference)?;
    let last = name.rsplit('/').next()?;
    let prefix = match name.rsplit_once('/') {
        Some((p, _)) => p.to_string(),
        // 不带任何前缀的官方库镜像（`postgres:16-alpine`）
        None => "docker.io/library".to_string(),
    };
    if let Some(svc) = last.strip_prefix("hunter-community-") {
        if SERVICES.contains(&svc) {
            return Some((svc.to_string(), prefix, tag));
        }
        return None;
    }
    if last == "postgres" || last == "redis" {
        return Some((last.to_string(), prefix, tag));
    }
    None
}

/// `a/b:1.2.0` → `("a/b", "1.2.0")`。**要小心带端口的主机名**（`host:5000/x:1.0`）：
/// 冒号要从最后一个 `/` 之后找。
fn split_tag(reference: &str) -> Option<(String, String)> {
    let r = reference.trim();
    if r.is_empty() {
        return None;
    }
    let slash = r.rfind('/').map(|i| i + 1).unwrap_or(0);
    match r[slash..].rfind(':') {
        Some(i) => Some((r[..slash + i].to_string(), r[slash + i + 1..].to_string())),
        // 没有 tag 就是 latest（docker 的规则），但离线包里出现这种基本是打包时漏了
        None => Some((r.to_string(), "latest".to_string())),
    }
}

/// 导入一个离线包。
pub fn import(tar: &Path, mut note: impl FnMut(&str)) -> AppResult<ImportResult> {
    if !tar.exists() {
        return Err(AppError::new(
            Code::PullFailed,
            format!("找不到这个文件：{}", tar.display()),
        ));
    }
    let bytes = std::fs::metadata(tar).map(|m| m.len()).unwrap_or(0);
    if bytes == 0 {
        return Err(AppError::new(
            Code::PullFailed,
            format!("{} 是个空文件", tar.display()),
        ));
    }
    let path_s = tar.to_string_lossy().into_owned();
    note(&format!(
        "正在从 {} 导入镜像（{}）…",
        path_s,
        crate::flow::human_bytes(bytes)
    ));
    linfo!("离线导入：{path_s}（{bytes} 字节）");

    let t0 = Instant::now();
    // 红线 3：参数数组。`-i` 让 docker 自己读文件，不走 shell 重定向
    let r = proc::run_timeout(&which::docker_bin(), &["load", "-i", &path_s], LOAD_TIMEOUT)?;
    if !r.ok() {
        return Err(AppError::new(
            Code::PullFailed,
            format!("docker load 失败：{}", r.err_line()),
        ));
    }
    let seconds = t0.elapsed().as_secs();
    // docker load 的进度在 stderr，结果行（`Loaded image: …`）在 stdout
    let loaded = parse_loaded(&format!("{}\n{}", r.stdout, r.stderr));
    if loaded.is_empty() {
        return Err(AppError::new(
            Code::PullFailed,
            "docker load 成功了，但没有报出任何镜像名。这个 tar 可能是用 `docker export` 而不是 `docker save` 做的。"
                .to_string(),
        ));
    }
    for l in &loaded {
        note(&format!("  导入 {l}"));
    }

    // 反推这一包是哪个源、哪个版本
    let mut hunter_prefix: Option<String> = None;
    let mut base_prefix: Option<String> = None;
    let mut tag: Option<String> = None;
    let mut by_service: Vec<(String, String)> = Vec::new();
    for reference in &loaded {
        if let Some((svc, prefix, t)) = classify(reference) {
            if svc == "postgres" || svc == "redis" {
                base_prefix.get_or_insert(prefix);
            } else {
                hunter_prefix.get_or_insert(prefix);
                tag.get_or_insert(t);
            }
            by_service.push((svc, reference.clone()));
        }
    }

    // 复核：docker load 说成功不等于镜像真在（约束 2）
    let mut matched: Vec<MatchedImage> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    for svc in SERVICES {
        match by_service.iter().find(|(s, _)| s == svc) {
            Some((_, reference)) => match inspect_size(reference) {
                Some(n) => matched.push(MatchedImage {
                    service: svc.to_string(),
                    reference: reference.clone(),
                    bytes: Some(n),
                }),
                None => {
                    lwarn!("离线包里有 {reference}，但 docker image inspect 找不到它");
                    missing.push(svc.to_string());
                }
            },
            None => missing.push(svc.to_string()),
        }
    }

    let complete = missing.is_empty() && tag.is_some() && hunter_prefix.is_some();
    if complete {
        note(&format!(
            "六个镜像齐了 · 版本 v{} · 源 {} · 用时 {seconds} 秒。这一步之后不用再拉镜像。",
            tag.clone().unwrap_or_default(),
            hunter_prefix.clone().unwrap_or_default()
        ));
    } else {
        note(&format!(
            "这个包不完整，还缺：{}。缺的那几个仍然要联网拉。",
            missing.join("、")
        ));
    }
    linfo!(
        "离线导入完成：{} 个镜像 · 缺 {:?} · tag={:?} · 用时 {seconds} 秒",
        matched.len(),
        missing,
        tag
    );

    Ok(ImportResult {
        path: path_s,
        bytes,
        seconds,
        loaded,
        matched,
        missing,
        registry_prefix: hunter_prefix,
        base_prefix,
        tag,
        complete,
    })
}

/// 本机有没有这个镜像，有的话多大。没有就是 `None`（**不区分"没有"与"读不出来"**，
/// 两种情况下都不能当它存在）。
pub fn inspect_size(reference: &str) -> Option<u64> {
    let r = proc::run_timeout(
        &which::docker_bin(),
        &["image", "inspect", reference, "--format", "{{.Size}}"],
        INSPECT_TIMEOUT,
    )
    .ok()?;
    if !r.ok() {
        return None;
    }
    r.stdout.trim().parse::<u64>().ok()
}

/// 一组镜像在本机齐了没有。安装流程靠它决定「还要不要拉」。
pub fn all_present(specs: &[ImageSpec]) -> (bool, Vec<String>) {
    let mut missing = Vec::new();
    for s in specs {
        if inspect_size(&s.reference).is_none() {
            missing.push(s.service.clone());
        }
    }
    (missing.is_empty(), missing)
}

/// 把本机的六个镜像打成一个离线包（`docker save`）。给做离线包的人用。
pub fn export(
    out: &Path,
    registry_prefix: &str,
    base_prefix: &str,
    tag: &str,
    mut note: impl FnMut(&str),
) -> AppResult<(PathBuf, u64, u64)> {
    let specs = config::images(registry_prefix, base_prefix, tag);
    let (ok, missing) = all_present(&specs);
    if !ok {
        return Err(AppError::new(
            Code::PullFailed,
            format!(
                "本机没有这几个镜像，先拉下来再打包：{}（用 --pull-only）",
                missing.join("、")
            ),
        ));
    }
    if let Some(parent) = out.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let out_s = out.to_string_lossy().into_owned();
    let mut args: Vec<&str> = vec!["save", "-o", &out_s];
    for s in &specs {
        args.push(&s.reference);
    }
    note(&format!("正在把 6 个镜像打包到 {out_s}…"));
    let t0 = Instant::now();
    let r = proc::run_timeout(&which::docker_bin(), &args, SAVE_TIMEOUT)?;
    if !r.ok() {
        return Err(AppError::new(
            Code::PullFailed,
            format!("docker save 失败：{}", r.err_line()),
        ));
    }
    let bytes = std::fs::metadata(out).map(|m| m.len()).unwrap_or(0);
    let seconds = t0.elapsed().as_secs();
    note(&format!(
        "打包完成：{out_s}（{}，用时 {seconds} 秒）",
        crate::flow::human_bytes(bytes)
    ));
    linfo!("离线包已导出：{out_s} · {bytes} 字节 · {seconds} 秒");
    Ok((out.to_path_buf(), bytes, seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 认得出_docker_load_的输出() {
        let out = "\
Loaded image: ghcr.io/agentpit-io/hunter-community-web:1.2.0
Loaded image: ghcr.io/agentpit-io/hunter-community-api:1.2.0
Loaded image ID: sha256:0123456789abcdef
Loaded image: postgres:16-alpine
";
        let v = parse_loaded(out);
        assert_eq!(v.len(), 3, "没有名字的那一行认不出来，跳过它：{v:?}");
        assert!(v.contains(&"postgres:16-alpine".to_string()));
    }

    #[test]
    fn 重复的行只算一次() {
        let out = "Loaded image: postgres:16-alpine\nLoaded image: postgres:16-alpine\n";
        assert_eq!(parse_loaded(out).len(), 1);
    }

    #[test]
    fn 镜像引用能对上服务名() {
        let (svc, prefix, tag) =
            classify("ghcr.io/agentpit-io/hunter-community-api:1.2.0").unwrap();
        assert_eq!(svc, "api");
        assert_eq!(prefix, "ghcr.io/agentpit-io");
        assert_eq!(tag, "1.2.0");

        let (svc, prefix, tag) =
            classify("hkccr.ccs.tencentyun.com/agentpit/hunter-community-llm-shim:1.1.0").unwrap();
        assert_eq!(svc, "llm-shim");
        assert_eq!(prefix, "hkccr.ccs.tencentyun.com/agentpit");
        assert_eq!(tag, "1.1.0");
    }

    #[test]
    fn 不带前缀的官方库镜像也认() {
        let (svc, prefix, tag) = classify("postgres:16-alpine").unwrap();
        assert_eq!(svc, "postgres");
        assert_eq!(prefix, "docker.io/library");
        assert_eq!(tag, "16-alpine");
    }

    #[test]
    fn 带端口的私有仓库地址不会被冒号切错() {
        // registry:5000 里的冒号不是 tag 分隔符
        let (svc, prefix, tag) =
            classify("registry.local:5000/agentpit/hunter-community-web:1.2.0").unwrap();
        assert_eq!(svc, "web");
        assert_eq!(prefix, "registry.local:5000/agentpit");
        assert_eq!(tag, "1.2.0");
    }

    #[test]
    fn 不相干的镜像不会被硬塞进某个服务() {
        assert!(classify("nginx:latest").is_none());
        assert!(classify("ghcr.io/someone/hunter-community-unknown:1.0").is_none());
        assert!(classify("").is_none());
    }

    #[test]
    fn 六个服务名与_config_images_对得上() {
        let specs = config::images("ghcr.io/agentpit-io", "docker.io/library", "1.2.0");
        let mut a: Vec<&str> = specs.iter().map(|s| s.service.as_str()).collect();
        let mut b: Vec<&str> = SERVICES.to_vec();
        a.sort_unstable();
        b.sort_unstable();
        assert_eq!(a, b);
    }

    #[test]
    fn 每个服务的镜像引用都能被自己的分类器认回去() {
        // 这条盯住「导出的包一定能被导入端认出来」—— 两边用的是同一套命名
        for prefix in ["ghcr.io/agentpit-io", "hkccr.ccs.tencentyun.com/agentpit"] {
            for s in config::images(prefix, prefix, "1.2.0") {
                let (svc, _, _) =
                    classify(&s.reference).unwrap_or_else(|| panic!("认不出 {}", s.reference));
                assert_eq!(svc, s.service);
            }
        }
    }
}
