//! 「一键腾空间」（I13 · R7）。
//!
//! R7 那张表里，规则层允许**自己动手**的只有三样：
//!
//! | 清什么 | 判据 | 为什么敢删 |
//! |---|---|---|
//! | 本项目的**旧**镜像 | 仓库名是 `hunter-community-{web,api,opencode,llm-shim}`，tag 不是现在这一版，**而且没有任何容器在用它** | 重新拉得回来；用着的一律跳过 |
//! | **悬空**数据卷 | 带本项目标签、但**不在 compose 声明的那六个卷里**，而且没有容器挂着 | 那是历史上改过卷名之后留下的孤儿，没人会再读它 |
//! | **过期**备份 | 按 `[backup] keep_days` 轮换（[`crate::backup::prune`]） | 保留策略是用户自己设的 |
//!
//! **需要删用户文件、给虚拟机扩容、调内存的一律只给建议**（方案 R7），
//! 这个模块里没有任何一条能做那些事的代码。
//!
//! ## 为什么不用 `docker image prune` / `volume prune`
//!
//! 因为它们会删到**别人的东西**。测试机上同时跑着 `hunter-community`、
//! 两套 `hca-*` 和本项目，一条 `docker image prune -a` 能把另外三套一起清掉。
//! 所以这里一律**指名道姓地删**，而且每一条都先问一句「有没有容器在用它」——
//! 守卫（[`crate::assist::guard::argv_image_rm`]）也只认指名的那种形状。

use std::collections::BTreeSet;
use std::time::Duration;

use serde::Serialize;

use crate::err::{AppError, AppResult, Code};
use crate::{backup, compose, config};

/// 本项目**自己那四个**服务镜像的名字（不含仓库前缀）。
///
/// `postgres` 与 `redis` 刻意不在里面：它们是公共基础镜像，
/// 这台机器上别的项目很可能也在用同一份。删了它们就是在动别人的东西。
pub const OWN_IMAGE_NAMES: &[&str] = &[
    "hunter-community-web",
    "hunter-community-api",
    "hunter-community-opencode",
    "hunter-community-llm-shim",
];

/// **我们自己会去拉的那几个仓库**（完整的 `<前缀>/<名字>`）。
///
/// ## 为什么不能只看名字的末一段
///
/// 2026-09-23 验收当场抓到的（I13 报告 5.2 节）：测试机上有
/// `hunter-community-api:dev`、`hunter-community-web:p2` 这样的镜像 ——
/// **那是用户自己在本地构建的**，仓库名恰好也叫 `hunter-community-api`。
/// 按末一段匹配，它们会被算成「本项目的旧镜像」，一键清理就会把它们删掉，
/// 而它们根本不是启动器拉下来的。
///
/// 所以判据收紧成：仓库必须是 `<我们的某个镜像源前缀>/<那四个名字之一>`。
/// 前缀取自 [`crate::registry::CANDIDATES`]（ghcr 与腾讯云香港）**再加上**
/// 用户当前配置里那一个（他可能填了自建源）—— 只有这些地方的镜像是我们拉的。
pub fn own_repos() -> BTreeSet<String> {
    let cfg = config::LauncherConfig::load();
    let mut prefixes: Vec<String> = crate::registry::CANDIDATES
        .iter()
        .map(|c| c.prefix.to_string())
        .collect();
    let cur = cfg.hunter.registry_prefix.trim().trim_end_matches('/');
    if !cur.is_empty() {
        prefixes.push(cur.to_string());
    }
    let mut out = BTreeSet::new();
    for p in prefixes {
        for n in OWN_IMAGE_NAMES {
            out.insert(format!("{}/{n}", p.trim_end_matches('/')));
        }
    }
    out
}

/// 一个可以清掉的镜像。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OldImage {
    pub reference: String,
    pub id: String,
    pub bytes: Option<u64>,
}

