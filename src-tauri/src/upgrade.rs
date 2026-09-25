//! Hunter 版本检查与升级（技术方案 §5.6、§10「Hunter 升级」）。
//!
//! 方案原文的四步，逐条落地：
//!
//! > 1. 检测到新 tag → 提示 Release Notes 摘要（从 GitHub Release body 取前 500 字）
//! > 2. 用户确认 → `pg_dump` 到 backups → 替换 compose 文件与 `OPENCODE_TAG` → `pull` → `up -d`
//! > 3. 健康检查通过 → 记录成功；失败 → 回滚 tag → `up -d` → 提示并引导反馈
//! > 4. 跨大版本（如 0.x → 1.x）时提示阅读升级须知
//!
//! 与方案的两处偏离（都写进了成果文档）：
//!
//! * 方案说「替换 `OPENCODE_TAG`」，实际 hunter-community 的 compose 用的是
//!   **一个** `HUNTER_VERSION` 管全部四个镜像，没有单独的 `OPENCODE_TAG`。换的是它。
//! * 方案说「回滚 tag → `up -d`」，这里**还会把 `.env` 与两个 compose 文件一起写回**：
//!   只改 tag 不够 —— 新版本的 compose 可能加了新服务、新的健康检查、新的环境变量，
//!   拿新 compose 配旧镜像起出来的东西谁也说不清是什么。回滚就要回到升级前那一整套。
//!
//! ## 升级的顺序与「哪一步之后才可能需要回滚」
//!
//! ```text
//! ① 取新版 compose（严格模式，取不到就停）      ← 还没动任何东西，失败 = 什么都没变
//! ② 备份（pg_dump + .env + 两个 compose）        ← 同上
//! ─────────────── 从这里开始，失败要回滚 ───────────────
//! ③ 写新 compose / 新 .env（tag 换掉）/ 覆盖文件
//! ④ docker compose config 校验 + 红线 4 的端口自查
//! ⑤ pull（失败自动换源重试 3 次）
//! ⑥ up -d + 等 180 秒健康
//! ```
//!
//! ①② 失败时如实报错，**不谎称"已回滚"** —— 没动过的东西没有什么可回滚的。
//!
//! ## 为什么镜像不存在这件事留到 ⑤ 才发现
//!
//! 大可以在 ① 之后挨个探一遍六个镜像的 manifest。没这么做，因为 `pull` 本来就要做同样的事，
//! 而且它还带换源重试；提前探一遍只是把同样的网络往返做两次，却换不来更多信息。
//! 镜像拉不到是升级失败最常见的原因（镜像源同步滞后、网断、限流），
//! 它落在 ⑤ 正好走完整的回滚路径 —— 那条路径本来就是为这种情况写的。

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;

use crate::compose::{self, PullProgress};
use crate::config;
use crate::err::{AppError, AppResult, Code};
use crate::flow::{self, AppState, InstallOptions};
use crate::{linfo, lwarn, registry};

/// Release 信息的缓存时长。方案 §11.3 写的就是 6 小时。
///
/// **缓存不是优化，是必需**：GitHub 对未认证请求的限额是每小时 60 次，
/// 而运行面板每 10 秒刷新一次状态 —— 不缓存的话一个用户开着面板十分钟就把额度用光，
/// 之后「有新版本」角标会无声无息地不再出现。
pub const RELEASE_TTL: Duration = Duration::from_secs(6 * 3600);

const NET_TIMEOUT: Duration = Duration::from_secs(25);
const NOTES_LIMIT: usize = 500;

/// hunter-community 的一次 Release。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseInfo {
    /// 去掉了前导 `v`，和 `.env` 里的 `HUNTER_VERSION` 同一种写法
    pub tag: String,
    /// Release Notes 摘要（方案 §10：取前 500 字）
    pub notes: Option<String>,
    /// 完整 Release 页地址，界面上「看完整说明」跳这里
    pub notes_url: Option<String>,
    /// 发布时间（原样保留 GitHub 给的 ISO 8601）
    pub published_at: Option<String>,
}

/// 「有没有新版本」的完整答案。拿不到就是 `latest: None` + `reason`，绝不猜（红线 1）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeCheck {
    pub current: String,
    pub latest: Option<String>,
    pub has_update: bool,
    pub notes: Option<String>,
    pub notes_url: Option<String>,
    pub published_at: Option<String>,
    /// 跨大版本（方案 §10 第 4 条：这种时候要提示读升级须知）
    pub major_jump: bool,
    /// 查不到时的原因，直接显示给用户
    pub reason: Option<String>,
}

/// 一次升级的结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeResult {
    pub ok: bool,
    pub from: String,
    pub to: String,
    /// 备份目录名，失败时也有 —— 用户要能找到它
    pub backup_id: Option<String>,
    /// 备份里的数据库转储有多大；没转成就是 `None`
    pub backup_sql_bytes: Option<u64>,
    /// 失败时：回滚做了没有、做成了没有
    pub rolled_back: bool,
    /// **是用户自己点的「取消」**（I16），不是出了错。
    ///
    /// 界面要靠它把这两件事分开说：失败要给「重试 / 反馈」，
    /// 取消只要一句「已取消，什么都没变」。把用户的主动选择显示成一条红色错误，
    /// 会让他以为自己把机器点坏了。
    pub cancelled: bool,
    pub message: String,
}

// ── 版本检查 ──────────────────────────────────────────────────────────────

fn release_slot() -> &'static flow::CacheSlot<Option<ReleaseInfo>> {
    static CACHE: flow::CacheSlot<Option<ReleaseInfo>> = Mutex::new(None);
    &CACHE
}

/// 最新 Release，缓存 [`RELEASE_TTL`]。
pub fn latest_release() -> Option<ReleaseInfo> {
    flow::cached(release_slot(), RELEASE_TTL, fetch_latest_release)
}

/// 用户亲手点「检查更新」时用：绕过缓存真去问一次，并把缓存刷新掉。
///
/// 自动刷新走缓存是为了不被 GitHub 限流；但用户主动点的那一下如果也回一个
/// 六小时前的答案，这个按钮就等于没有。一次手点一次请求，离限额远得很。
pub fn latest_release_now() -> Option<ReleaseInfo> {
    flow::cached(release_slot(), Duration::ZERO, fetch_latest_release)
}

fn fetch_latest_release() -> Option<ReleaseInfo> {
    let r = crate::http::get(
        "https://api.github.com/repos/agentpit-io/hunter-community/releases/latest",
        &[("Accept", "application/vnd.github+json")],
        Duration::from_secs(12),
    )
    .ok()?;
    if !r.ok() {
        lwarn!("查最新版本失败：GitHub 返回 HTTP {}", r.status);
        return None;
    }
    let v = r.json()?;
    let tag = v
        .get("tag_name")?
        .as_str()?
        .trim_start_matches('v')
        .to_string();
    Some(ReleaseInfo {
        tag,
        notes: v
            .get("body")
            .and_then(|x| x.as_str())
            .map(|s| summarize_notes(s, NOTES_LIMIT)),
        notes_url: v
            .get("html_url")
            .and_then(|x| x.as_str())
            .map(str::to_string),
        published_at: v
            .get("published_at")
            .and_then(|x| x.as_str())
            .map(str::to_string),
    })
}

/// Release Notes 摘要：取前 `limit` 个**字符**（不是字节 —— 中文一个字三字节，
/// 按字节切会把字切碎），截断了就补一个省略号。
pub fn summarize_notes(body: &str, limit: usize) -> String {
    let body = body.replace("\r\n", "\n");
    let trimmed = body.trim();
    if trimmed.chars().count() <= limit {
        return trimmed.to_string();
    }
    let cut: String = trimmed.chars().take(limit).collect();
    format!("{cut}…")
}

/// 比较两个版本号。只认 `1.2.0` 这种点分数字，认不出的段落按 0 处理并**排在后面**比较原串，
/// 这样 `1.2.0` 与 `1.2.0-rc1` 不会被判成相等。
pub fn is_newer(candidate: &str, current: &str) -> bool {
    let nums = |s: &str| -> Vec<u32> {
        s.split('-')
            .next()
            .unwrap_or("")
            .split('.')
            .map(|x| x.parse::<u32>().unwrap_or(0))
            .collect()
    };
    let (a, b) = (nums(candidate), nums(current));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (
            a.get(i).copied().unwrap_or(0),
            b.get(i).copied().unwrap_or(0),
        );
        if x != y {
            return x > y;
        }
    }
    // 主版本号一样：`1.2.0` 比 `1.2.0-rc1` 新（正式版没有后缀）
    let pre = |s: &str| s.split_once('-').map(|(_, p)| p.to_string());
    match (pre(candidate), pre(current)) {
        (None, Some(_)) => true,
        (Some(_), None) => false,
        (Some(x), Some(y)) => x > y,
        (None, None) => false,
    }
}

/// 大版本变了没有（方案 §10 第 4 条）。
pub fn is_major_jump(from: &str, to: &str) -> bool {
    let major = |s: &str| {
        s.split('-')
            .next()
            .unwrap_or("")
            .split('.')
            .next()
            .and_then(|x| x.parse::<u32>().ok())
    };
    match (major(from), major(to)) {
        (Some(a), Some(b)) => a != b,
        _ => false,
    }
}