/// 一个悬空的数据卷。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OrphanVolume {
    pub name: String,
    pub short: String,
    pub bytes: Option<u64>,
}

/// 能腾出多少、腾的是什么（**只看不删**）。
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub old_images: Vec<OldImage>,
    pub old_images_bytes: u64,
    pub orphan_volumes: Vec<OrphanVolume>,
    pub orphan_volumes_bytes: u64,
    pub expired_backups: Vec<String>,
    pub expired_backups_bytes: u64,
    pub total_bytes: u64,
    /// 逐条实测原话（为什么某一个没算进来）
    pub lines: Vec<String>,
    /// 查不了的原因
    pub reasons: Vec<String>,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.old_images.is_empty()
            && self.orphan_volumes.is_empty()
            && self.expired_backups.is_empty()
    }
}

/// `docker image ls` 的一行 → （仓库, tag, id, 仓库末段）。
/// 认不出来就是 `None`（不猜）。
pub fn parse_image_line(l: &str) -> Option<(String, String, String, String)> {
    let f: Vec<&str> = l.trim().split('\t').collect();
    if f.len() < 3 {
        return None;
    }
    let (repo, tag, id) = (f[0].trim(), f[1].trim(), f[2].trim());
    if repo.is_empty() || repo == "<none>" || id.is_empty() {
        return None;
    }
    let last = repo.rsplit('/').next().unwrap_or(repo).to_string();
    Some((repo.to_string(), tag.to_string(), id.to_string(), last))
}

/// 哪些镜像算「本项目的旧镜像」。**纯函数**，方便逐条单测。
///
/// 四个条件缺一不可：
///
/// 1. 仓库**完整地**在 `own` 里（`<我们的镜像源前缀>/<那四个名字之一>`，见 [`own_repos`]）；
/// 2. tag 不是现在这一版；
/// 3. tag 不是 `<none>`；
/// 4. **不在 `in_use` 里**（有容器在用它 —— 不管是谁的容器）。
pub fn old_image_refs(
    lines: &str,
    current_tag: &str,
    own: &BTreeSet<String>,
    in_use: &BTreeSet<String>,
) -> Vec<(String, String)> {
    let mut v = Vec::new();
    for l in lines.lines() {
        let Some((repo, tag, id, _last)) = parse_image_line(l) else {
            continue;
        };
        if !own.contains(&repo) {
            continue;
        }
        if tag == current_tag || tag == "<none>" {
            continue;
        }
        let r = format!("{repo}:{tag}");
        if in_use.contains(&r) || in_use.contains(&id) {
            continue;
        }
        v.push((r, id));
    }
    v.sort();
    v.dedup();
    v
}

/// 哪些卷算「悬空」。**纯函数**。
///
/// compose 声明过的六个卷一个都不碰 —— 那些是活的数据。
pub fn orphan_shorts(all: &[compose::VolumeInfo], in_use: &BTreeSet<String>) -> Vec<String> {
    let declared = compose::declared_volumes();
    all.iter()
        .filter(|v| !declared.contains(&v.short.as_str()))
        .filter(|v| !in_use.contains(&v.name))
        // 标签读不到、短名是猜出来的那一条**不删**：不确定就不动
        .filter(|v| !v.label_missing)
        .map(|v| v.name.clone())
        .collect()
}