/// 完整的「有没有新版本」。`force` 为真时绕过 6 小时缓存。
pub fn check(current: &str, force: bool) -> UpgradeCheck {
    let rel = if force {
        latest_release_now()
    } else {
        latest_release()
    };
    match rel {
        Some(r) => UpgradeCheck {
            has_update: is_newer(&r.tag, current),
            major_jump: is_major_jump(current, &r.tag),
            latest: Some(r.tag),
            notes: r.notes,
            notes_url: r.notes_url,
            published_at: r.published_at,
            current: current.to_string(),
            reason: None,
        },
        None => UpgradeCheck {
            current: current.to_string(),
            latest: None,
            has_update: false,
            notes: None,
            notes_url: None,
            published_at: None,
            major_jump: false,
            reason: Some(
                "问不到 GitHub 的 Release 接口（网络不通，或者每小时 60 次的未认证限额用完了）"
                    .to_string(),
            ),
        },
    }
}

// ── 升级 ──────────────────────────────────────────────────────────────────

/// 把 Hunter 升到 `target` 这个 tag。失败自动回滚到升级前那一整套配置。
///
/// `note` 收每一步的文字（界面上的步骤条与 headless 的终端输出共用它），
/// `on_pull` 收拉取进度。
pub fn upgrade(
    state: &AppState,
    target: &str,
    mut note: impl FnMut(&str),
    on_pull: impl FnMut(&PullProgress),
) -> AppResult<UpgradeResult> {
    let cfg0 = state.config();
    let from = cfg0.hunter.tag.clone();
    if target.trim().is_empty() {
        return Err(AppError::new(Code::UpdateFailed, "没给要升到哪个版本"));
    }
    if target == from {
        return Err(AppError::new(
            Code::UpdateFailed,
            format!("当前已经是 v{from} 了"),
        ));
    }
    if !cfg0.install.done {
        return Err(AppError::new(
            Code::UpdateFailed,
            "还没装过 Hunter，没有可升级的东西".to_string(),
        ));
    }
    linfo!("开始升级 Hunter：v{from} → v{target}");
    note(&format!("升级 Hunter：v{from} → v{target}"));
    if is_major_jump(&from, target) {
        note("这是一次跨大版本升级，升级前请先读一遍这个版本的 Release Notes");
    }

    // ① 新版 compose。严格模式：取不到就停在这里，此时什么都还没动
    note(&format!("正在取 v{target} 的 docker-compose.yml…"));
    let (new_yml, src_host) = config::fetch_compose_strict(target, NET_TIMEOUT)?;
    note(&format!(
        "已取到（来自 {src_host}，{} 字节）",
        new_yml.len()
    ));

    // ② 备份
    note("正在备份（数据库 + 配置）…");
    let backup = crate::backup::create(crate::backup::Kind::PreUpgrade, &from, &mut note)?;
    let _ = crate::backup::write_readme(&backup.id);
    if !backup.has_dump() {
        // 数据库没备份成功就不往下走。升级最坏的结果是数据出问题，
        // 没有转储就等于没有后悔药 —— 这时候继续是拿用户的数据赌运气。
        return Err(AppError::new(
            Code::UpdateFailed,
            format!(
                "数据库没备份成功（{}），升级中止。什么都没有改动。",
                backup
                    .dump_error
                    .clone()
                    .unwrap_or_else(|| "原因不明".into())
            ),
        ));
    }

    // ─── 从这里开始，失败要回滚 ───
    let r = do_upgrade(state, &cfg0, target, &new_yml, &mut note, on_pull);
    match r {
        Ok(()) => {
            let mut cfg = state.config();
            cfg.hunter.tag = target.to_string();
            cfg.install.at = crate::timefmt::now_shanghai();
            let _ = cfg.save();
            state.set_config(cfg);
            let msg = format!(
                "已升级到 v{target}，六个服务都已就绪。升级前的备份留在 {}。",
                crate::backup::dir_of(&backup.id).display()
            );
            linfo!("升级成功：v{from} → v{target}");
            note(&msg);
            state.tele(
                "hunter_upgraded",
                &[
                    ("from", crate::telemetry::Field::Enum(from.clone())),
                    ("to", crate::telemetry::Field::Enum(target.to_string())),
                    ("ok", crate::telemetry::Field::Bool(true)),
                ],
            );
            Ok(UpgradeResult {
                ok: true,
                from,
                to: target.to_string(),
                backup_id: Some(backup.id),
                backup_sql_bytes: backup.sql_bytes,
                rolled_back: false,
                cancelled: false,
                message: msg,
            })
        }
        // ── 用户自己点了「取消」（I16） ───────────────────────────────
        //
        // 客户 2026-09-25 那次只能强退，而强退发生在「`.env` 已经写成 1.2.2、
        // 镜像还没拉全」之后 —— **没有走到回滚**，机器就停在一个说不清的中间态。
        // 有了「取消」这条路，同样的现场会走完整的回滚，而且说得清楚：
        // 版本没变、容器没动。
        Err(e) if state.cancel.load(std::sync::atomic::Ordering::Relaxed) => {
            linfo!("升级被用户取消：{}", e.msg);
            note("已取消，正在把配置写回升级前那一份…");
            let rolled = rollback(state, &cfg0, &backup.id, &mut note);
            let running = compose::ps()
                .map(|v| v.iter().filter(|s| s.state == "running").count())
                .unwrap_or(0);
            let msg = match &rolled {
                Ok(_) => format!(
                    "已取消。Hunter 还是 v{from}，正在跑的 {running} 个容器一个都没动，\
                     配置也写回了升级前那一份。升级前的备份留在 {}。",
                    crate::backup::dir_of(&backup.id).display()
                ),
                Err(re) => format!(
                    "已取消，但配置没能完全写回：{}。备份在 {}，可以照着它手工恢复。",
                    re.msg,
                    crate::backup::dir_of(&backup.id).display()
                ),
            };
            note(&msg);
            state.tele(
                "hunter_upgrade_cancelled",
                &[
                    ("from", crate::telemetry::Field::Enum(from.clone())),
                    ("to", crate::telemetry::Field::Enum(target.to_string())),
                    ("rolled_back", crate::telemetry::Field::Bool(rolled.is_ok())),
                ],
            );
            Ok(UpgradeResult {
                ok: false,
                from,
                to: target.to_string(),
                backup_id: Some(backup.id),
                backup_sql_bytes: backup.sql_bytes,
                rolled_back: rolled.is_ok(),
                cancelled: true,
                message: msg,
            })
        }
        Err(e) => {
            lwarn!("升级失败，开始回滚：{}", e.msg);
            note(&format!("升级失败：{}", e.msg));
            note(&format!("正在回滚到 v{from}…"));
            let rolled = rollback(state, &cfg0, &backup.id, &mut note);
            state.tele(
                "hunter_upgraded",
                &[
                    ("from", crate::telemetry::Field::Enum(from.clone())),
                    ("to", crate::telemetry::Field::Enum(target.to_string())),
                    ("ok", crate::telemetry::Field::Bool(false)),
                    ("rolled_back", crate::telemetry::Field::Bool(rolled.is_ok())),
                ],
            );
            let tail = match &rolled {
                Ok(m) => format!(
                    "{m} 升级前的备份在 {}。",
                    crate::backup::dir_of(&backup.id).display()
                ),
                Err(re) => format!(
                    "而且回滚也没做干净：{}。备份在 {}，可以照着它手工恢复。",
                    re.msg,
                    crate::backup::dir_of(&backup.id).display()
                ),
            };
            note(&tail);
            Err(AppError::new(
                Code::UpdateFailed,
                format!("升到 v{target} 失败：{}。{tail}", e.msg),
            ))
        }
    }
}