/// 现在能清掉多少（只读）。
pub fn plan() -> Plan {
    let cfg = config::LauncherConfig::load();
    let mut p = Plan::default();
    let bin = crate::runtime::which::docker_bin();

    // ① 旧镜像
    match crate::proc::run_timeout(
        &bin,
        &[
            "image",
            "ls",
            "--format",
            "{{.Repository}}\t{{.Tag}}\t{{.ID}}\t{{.Size}}",
        ],
        Duration::from_secs(60),
    ) {
        Ok(r) if r.ok() => {
            let used = images_in_use();
            let own = own_repos();
            let sizes = image_size_map(&r.stdout);
            for (reference, id) in old_image_refs(&r.stdout, &cfg.hunter.tag, &own, &used) {
                p.old_images_bytes += sizes.get(&reference).copied().unwrap_or(0);
                p.old_images.push(OldImage {
                    bytes: sizes.get(&reference).copied(),
                    reference,
                    id,
                });
            }
            p.lines.push(format!(
                "现在装的是 v{}；启动器自己那几个仓库（{}）下别的版本的镜像有 {} 个没有任何容器在用",
                cfg.hunter.tag,
                own.len(),
                p.old_images.len()
            ));
        }
        Ok(r) => p
            .reasons
            .push(format!("docker image ls 没跑通：{}", r.err_line())),
        Err(e) => p.reasons.push(format!("docker image ls 起不来：{}", e.msg)),
    }

    // ② 悬空卷
    match compose::volumes_of_project() {
        Ok(all) => {
            let used = volumes_in_use();
            let sizes = compose::volume_sizes().unwrap_or_default();
            for name in orphan_shorts(&all, &used) {
                let short = all
                    .iter()
                    .find(|v| v.name == name)
                    .map(|v| v.short.clone())
                    .unwrap_or_else(|| name.clone());
                let b = sizes.get(&name).copied();
                p.orphan_volumes_bytes += b.unwrap_or(0);
                p.orphan_volumes.push(OrphanVolume {
                    name,
                    short,
                    bytes: b,
                });
            }
            p.lines.push(format!(
                "本项目名下 {} 个卷里，compose 现在还声明着的有 {} 个",
                all.len(),
                compose::declared_volumes().len()
            ));
        }
        Err(e) => p.reasons.push(format!("列数据卷没问出来：{}", e.msg)),
    }

    // ③ 过期备份
    let all = backup::list();
    let (_, drop, _) = backup::plan_prune(&all, cfg.backup.keep_days_clamped(), 2);
    for id in &drop {
        if let Some(m) = all.iter().find(|x| &x.id == id) {
            p.expired_backups_bytes += backup::dir_size(&m.path());
        }
        p.expired_backups.push(id.clone());
    }
    p.lines.push(format!(
        "备份保留策略是「最近 {} 天」，现在有 {} 份，超出的有 {} 份",
        cfg.backup.keep_days_clamped(),
        all.len(),
        drop.len()
    ));

    p.total_bytes = p.old_images_bytes + p.orphan_volumes_bytes + p.expired_backups_bytes;
    p
}

/// [`plan`] 的缓存（5 分钟）。
///
/// 为什么需要它：运行面板每 60 秒跑一次 [`crate::monitor::alerts`]，而只要
/// 磁盘那一条提醒还立着，每一轮都会来问一次「能清掉多少」。那一问要跑
/// `docker image ls` + `docker volume ls` + `docker system df -v` +
/// **每个容器一次 `docker inspect`**（测试机上同时跑着二十几个容器）。
/// 一分钟一遍太贵，而「有多少旧镜像可以清」这件事也不是秒级变化的。
///
/// 用户在界面上点「看看能腾出多少」走的是 [`plan`] 本身（不走缓存）——
/// 他点的那一下要的就是当下的真值。
static PLAN_CACHE: std::sync::Mutex<Option<(std::time::Instant, Plan)>> =
    std::sync::Mutex::new(None);

const PLAN_TTL: Duration = Duration::from_secs(300);

/// 带缓存的 [`plan`]。只给后台轮询用。
pub fn plan_cached() -> Plan {
    if let Ok(g) = PLAN_CACHE.lock() {
        if let Some((at, p)) = g.as_ref() {
            if at.elapsed() < PLAN_TTL {
                return p.clone();
            }
        }
    }
    let p = plan();
    if let Ok(mut g) = PLAN_CACHE.lock() {
        *g = Some((std::time::Instant::now(), p.clone()));
    }
    p
}