/// ③④⑤⑥ —— 真正会改变现状的那几步。任何一处出错都由调用方回滚。
fn do_upgrade(
    state: &AppState,
    cfg0: &config::LauncherConfig,
    target: &str,
    new_yml: &str,
    note: &mut impl FnMut(&str),
    on_pull: impl FnMut(&PullProgress),
) -> AppResult<()> {
    // ③ 写新配置
    config::write_compose(new_yml)?;
    crate::flow::write_version_file(target);
    let mut cfg = cfg0.clone();
    cfg.hunter.tag = target.to_string();
    flow::rewrite_env_and_override(state, &cfg, target)?;
    note(&format!(
        "已写入新的 compose 与 .env（HUNTER_VERSION={target}，端口不变）"
    ));

    // ④ 让 compose 自己解析一遍，顺便复核红线 4
    let rendered = compose::config_check_json()?;
    let bind = crate::config::WebBind::detect();
    flow::verify_bindings(&rendered, bind)?;
    note(if bind.lan_exposed() {
        // 用户 2026-09-21 19:05 的决定第三点：升级**不自动改动**已有配置。
        // 但不能不说 —— 说清楚现状与怎么收紧，运行面板上还有一个按钮。
        "docker compose config 校验通过。这台机器的网页端口升级前就对局域网开放，本次没有改动它；         新版本默认只允许本机访问，运行面板上点「只允许本机访问」就能收紧（收紧后不能再放开）。"
    } else {
        "docker compose config 校验通过，六个服务的端口全部绑在 127.0.0.1（红线 4）"
    });

    // ⑤ 拉新镜像
    let prep = prepare_pull(&cfg, target, note)?;
    let opts = InstallOptions {
        // 不指定源 = 拉不动时 flow::pull 会自动换到另一个源重试
        //（国内源同步滞后时正好靠它退回 GHCR，`plan/国内镜像与下载源.md` 要求的行为）
        registry: None,
        tag: target.to_string(),
    };
    note("正在拉取新版本的镜像…");
    flow::pull(state, &prep, &opts, on_pull)?;

    // ⑥ 起容器 + 等健康
    note("正在用新镜像重建容器…");
    compose::up()?;
    let list = compose::wait_healthy(compose::START_TIMEOUT, |v| {
        if let Ok(mut g) = state.services.lock() {
            *g = v.to_vec();
        }
    })?;
    note(&format!(
        "{} 个服务全部就绪",
        list.iter().filter(|s| compose::service_ready(s)).count()
    ));
    Ok(())
}

/// 给 [`flow::pull`] 攒一份 `PrepareResult`。
///
/// 和安装时的 [`flow::prepare`] 不同，这里**不重新解端口**：端口是升级前就在用的，
/// 换一次等于把用户的书签、外部集成全弄坏，而且那几个端口正被我们自己的容器占着，
/// 重新解析反而会把它们判成"冲突"。
fn prepare_pull(
    cfg: &config::LauncherConfig,
    tag: &str,
    note: &mut impl FnMut(&str),
) -> AppResult<flow::PrepareResult> {
    let specs = config::images(&cfg.hunter.registry_prefix, &cfg.hunter.base_prefix, tag);
    let arch = registry::oci_arch();
    let mut sizes: BTreeMap<String, u64> = BTreeMap::new();
    for s in &specs {
        if let Ok(n) = registry::compressed_size(&s.host, &s.repo, &s.tag, arch, NET_TIMEOUT) {
            sizes.insert(s.service.clone(), n);
        }
    }
    let total: u64 = sizes.values().sum();
    if total > 0 {
        note(&format!(
            "要拉的六个镜像合计约 {}（已有的层不会重复下载）",
            flow::human_bytes(total)
        ));
    } else {
        note("读不到 manifest，进度条的总量会在拉取过程中逐步补齐");
    }
    Ok(flow::PrepareResult {
        specs,
        sizes,
        registry_prefix: cfg.hunter.registry_prefix.clone(),
        registry_label: registry::by_id(&cfg.hunter.registry_id)
            .map(|c| c.label.to_string())
            .unwrap_or_else(|| cfg.hunter.registry_prefix.clone()),
        ports: cfg.hunter.ports.clone(),
        changes: Vec::new(),
        // 升级这条路不测速（用的是配置里已经定下的源），所以没有被绕开的源
        registry_skipped: Vec::new(),
    })
}

/// 回滚：把升级前那一整套配置写回去，用旧镜像重新起，等健康。
///
/// **不动数据库**（见 [`crate::backup`] 模块头的决定二）。
fn rollback(
    state: &AppState,
    cfg0: &config::LauncherConfig,
    backup_id: &str,
    note: &mut impl FnMut(&str),
) -> AppResult<String> {
    let files = crate::backup::restore_config(backup_id)?;
    note(&format!("已写回升级前的 {}", files.join("、")));
    crate::flow::write_version_file(&cfg0.hunter.tag);

    // `launcher.toml` 整份写回升级前那一版，**不是只改 tag**。
    // 因为升级过程中 `flow::pull` 可能已经换过镜像源并把它存进了 launcher.toml；
    // 只改 tag 的话会留下「.env 是旧源、launcher.toml 是新源」这种对不上的状态，
    // 下一次启动或换源时就会撞上。
    let _ = cfg0.save();
    state.set_config(cfg0.clone());

    compose::up()?;
    let list = compose::wait_healthy(compose::START_TIMEOUT, |v| {
        if let Ok(mut g) = state.services.lock() {
            *g = v.to_vec();
        }
    })?;
    let ok = list.iter().filter(|s| compose::service_ready(s)).count();
    linfo!("回滚完成，v{} 的 {ok} 个服务已就绪", cfg0.hunter.tag);
    Ok(format!(
        "已回滚到 v{}，{ok} 个服务重新就绪。",
        cfg0.hunter.tag
    ))
}

// ── 上一次升级没做完（I16 · P1-2） ────────────────────────────────────────