/// 清掉缓存。真的清理完之后要叫一次 —— 不然界面上还会显示「能腾出 1.8 GB」。
pub fn invalidate_cache() {
    if let Ok(mut g) = PLAN_CACHE.lock() {
        *g = None;
    }
}

/// `docker image ls` 的输出里每个 `repo:tag` 多大。
/// 这里的大小是 docker 印的人话（`1.23GB`），按**十进制**还原 —— `image ls` 用的是 SI。
fn image_size_map(lines: &str) -> std::collections::BTreeMap<String, u64> {
    let mut m = std::collections::BTreeMap::new();
    for l in lines.lines() {
        let f: Vec<&str> = l.trim().split('\t').collect();
        if f.len() < 4 {
            continue;
        }
        let Some((repo, tag, _, _)) = parse_image_line(l) else {
            continue;
        };
        if let Some(b) = compose::parse_human_size(f[3]) {
            m.insert(format!("{repo}:{tag}"), b);
        }
    }
    m
}

/// 这台机器上**所有容器**（不只本项目）正在用的镜像。
/// 一条都不能漏 —— 漏一条就可能删掉别人正在用的镜像。
fn images_in_use() -> BTreeSet<String> {
    let bin = crate::runtime::which::docker_bin();
    let mut s = BTreeSet::new();
    let Ok(r) = crate::proc::run_timeout(
        &bin,
        &["ps", "-a", "--format", "{{.Image}}"],
        Duration::from_secs(30),
    ) else {
        // 问不出来就当**全都在用**（返回一个哨兵），调用方因此一个都不删
        s.insert("__查不到，一个都不删__".to_string());
        return s;
    };
    if !r.ok() {
        s.insert("__查不到，一个都不删__".to_string());
        return s;
    }
    for l in r.stdout.lines() {
        let t = l.trim();
        if !t.is_empty() {
            s.insert(t.to_string());
        }
    }
    s
}

/// 这台机器上所有容器挂着的数据卷。
fn volumes_in_use() -> BTreeSet<String> {
    let bin = crate::runtime::which::docker_bin();
    let mut s = BTreeSet::new();
    let Ok(r) = crate::proc::run_timeout(
        &bin,
        &["ps", "-a", "--format", "{{.Names}}"],
        Duration::from_secs(30),
    ) else {
        s.insert("__查不到__".into());
        return s;
    };
    if !r.ok() {
        s.insert("__查不到__".into());
        return s;
    }
    for name in r.stdout.lines().map(str::trim).filter(|x| !x.is_empty()) {
        let Ok(m) = crate::proc::run_timeout(
            &bin,
            &[
                "inspect",
                "-f",
                "{{range .Mounts}}{{if eq .Type \"volume\"}}{{.Name}}\n{{end}}{{end}}",
                name,
            ],
            Duration::from_secs(20),
        ) else {
            continue;
        };
        for v in m.stdout.lines().map(str::trim).filter(|x| !x.is_empty()) {
            s.insert(v.to_string());
        }
    }
    s
}

/// 一次清理的结果。
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Done {
    pub images: Vec<String>,
    pub volumes: Vec<String>,
    pub backups: Vec<String>,
    pub freed_bytes: u64,
    pub steps: Vec<String>,
    pub failures: Vec<String>,
    pub headline: String,
}