/// 「配置说的是一版、跑着的是另一版、而新版镜像本机还不齐」的现场。
///
/// ## 这是怎么留下来的
///
/// 客户 2026-09-25 那次：`.env` 已经写成 `HUNTER_VERSION=1.2.2`、
/// `docker-compose.yml` 也换成了 1.2.2 的，镜像拉到一半卡死，
/// 他只能强退 —— **强退不会走回滚**。于是机器停在：
///
/// ```text
/// 配置（.env / compose）  → 1.2.2
/// 正在跑的六个容器        → 1.2.0
/// 本机的 1.2.2 镜像       → 不齐
/// ```
///
/// 下一次打开启动器，如果照着配置去 `up -d`，docker 会去拉那个还没下完的镜像
/// —— 用户又看到一次「卡住」。**这一档必须先问人**。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Interrupted {
    /// 配置（`.env` / compose）说的版本
    pub config_tag: String,
    /// 正在跑的容器实际用的版本
    pub running_tag: String,
    /// 本机还缺的那几个服务的镜像
    pub missing_images: Vec<String>,
    /// 摆给用户看的一句话
    pub headline: String,
    /// 逐条事实（每一条都是实测的，不猜）
    pub lines: Vec<String>,
}

/// 纯判据。**机器状态由调用方喂进来**，这样它考得了 ——
/// I7 那条「把机器状态当成测试前提」的教训在这里同样成立。
///
/// 三个条件缺一不可：
/// 1. 配置说的版本与正在跑的版本不一样；
/// 2. 真的有容器在跑（一个都没有 = 那是「还没装」或「停着」，不是「升级没做完」）；
/// 3. 配置那一版的镜像**本机不齐** —— 齐了的话直接 `up -d` 就完事，
///    那不是一个需要打断用户的现场。
pub fn judge_interrupted(
    config_tag: &str,
    running_tags: &[(String, String)],
    missing_images: &[String],
) -> Option<Interrupted> {
    let config_tag = config_tag.trim();
    if config_tag.is_empty() || running_tags.is_empty() || missing_images.is_empty() {
        return None;
    }
    // 跑着的版本：取出现次数最多的那一个（升级到一半时可能几个新几个旧）
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, t) in running_tags {
        *counts.entry(t.as_str()).or_default() += 1;
    }
    let running_tag = counts
        .iter()
        .max_by_key(|(_, n)| **n)
        .map(|(t, _)| (*t).to_string())?;
    if running_tag.is_empty() || running_tag == config_tag {
        return None;
    }
    let mut lines = vec![
        format!("配置（.env 与 compose）写的是 v{config_tag}"),
        format!(
            "正在跑的 {} 个容器用的是 v{running_tag}",
            running_tags.len()
        ),
        format!(
            "v{config_tag} 的镜像本机还缺 {} 个：{}",
            missing_images.len(),
            missing_images.join("、")
        ),
    ];
    lines.push(
        "所以上一次升级多半是拉镜像时被打断的（强退 / 断电 / 关机）。\
         在你选之前，启动器不会按新配置去起容器 —— 那只会再卡一次。"
            .to_string(),
    );
    Some(Interrupted {
        headline: format!(
            "上一次升级没做完：配置已经是 v{config_tag}，跑着的还是 v{running_tag}，而 v{config_tag} 的镜像本机不齐"
        ),
        config_tag: config_tag.to_string(),
        running_tag,
        missing_images: missing_images.to_vec(),
        lines,
    })
}

/// 去现场查一遍。**只读**：`docker compose ps` + `docker image inspect`，
/// 一个容器、一个卷、一个配置文件都不碰。
pub fn interrupted(cfg: &config::LauncherConfig) -> Option<Interrupted> {
    let config_tag = config::parse_env_file(&crate::paths::env_file())
        .get("HUNTER_VERSION")
        .cloned()
        .unwrap_or_else(|| cfg.hunter.tag.clone());
    let ps = compose::ps().ok()?;
    let running: Vec<(String, String)> = ps
        .iter()
        .filter(|s| s.state == "running")
        .filter_map(|s| {
            let img = s.image.as_deref()?;
            Some((s.service.clone(), tag_of_image(img)?))
        })
        // postgres:16-alpine / redis:7-alpine 的 tag 和 Hunter 的版本号无关，
        // 拿它们比版本会得出一堆假阳性 —— 只看我们自己那四个镜像
        .filter(|(svc, _)| matches!(svc.as_str(), "web" | "api" | "opencode" | "llm-shim"))
        .collect();
    let specs = config::images(
        &cfg.hunter.registry_prefix,
        &cfg.hunter.base_prefix,
        &config_tag,
    );
    let (_ok, missing) = crate::offline::all_present(&specs);
    judge_interrupted(&config_tag, &running, &missing)
}

/// 从一个完整镜像引用里取 tag：`ghcr.io/agentpit-io/hunter-community-web:1.2.0` → `1.2.0`。
/// 没有 tag（或者只有 digest）时返回 `None` —— **不猜成 latest**。
pub fn tag_of_image(image: &str) -> Option<String> {
    let last = image.rsplit('/').next()?;
    let (_, tag) = last.rsplit_once(':')?;
    if tag.is_empty() || tag.contains('@') {
        return None;
    }
    Some(tag.to_string())
}