/// 真的清。**先重新算一遍**（界面上那份可能已经过时了），再按算出来的清单动手。
pub fn run(mut note: impl FnMut(&str)) -> AppResult<Done> {
    let p = plan();
    let mut d = Done::default();
    let bin = crate::runtime::which::docker_bin();

    if !p.old_images.is_empty() {
        let mut argv = vec![bin.clone(), "image".into(), "rm".into()];
        argv.extend(p.old_images.iter().map(|i| i.reference.clone()));
        crate::assist::guard::argv_image_rm(&argv, true)?;
        let args: Vec<&str> = argv[1..].iter().map(String::as_str).collect();
        match crate::proc::run_timeout(&bin, &args, Duration::from_secs(300)) {
            Ok(r) if r.ok() || r.stderr.contains("No such image") => {
                d.freed_bytes += p.old_images_bytes;
                d.images = p.old_images.iter().map(|i| i.reference.clone()).collect();
                let s = format!(
                    "清掉 {} 个旧镜像（{}）",
                    d.images.len(),
                    crate::flow::human_bytes(p.old_images_bytes)
                );
                note(&s);
                d.steps.push(s);
            }
            Ok(r) => d.failures.push(format!("删旧镜像没成：{}", r.err_line())),
            Err(e) => d.failures.push(format!("删旧镜像没成：{}", e.msg)),
        }
    }

    if !p.orphan_volumes.is_empty() {
        let mut ok: Vec<String> = Vec::new();
        for v in &p.orphan_volumes {
            match crate::assist::guard::own_volume(&v.name) {
                Ok(()) => ok.push(v.name.clone()),
                Err(e) => d
                    .failures
                    .push(format!("卷 {} 没过归属检查，跳过：{}", v.name, e.msg)),
            }
        }
        if !ok.is_empty() {
            let mut argv = vec![bin.clone(), "volume".into(), "rm".into()];
            argv.extend(ok.iter().cloned());
            crate::assist::guard::argv_volume_rm(&argv, true)?;
            let args: Vec<&str> = argv[1..].iter().map(String::as_str).collect();
            match crate::proc::run_timeout(&bin, &args, Duration::from_secs(120)) {
                Ok(r) if r.ok() => {
                    d.freed_bytes += p.orphan_volumes_bytes;
                    let s = format!("清掉 {} 个悬空数据卷", ok.len());
                    note(&s);
                    d.steps.push(s);
                    d.volumes = ok;
                }
                Ok(r) => d.failures.push(format!("删悬空卷没成：{}", r.err_line())),
                Err(e) => d.failures.push(format!("删悬空卷没成：{}", e.msg)),
            }
        }
    }

    if !p.expired_backups.is_empty() {
        let r = backup::prune();
        d.freed_bytes += r.freed_bytes;
        d.backups = r.removed.clone();
        let s = format!(
            "清掉 {} 份过期备份（{}）",
            r.removed.len(),
            crate::flow::human_bytes(r.freed_bytes)
        );
        note(&s);
        d.steps.push(s);
    }

    invalidate_cache();
    d.headline = if d.steps.is_empty() {
        "没有可以安全清掉的东西。".to_string()
    } else {
        format!("一共腾出 {}", crate::flow::human_bytes(d.freed_bytes))
    };
    crate::linfo!("一键清理：{}", d.headline);
    if !d.failures.is_empty() {
        crate::lwarn!("一键清理有没做成的：{:?}", d.failures);
    }
    Ok(d)
}