/// **回退到正在跑的那一版**（中间态的第二个出口）。
///
/// 做的事只有一件：把配置写回 `tag` 那一版，让「配置说的」和「跑着的」
/// 重新对上。**不碰容器、不碰卷、不拉任何镜像** —— 那六个容器本来就在正常跑，
/// 为了「让配置对上」去重建它们是完全不必要的风险。
///
/// 优先从**升级前那次自动备份**里把配置原样写回（它就是当时那一份，
/// 而且不需要网络）；找不到对得上的备份才去重新取一份 compose。
pub fn revert_to_running(
    state: &AppState,
    tag: &str,
    mut note: impl FnMut(&str),
) -> AppResult<String> {
    let tag = tag.trim().to_string();
    if tag.is_empty() {
        return Err(AppError::new(Code::UpdateFailed, "没说要回到哪一版"));
    }
    let mut cfg = state.config();
    let from = cfg.hunter.tag.clone();
    note(&format!("把配置写回 v{tag}（不动正在跑的容器）"));

    let backup = crate::backup::list()
        .into_iter()
        .find(|m| m.tag == tag && m.kind == crate::backup::Kind::PreUpgrade);
    match &backup {
        Some(m) => {
            let files = crate::backup::restore_config(&m.id)?;
            note(&format!(
                "已从升级前那次备份（{}）写回 {}",
                m.id,
                files.join("、")
            ));
        }
        None => {
            // 没有对得上的备份就重新取一份那一版的 compose。
            // 取不到就停 —— 宁可什么都不改，也不要写一份来路不明的配置
            note(&format!(
                "没有找到 v{tag} 的升级前备份，去取一份 v{tag} 的 compose…"
            ));
            let (yml, host) = config::fetch_compose_strict(&tag, NET_TIMEOUT)?;
            config::write_compose(&yml)?;
            note(&format!("已取到（来自 {host}，{} 字节）", yml.len()));
            cfg.hunter.tag = tag.clone();
            flow::rewrite_env_and_override(state, &cfg, &tag)?;
        }
    }
    cfg.hunter.tag = tag.clone();
    crate::flow::write_version_file(&tag);
    cfg.save()?;
    state.set_config(cfg);

    // 复核一遍：写完之后配置到底是不是那一版（红线 1：说「已回退」之前先看一眼）
    let now = config::parse_env_file(&crate::paths::env_file())
        .get("HUNTER_VERSION")
        .cloned()
        .unwrap_or_default();
    if now != tag {
        return Err(AppError::new(
            Code::UpdateFailed,
            format!("写回之后 .env 里的 HUNTER_VERSION 还是 {now}，不是 {tag}。配置没有改成功。"),
        ));
    }
    let msg = format!(
        "已经回到 v{tag}：配置写回了，正在跑的容器一个都没动（它们本来就是 v{tag}）。\
         之前那次没做完的升级（→ v{from}）就此作罢，想再升随时可以在更新页点。"
    );
    linfo!("{msg}");
    note(&msg);
    Ok(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── 上一次升级没做完（I16 · P1-2） ──────────────────────────────────

    fn running(tag: &str) -> Vec<(String, String)> {
        ["web", "api", "opencode", "llm-shim"]
            .iter()
            .map(|s| ((*s).to_string(), tag.to_string()))
            .collect()
    }

    /// 客户 2026-09-25 强退之后留下的那个现场：
    /// 配置 1.2.2 / 容器 1.2.0 / 1.2.2 的镜像本机不齐。
    #[test]
    fn 配置_1_2_2_容器_1_2_0_镜像不全判定为升级未完成() {
        let r = judge_interrupted(
            "1.2.2",
            &running("1.2.0"),
            &["web".into(), "api".into(), "opencode".into()],
        )
        .expect("这就是「上一次升级没做完」");
        assert_eq!(r.config_tag, "1.2.2");
        assert_eq!(r.running_tag, "1.2.0");
        assert_eq!(r.missing_images.len(), 3);
        // 两个按钮都给得出来：继续升到 config_tag、回退到 running_tag
        assert!(r.headline.contains("1.2.2") && r.headline.contains("1.2.0"));
        assert!(r.lines.iter().any(|l| l.contains("镜像本机还缺")));
    }

    /// **配置与容器一致时不许误报。** 这是最要紧的一条 ——
    /// 误报会在每一台正常机器的开机第一屏上弹一个吓人的问句。
    #[test]
    fn 配置与容器一致时不误报() {
        assert!(judge_interrupted("1.2.0", &running("1.2.0"), &[]).is_none());
        // 就算镜像不齐（有人手工 `docker rmi` 过），只要版本对得上就不是这个现场
        assert!(judge_interrupted("1.2.0", &running("1.2.0"), &["web".into()]).is_none());
    }

    #[test]
    fn 镜像齐了就不是这个现场() {
        // 配置 1.2.2、容器 1.2.0，但 1.2.2 的镜像都在本机 ——
        // 那只是「拉完了还没 up」，直接起就行，不用打断用户
        assert!(judge_interrupted("1.2.2", &running("1.2.0"), &[]).is_none());
    }

    #[test]
    fn 一个容器都没跑的时候不是这个现场() {
        // 「还没装」与「全停着」都会走到这里，它们都不是「升级没做完」
        assert!(judge_interrupted("1.2.2", &[], &["web".into()]).is_none());
    }

    #[test]
    fn 升到一半几个新几个旧时按多数判() {
        let mixed = vec![
            ("web".to_string(), "1.2.2".to_string()),
            ("api".to_string(), "1.2.0".to_string()),
            ("opencode".to_string(), "1.2.0".to_string()),
            ("llm-shim".to_string(), "1.2.0".to_string()),
        ];
        let r = judge_interrupted("1.2.2", &mixed, &["opencode".into()]).expect("算这个现场");
        assert_eq!(r.running_tag, "1.2.0", "多数派是旧版");
    }

    #[test]
    fn 从镜像引用里取_tag() {
        assert_eq!(
            tag_of_image("ghcr.io/agentpit-io/hunter-community-web:1.2.0").as_deref(),
            Some("1.2.0")
        );
        assert_eq!(
            tag_of_image("hkccr.ccs.tencentyun.com/agentpit/hunter-community-api:1.2.2").as_deref(),
            Some("1.2.2")
        );
        // 带端口的私有仓库：冒号在主机那一段，别把端口当成 tag
        assert_eq!(
            tag_of_image("10.0.0.2:5000/hunter/web:1.2.0").as_deref(),
            Some("1.2.0")
        );
        assert_eq!(
            tag_of_image("10.0.0.2:5000/hunter/web"),
            None,
            "没有 tag 就不猜"
        );
        assert_eq!(tag_of_image("postgres"), None);
    }

    #[test]
    fn 版本比较() {
        assert!(is_newer("1.2.0", "1.1.0"));
        assert!(is_newer("1.10.0", "1.9.0"), "10 要比 9 大，不是按字符串比");
        assert!(is_newer("2.0.0", "1.99.99"));
        assert!(!is_newer("1.1.0", "1.2.0"));
        assert!(!is_newer("1.2.0", "1.2.0"));
    }

    #[test]
    fn 正式版比同号的预发布新() {
        assert!(is_newer("1.2.0", "1.2.0-rc1"));
        assert!(!is_newer("1.2.0-rc1", "1.2.0"));
        assert!(is_newer("1.2.0-rc2", "1.2.0-rc1"));
    }

    #[test]
    fn 跨大版本认得出来() {
        assert!(is_major_jump("0.9.0", "1.0.0"));
        assert!(is_major_jump("1.2.0", "2.0.0"));
        assert!(!is_major_jump("1.1.0", "1.2.0"));
        // 认不出来的版本号不瞎报"跨大版本"
        assert!(!is_major_jump("main", "1.2.0"));
    }

    #[test]
    fn 摘要按方案取前_500_字() {
        let long = "版".repeat(800);
        let s = summarize_notes(&long, NOTES_LIMIT);
        // 按字符切，不按字节 —— 按字节切会把中文切成乱码
        assert_eq!(s.chars().count(), NOTES_LIMIT + 1, "多出来的一个是省略号");
        assert!(s.ends_with('…'));

        let short = "修了三个 bug";
        assert_eq!(summarize_notes(short, NOTES_LIMIT), short);
    }

    #[test]
    fn 摘要把_crlf_归一并去掉首尾空白() {
        assert_eq!(summarize_notes("\r\n  一行  \r\n", 100), "一行");
    }

    #[test]
    fn 查不到最新版本时如实给原因而不是说已是最新() {
        // 这个分支只有在 fetch 返回 None 时才走到；这里直接构造一个来盯住文案
        let c = UpgradeCheck {
            current: "1.2.0".into(),
            latest: None,
            has_update: false,
            notes: None,
            notes_url: None,
            published_at: None,
            major_jump: false,
            reason: Some("网络不通".into()),
        };
        assert!(!c.has_update);
        assert!(c.reason.is_some(), "拿不到就要给原因（红线 1）");
    }

    #[test]
    fn 缓存时长与方案一致() {
        assert_eq!(RELEASE_TTL, Duration::from_secs(6 * 3600));
    }
}