/// 单独跑「清过期备份」（备份目录所在盘满了那一条规则用它）。
pub fn prune_backups_only() -> AppResult<Done> {
    let r = backup::prune();
    if r.removed.is_empty() && !r.skipped.is_empty() {
        return Err(AppError::new(
            Code::NotImplemented,
            format!(
                "按现在的保留策略没有可以删的备份；有 {} 份认不出日期，那些一律不动。",
                r.skipped.len()
            ),
        ));
    }
    Ok(Done {
        backups: r.removed.clone(),
        freed_bytes: r.freed_bytes,
        headline: format!(
            "清掉 {} 份过期备份，腾出 {}",
            r.removed.len(),
            crate::flow::human_bytes(r.freed_bytes)
        ),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn used(v: &[&str]) -> BTreeSet<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// `docker image ls` 的一段真实输出（测试机 2026-09-23 实抓，只删了无关行）。
    ///
    /// 注意最后那三行：**那是测试机上别人本地构建的镜像**，仓库名恰好也叫
    /// `hunter-community-api` / `hunter-community-web`。它们是这一组测试的重点。
    const LS: &str = "\
ghcr.io/agentpit-io/hunter-community-web\t1.1.0\tsha256aaa\t1.7GB
ghcr.io/agentpit-io/hunter-community-web\t1.2.0\tsha256bbb\t1.7GB
ghcr.io/agentpit-io/hunter-community-api\t1.1.0\tsha256ccc\t898MB
postgres\t16-alpine\tsha256ddd\t250MB
redis\t7-alpine\tsha256eee\t40MB
ghcr.io/other/whatever\t1.0.0\tsha256fff\t10MB
hkccr.ccs.tencentyun.com/agentpit/hunter-community-opencode\t1.1.0\tsha256ggg\t619MB
hunter-community-api\tdev\tsha256hhh\t898MB
hunter-community-web\tp2\tsha256iii\t1.7GB
hunter-community-llm-shim\tdev\tsha256jjj\t74.2MB
";

    /// 测试机上那两个镜像源的前缀（`registry::CANDIDATES` 里那两条）。
    fn own() -> BTreeSet<String> {
        let mut o = BTreeSet::new();
        for p in ["ghcr.io/agentpit-io", "hkccr.ccs.tencentyun.com/agentpit"] {
            for n in OWN_IMAGE_NAMES {
                o.insert(format!("{p}/{n}"));
            }
        }
        o
    }

    #[test]
    fn 旧镜像_只认我们自己那几个仓库下的() {
        let v = old_image_refs(LS, "1.2.0", &own(), &used(&[]));
        let names: Vec<&str> = v.iter().map(|(r, _)| r.as_str()).collect();
        assert!(names.contains(&"ghcr.io/agentpit-io/hunter-community-web:1.1.0"));
        assert!(names.contains(&"ghcr.io/agentpit-io/hunter-community-api:1.1.0"));
        assert!(
            names.contains(&"hkccr.ccs.tencentyun.com/agentpit/hunter-community-opencode:1.1.0"),
            "换过镜像源之后留下的那一份也算本项目的：{names:?}"
        );
        // 现在这一版不能删
        assert!(!names.contains(&"ghcr.io/agentpit-io/hunter-community-web:1.2.0"));
        // postgres / redis 是公共基础镜像，别的项目也在用 —— 一律不碰
        assert!(!names.iter().any(|n| n.starts_with("postgres")));
        assert!(!names.iter().any(|n| n.starts_with("redis")));
        // 别人的镜像更不碰
        assert!(!names.iter().any(|n| n.contains("ghcr.io/other/")));
    }

    /// **本轮验收当场抓到的那一条。**
    ///
    /// 第一版按「仓库名末一段」匹配，于是测试机上用户自己本地构建的
    /// `hunter-community-api:dev` / `hunter-community-web:p2` 被算成了
    /// 「本项目的旧镜像」，一键清理的清单里赫然列着它们（13.3 GB 里有 5 GB 是它们的）。
    /// 那几个镜像**根本不是启动器拉下来的** —— 它们没有我们任何一个镜像源的前缀。
    #[test]
    fn 旧镜像_用户自己本地构建的同名镜像一个都不碰() {
        let v = old_image_refs(LS, "1.2.0", &own(), &used(&[]));
        let names: Vec<&str> = v.iter().map(|(r, _)| r.as_str()).collect();
        for bad in [
            "hunter-community-api:dev",
            "hunter-community-web:p2",
            "hunter-community-llm-shim:dev",
        ] {
            assert!(
                !names.contains(&bad),
                "「{bad}」是用户自己 build 的（没有我们任何一个镜像源的前缀），不该被清：{names:?}"
            );
        }
    }

    #[test]
    fn 旧镜像_有容器在用的一律跳过() {
        let v = old_image_refs(
            LS,
            "1.2.0",
            &own(),
            &used(&["ghcr.io/agentpit-io/hunter-community-api:1.1.0"]),
        );
        let names: Vec<&str> = v.iter().map(|(r, _)| r.as_str()).collect();
        assert!(
            !names.contains(&"ghcr.io/agentpit-io/hunter-community-api:1.1.0"),
            "别的项目可能正拿旧 tag 在跑：{names:?}"
        );
        assert!(names.contains(&"ghcr.io/agentpit-io/hunter-community-web:1.1.0"));
        // 按 ID 命中也要跳过
        let v2 = old_image_refs(LS, "1.2.0", &own(), &used(&["sha256aaa"]));
        assert!(!v2.iter().any(|(r, _)| r.contains("web:1.1.0")));
    }

    #[test]
    fn 旧镜像_仓库集合是空的时候一个都不删() {
        // 配置读不出来、镜像源前缀是空的 —— 宁可什么都不清，也不按名字猜
        let v = old_image_refs(LS, "1.2.0", &BTreeSet::new(), &used(&[]));
        assert!(v.is_empty(), "{v:?}");
    }

    #[test]
    fn 我们自己那几个仓库的全名拼得对() {
        let _h = crate::paths::test_home("cleanup-own-repos");
        let o = own_repos();
        assert!(
            o.contains("ghcr.io/agentpit-io/hunter-community-web"),
            "{o:?}"
        );
        assert!(
            o.contains("hkccr.ccs.tencentyun.com/agentpit/hunter-community-api"),
            "{o:?}"
        );
        // 不带前缀的裸名字**不在**里面 —— 那正是用户本地构建的那一种
        assert!(!o.contains("hunter-community-web"), "{o:?}");
        assert!(!o.iter().any(|x| x.contains("postgres")), "{o:?}");
    }

    #[test]
    fn 悬空卷_声明过的六个一个都不碰() {
        let mk = |short: &str, label_missing: bool| compose::VolumeInfo {
            name: format!("hunter_{short}"),
            short: short.to_string(),
            label_missing,
            mountpoint: "/x".into(),
            size_bytes: None,
        };
        let all = vec![
            mk("hunter_pg_data", false),
            mk("hunter_secrets", false),
            mk("hunter_old_cache", false),
            mk("hunter_guessed", true),
        ];
        let v = orphan_shorts(&all, &used(&[]));
        assert_eq!(v, vec!["hunter_hunter_old_cache".to_string()]);
        // 标签读不到、短名靠猜的那一个不删（不确定就不动）
        assert!(!v.iter().any(|x| x.contains("guessed")));
        // 有容器挂着的也不删
        let v2 = orphan_shorts(&all, &used(&["hunter_hunter_old_cache"]));
        assert!(v2.is_empty());
    }

    #[test]
    fn image_ls_的行认不出来就跳过不猜() {
        assert!(parse_image_line("").is_none());
        assert!(parse_image_line("只有一列").is_none());
        assert!(parse_image_line("<none>\t<none>\tsha1\t1MB").is_none());
        let (repo, tag, id, last) =
            parse_image_line("ghcr.io/a/hunter-community-web\t1.1.0\tsha1\t1MB").unwrap();
        assert_eq!(repo, "ghcr.io/a/hunter-community-web");
        assert_eq!(tag, "1.1.0");
        assert_eq!(id, "sha1");
        assert_eq!(last, "hunter-community-web");
    }

    #[test]
    fn 这个模块里没有任何一条_prune_命令() {
        // R7 的红线：不能用 docker 自己的 prune（它会删到别人的东西）。
        // 这一条靠守卫单测（guard::删镜像_不认_prune_只认指名道姓）兜底，
        // 这里再钉一次意图：本模块拼出来的命令形状只有两种
        let bin = "docker".to_string();
        let argv = vec![bin.clone(), "image".into(), "rm".into(), "x:1".into()];
        crate::assist::guard::argv_image_rm(&argv, true).expect("指名删要放行");
        let bad = vec![bin, "image".into(), "prune".into(), "-a".into()];
        assert!(crate::assist::guard::argv_image_rm(&bad, true).is_err());
    }
}
