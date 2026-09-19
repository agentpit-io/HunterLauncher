//! 把各模块串成「一次完整安装」的编排层。
//!
//! **GUI 与 headless 共用这里的全部逻辑** —— 两条路径只在「怎么把进度显示出来」上不同，
//! 做的事情一模一样。这样 SSH 用户与桌面用户不会遇到两套行为。
//!
//! 一次安装的顺序：
//!
//! 1. 选镜像源（真实探测，只有探到的才参与）
//! 2. 取 `docker-compose.yml`（固定 tag + 校验 + 内置副本兜底）
//! 3. 解端口冲突 → 写 `.env`（600）与覆盖文件 → `docker compose config` 验一遍
//! 4. 读 manifest 算出每个镜像的压缩大小（进度条的分母）
//! 5. `compose pull`，失败换源重试，最多 3 次
//! 6. `compose up -d` → 轮询健康，最多 180 秒
//!
//! 注意第 3 步在拉取**之前**：compose 要靠 `.env` 里的 `HUNTER_REGISTRY` / `HUNTER_VERSION`
//! 才知道该拉哪些镜像，配置不先写好根本没法拉。方案 §4 的状态机把 `WriteConfig` 画在
//! `Pulling` 之后，那个顺序在真实的 compose 工作流里走不通 —— 已在成果文档里写明这处偏离。

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;

use crate::compose::{self, PullAggregator, PullPhase, PullProgress, ServiceStatus};
use crate::config::{self, LauncherConfig, PortChange, Ports};
use crate::err::{AppError, AppResult, Code};
use crate::gateway;
use crate::registry::{self, ProbeResult};
use crate::{linfo, lwarn, paths};

const NET_TIMEOUT: Duration = Duration::from_secs(25);
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);

/// 进程内的共享状态。**key 只在这里的内存与 `.env` 里**（红线 2）。
pub struct AppState {
    pub cfg: Mutex<LauncherConfig>,
    /// hunter key
    pub key: Mutex<Option<String>>,
    /// 自带模型 key（自带 key 模式才有）
    pub own_key: Mutex<Option<String>>,
    pub pull: Mutex<PullProgress>,
    pub services: Mutex<Vec<ServiceStatus>>,
    pub port_changes: Mutex<Vec<PortChange>>,
    pub probes: Mutex<Vec<ProbeResult>>,
    pub compose_note: Mutex<Option<String>>,
    pub cancel: Arc<AtomicBool>,
    pub busy: AtomicBool,
    pub start_note: Mutex<Option<String>>,
    /// 这一轮安装的镜像是**离线包导入**来的（方案 §9）。为真时 [`pull`] 直接跳过拉取。
    ///
    /// 为什么要一个显式的开关，而不是「本机已经有这六个镜像就跳过」：
    /// 后者会顺手把**正常安装**也变成跳过（开发机、重装的机器上这些层本来就在），
    /// 于是「拉取」这一步在很多机器上悄悄没执行，出问题时谁也说不清它到底跑没跑。
    /// 显式开关只在用户真的导入过离线包时为真，行为可预期。
    pub offline: AtomicBool,
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppState {
    pub fn new() -> Self {
        Self {
            cfg: Mutex::new(LauncherConfig::load()),
            key: Mutex::new(None),
            own_key: Mutex::new(None),
            pull: Mutex::new(PullProgress::empty()),
            services: Mutex::new(Vec::new()),
            port_changes: Mutex::new(Vec::new()),
            probes: Mutex::new(Vec::new()),
            compose_note: Mutex::new(None),
            cancel: Arc::new(AtomicBool::new(false)),
            busy: AtomicBool::new(false),
            start_note: Mutex::new(None),
            offline: AtomicBool::new(false),
        }
    }

    pub fn config(&self) -> LauncherConfig {
        self.cfg.lock().map(|g| g.clone()).unwrap_or_default()
    }

    pub fn set_config(&self, c: LauncherConfig) {
        if let Ok(mut g) = self.cfg.lock() {
            *g = c;
        }
    }

    /// 拿 hunter key：先看内存，没有就从 `.env` 里读回来
    /// （第二次打开启动器时要靠它查额度）。
    pub fn hunter_key(&self) -> Option<String> {
        if let Ok(g) = self.key.lock() {
            if let Some(k) = g.as_ref() {
                return Some(k.clone());
            }
        }
        let k = config::parse_env_file(&paths::env_file())
            .get("HUNTER_API_KEY")
            .cloned()
            .filter(|s| !s.is_empty())?;
        crate::redact::register_secret(&k);
        if let Ok(mut g) = self.key.lock() {
            *g = Some(k.clone());
        }
        Some(k)
    }

    pub fn set_hunter_key(&self, k: &str) {
        crate::redact::register_secret(k);
        if let Ok(mut g) = self.key.lock() {
            *g = Some(k.to_string());
        }
    }

    pub fn set_own_key(&self, k: &str) {
        crate::redact::register_secret(k);
        if let Ok(mut g) = self.own_key.lock() {
            *g = Some(k.to_string());
        }
    }

    /// 记一条遥测事件。**开关默认关着**，绝大多数机器上这一句什么都不做
    /// （`telemetry::record` 第一件事就是看这个布尔值）。
    pub fn tele(&self, event: &str, fields: &[(&str, crate::telemetry::Field)]) {
        let c = self.config();
        crate::telemetry::record(c.telemetry.enabled, &c.telemetry.install_id, event, fields);
    }
}

/// 一次安装的入参。
#[derive(Debug, Clone)]
pub struct InstallOptions {
    /// 固定用哪个源（`ghcr` / `tencent` / 自定义前缀）。None = 测速自动选
    pub registry: Option<String>,
    pub tag: String,
}

impl Default for InstallOptions {
    fn default() -> Self {
        Self {
            registry: None,
            tag: config::BUNDLED_TAG.to_string(),
        }
    }
}

/// 第 1–4 步：准备。返回定下来的镜像清单与每镜像大小。
pub fn prepare(
    state: &AppState,
    opts: &InstallOptions,
    mut note: impl FnMut(&str),
) -> AppResult<PrepareResult> {
    paths::ensure_dirs()?;
    let mut cfg = state.config();
    cfg.hunter.tag = opts.tag.clone();

    // ① 镜像源
    let (cand, probes) = match opts.registry.as_deref() {
        Some(id) if registry::by_id(id).is_some() => {
            let c = registry::by_id(id).unwrap();
            let p = registry::probe(c, &opts.tag, PROBE_TIMEOUT);
            if !p.available {
                return Err(AppError::new(
                    Code::PullFailed,
                    format!(
                        "指定的镜像源 {} 上拉不到 {}：{}",
                        c.label,
                        opts.tag,
                        p.detail.clone().unwrap_or_default()
                    ),
                ));
            }
            note(&format!("镜像源：{}（{} ms，指定）", p.label, p.elapsed_ms));
            (c, vec![p])
        }
        Some(prefix) => {
            // 用户手填的自定义源：探不了 manifest（不知道仓库路径），直接信任并写清楚
            let c: &'static registry::Candidate = Box::leak(Box::new(registry::custom(prefix)));
            note(&format!(
                "镜像源：自定义 {}（用户手填，没有做可用性探测）",
                c.prefix
            ));
            (c, Vec::new())
        }
        None => {
            note("正在测速候选镜像源…");
            let (c, p) = registry::choose(&opts.tag, PROBE_TIMEOUT)?;
            for r in &p {
                note(&format!(
                    "  {} {} · {} ms{}",
                    if r.available { "可用" } else { "不可用" },
                    r.label,
                    r.elapsed_ms,
                    r.detail
                        .as_ref()
                        .map(|d| format!(" · {d}"))
                        .unwrap_or_default()
                ));
            }
            note(&format!("选定：{}", c.label));
            (c, p)
        }
    };
    cfg.apply_registry(cand);
    if let Ok(mut g) = state.probes.lock() {
        *g = probes;
    }

    // ② compose 文件
    let (yml, src, cnote) = config::fetch_compose(&opts.tag, NET_TIMEOUT);
    config::write_compose(&yml)?;
    std::fs::write(paths::version_file(), format!("{}\n", opts.tag)).ok();
    match src {
        config::ComposeSource::Download => note(&format!(
            "compose：已按 tag v{} 下载并校验（{} 字节）",
            opts.tag,
            yml.len()
        )),
        config::ComposeSource::Bundled => note(&format!(
            "compose：用启动器内置的 {} 副本",
            config::BUNDLED_TAG
        )),
    }
    if let Some(n) = &cnote {
        note(n);
    }
    if let Ok(mut g) = state.compose_note.lock() {
        *g = cnote;
    }

    // ③ 端口 + .env + 覆盖文件
    // 先问清楚哪些端口是**我们自己这一套**正占着的：重装 / 第二次打开时它们不算冲突
    let own_ports: Vec<u16> = compose::ps()
        .unwrap_or_default()
        .iter()
        .filter_map(|s| s.port)
        .collect();
    if !own_ports.is_empty() {
        note(&format!(
            "现有的这一套 hunter 正占着端口 {own_ports:?}，这些不算冲突"
        ));
    }
    let (ports, changes) = config::resolve_ports(&cfg.hunter.ports, &own_ports)?;
    for c in &changes {
        note(&format!(
            "端口 {} 被占用，自动改用 {}（服务 {}）",
            c.wanted, c.actual, c.service
        ));
        lwarn!("端口冲突：{} {} → {}", c.service, c.wanted, c.actual);
    }
    cfg.hunter.ports = ports.clone();
    if let Ok(mut g) = state.port_changes.lock() {
        *g = changes.clone();
    }

    let hunter_key = state
        .hunter_key()
        .ok_or_else(|| AppError::new(Code::KeyInvalid, "还没有填 hunter key".to_string()))?;
    let own_key = state
        .own_key
        .lock()
        .ok()
        .and_then(|g| g.clone())
        .unwrap_or_default();
    let (llm_base, llm_model, llm_key, sanitize) = if cfg.model.mode == "own" {
        (
            cfg.model.base_url.clone(),
            cfg.model.model.clone(),
            own_key,
            cfg.model.schema_sanitize,
        )
    } else {
        (
            gateway::LLM_BASE_URL.to_string(),
            gateway::DEFAULT_MODEL.to_string(),
            hunter_key.clone(),
            false,
        )
    };
    config::write_env(&config::EnvInput {
        tag: &opts.tag,
        registry_prefix: &cfg.hunter.registry_prefix,
        ports: &ports,
        hunter_key: &hunter_key,
        model_mode: &cfg.model.mode,
        llm_base_url: &llm_base,
        llm_model: &llm_model,
        llm_api_key: &llm_key,
        schema_sanitize: sanitize,
    })?;
    config::write_override(&ports, &cfg.hunter.base_prefix)?;
    note(&format!(
        "已写 {}（权限 600）与覆盖文件；端口 web {} · api {} · opencode {} · postgres {} · redis {}",
        paths::env_file().display(),
        ports.web,
        ports.api,
        ports.opencode,
        ports.postgres,
        ports.redis
    ));

    // 真让 compose 解析一遍，写错了现在就知道，不用等拉完 800 MB
    let rendered = compose::config_check_json()?;
    verify_bindings(&rendered)?;
    note("docker compose config 校验通过，且除 web 外的端口都绑在 127.0.0.1");

    cfg.save()?;
    state.set_config(cfg.clone());

    // ④ 每镜像大小（进度条的分母）
    let specs = config::images(
        &cfg.hunter.registry_prefix,
        &cfg.hunter.base_prefix,
        &opts.tag,
    );
    let arch = registry::oci_arch();
    let mut sizes: BTreeMap<String, u64> = BTreeMap::new();
    for s in &specs {
        match registry::compressed_size(&s.host, &s.repo, &s.tag, arch, NET_TIMEOUT) {
            Ok(n) => {
                sizes.insert(s.service.clone(), n);
            }
            Err(e) => lwarn!(
                "读不到 {} 的 manifest（进度条分母会退而用观测值）：{}",
                s.reference,
                e.msg
            ),
        }
    }
    let total: u64 = sizes.values().sum();
    if total > 0 {
        note(&format!(
            "六个镜像合计约 {}（linux/{arch} 压缩后，来自 manifest）",
            human_bytes(total)
        ));
    } else {
        note("读不到 manifest，进度条的总量会在拉取过程中逐步补齐");
    }

    Ok(PrepareResult {
        specs,
        sizes,
        registry_prefix: cfg.hunter.registry_prefix.clone(),
        registry_label: cand.label.to_string(),
        ports,
        changes,
    })
}

pub struct PrepareResult {
    pub specs: Vec<config::ImageSpec>,
    pub sizes: BTreeMap<String, u64>,
    pub registry_prefix: String,
    pub registry_label: String,
    pub ports: Ports,
    pub changes: Vec<PortChange>,
}

/// 红线 4 的机器自查：从 `docker compose config --format json` 的渲染结果里确认
/// **除 web 之外的发布端口全部绑在 127.0.0.1**。
///
/// 为什么读 JSON 而不是看覆盖文件：覆盖文件只是我们写进去的「意图」，
/// 而 `compose config` 给的是**合并之后的最终结果** —— compose 对 `ports` 默认做追加合并，
/// `!override` 标签一旦写漏，只有在这里才看得出来（M0 §3.4 踩过）。
pub fn verify_bindings(rendered_json: &str) -> AppResult<()> {
    let v: serde_json::Value = serde_json::from_str(rendered_json).map_err(|e| {
        AppError::new(
            Code::ConfigWrite,
            format!("compose config 的 JSON 解析不了：{e}"),
        )
    })?;
    let services = v
        .get("services")
        .and_then(|s| s.as_object())
        .ok_or_else(|| {
            AppError::new(
                Code::ConfigWrite,
                "compose config 里没有 services".to_string(),
            )
        })?;

    let mut bad: Vec<String> = Vec::new();
    for (name, svc) in services {
        let Some(ports) = svc.get("ports").and_then(|p| p.as_array()) else {
            continue;
        };
        for p in ports {
            let host_ip = p.get("host_ip").and_then(|x| x.as_str()).unwrap_or("");
            let published = p
                .get("published")
                .map(|x| x.to_string())
                .unwrap_or_default();
            if name == "web" {
                continue; // web 按设计对外
            }
            if host_ip != "127.0.0.1" {
                bad.push(format!(
                    "{name} 的 {published} 绑在「{}」",
                    if host_ip.is_empty() {
                        "所有网卡"
                    } else {
                        host_ip
                    }
                ));
            }
        }
    }
    if bad.is_empty() {
        Ok(())
    } else {
        Err(AppError::new(
            Code::ConfigWrite,
            format!(
                "这些端口没有收回 127.0.0.1，不能继续（总控规则红线 4）：{}",
                bad.join("、")
            ),
        ))
    }
}

/// 第 5 步：拉镜像。失败自动换源重试，最多 [`compose::PULL_ATTEMPTS`] 次。
pub fn pull(
    state: &AppState,
    prep: &PrepareResult,
    opts: &InstallOptions,
    mut on_progress: impl FnMut(&PullProgress),
) -> AppResult<()> {
    let mut specs = prep.specs.clone();
    let mut sizes = prep.sizes.clone();
    let mut label = prep.registry_label.clone();
    let mut prefix = prep.registry_prefix.clone();
    let mut last_err: Option<AppError> = None;

    // 离线包模式：镜像已经在本机了，一个字节都不用下（方案 §9）。
    // 但仍然要**复核一遍六个镜像真在**，不然「跳过拉取」就成了一句空话，
    // 到 `up -d` 才发现少东西（那时候报出来的错更难懂）。
    if state.offline.load(Ordering::Relaxed) {
        let (ok, missing) = crate::offline::all_present(&specs);
        if ok {
            let mut agg = PullAggregator::new(&specs, &sizes, &prefix, &label);
            let mut snap = agg.snapshot(PullPhase::Done, None);
            snap.percent = 100;
            for i in snap.images.iter_mut() {
                i.state = compose::PullState::Done;
                if i.total_bytes > 0 {
                    i.downloaded_bytes = i.total_bytes;
                }
            }
            snap.log
                .push("离线包已导入，六个镜像都在本机，跳过拉取".to_string());
            if let Ok(mut g) = state.pull.lock() {
                *g = snap.clone();
            }
            on_progress(&snap);
            linfo!("离线模式：六个镜像都在本机，跳过拉取");
            return Ok(());
        }
        lwarn!("标了离线模式，但本机还缺 {missing:?}，照常联网拉");
    }

    for attempt in 1..=compose::PULL_ATTEMPTS as u32 {
        let mut agg = PullAggregator::new(&specs, &sizes, &prefix, &label);
        agg.set_attempt(attempt);
        let snap = agg.snapshot(PullPhase::Pulling, None);
        on_progress(&snap);
        if let Ok(mut g) = state.pull.lock() {
            *g = snap;
        }

        linfo!("第 {attempt} 次拉取，镜像源 {prefix}");
        let r = compose::pull_streaming(&mut agg, &state.cancel, |p| {
            if let Ok(mut g) = state.pull.lock() {
                *g = p.clone();
            }
            on_progress(p);
        });

        match r {
            Ok(()) => {
                let mut snap = agg.snapshot(PullPhase::Done, None);
                snap.percent = 100;
                for i in snap.images.iter_mut() {
                    i.state = compose::PullState::Done;
                    if i.total_bytes > 0 {
                        i.downloaded_bytes = i.total_bytes;
                    }
                }
                if let Ok(mut g) = state.pull.lock() {
                    *g = snap.clone();
                }
                on_progress(&snap);
                linfo!("拉取完成，用时 {} 秒", agg.elapsed().as_secs());
                state.tele(
                    "pull_done",
                    &[
                        ("registry", crate::telemetry::Field::Enum(prefix.clone())),
                        (
                            "total_mb",
                            crate::telemetry::Field::Int(
                                (sizes.values().sum::<u64>() / 1_000_000) as i64,
                            ),
                        ),
                        (
                            "duration_ms",
                            crate::telemetry::Field::Int(agg.elapsed().as_millis() as i64),
                        ),
                        ("retries", crate::telemetry::Field::Int(attempt as i64 - 1)),
                    ],
                );
                return Ok(());
            }
            Err(e) => {
                lwarn!("第 {attempt} 次拉取失败：{}", e.msg);
                let mut snap = agg.snapshot(PullPhase::Failed, Some(e.msg.clone()));
                snap.attempt = attempt;
                if let Ok(mut g) = state.pull.lock() {
                    *g = snap.clone();
                }
                on_progress(&snap);
                if state.cancel.load(Ordering::Relaxed) {
                    return Err(e);
                }
                last_err = Some(e);

                // 换源：只在「没有指定源」时才换，指定了就老实在同一个源上重试
                if opts.registry.is_none() && attempt < compose::PULL_ATTEMPTS as u32 {
                    if let Some(next) = next_registry(&prefix, &opts.tag) {
                        linfo!("换到镜像源 {} 重试", next.label);
                        prefix = next.prefix.to_string();
                        label = next.label.to_string();
                        let mut cfg = state.config();
                        cfg.apply_registry(next);
                        let _ = cfg.save();
                        // 换源要重写 .env 与覆盖文件里的镜像地址，再重算大小
                        rewrite_registry(state, &cfg, &opts.tag)?;
                        specs = config::images(
                            &cfg.hunter.registry_prefix,
                            &cfg.hunter.base_prefix,
                            &opts.tag,
                        );
                        sizes = BTreeMap::new();
                        for s in &specs {
                            if let Ok(n) = registry::compressed_size(
                                &s.host,
                                &s.repo,
                                &s.tag,
                                registry::oci_arch(),
                                NET_TIMEOUT,
                            ) {
                                sizes.insert(s.service.clone(), n);
                            }
                        }
                        state.set_config(cfg);
                    }
                }
                std::thread::sleep(Duration::from_secs(2));
            }
        }
    }
    Err(last_err.unwrap_or_else(|| AppError::new(Code::PullFailed, "拉取失败".to_string())))
}

/// 换源之后重写 `.env` 的 `HUNTER_REGISTRY` 与覆盖文件里 postgres/redis 的镜像地址。
fn rewrite_registry(state: &AppState, cfg: &LauncherConfig, tag: &str) -> AppResult<()> {
    rewrite_env_and_override(state, cfg, tag)
}

/// 用**当前配置 + 内存或 `.env` 里的 key**把 `.env` 与覆盖文件重写一遍，tag 换成给的这个。
///
/// 三个地方要做同一件事：换源重试（[`rewrite_registry`]）、Hunter 升级（[`crate::upgrade`]）、
/// 切模型模式（[`apply_model_change`] 用的是自己那份，因为它还要处理"内存里没有自带 key"）。
/// `write_env` 自带 sticky：JWT_SECRET / 数据库口令 / opencode 口令都会原样读回来，绝不换新。
pub fn rewrite_env_and_override(
    state: &AppState,
    cfg: &LauncherConfig,
    tag: &str,
) -> AppResult<()> {
    let hunter_key = state
        .hunter_key()
        .ok_or_else(|| AppError::new(Code::KeyInvalid, "内存里没有 key 了".to_string()))?;
    let own_key = state
        .own_key
        .lock()
        .ok()
        .and_then(|g| g.clone())
        .unwrap_or_default();
    let (llm_base, llm_model, llm_key, sanitize) = if cfg.model.mode == "own" {
        // 自带 key 模式下内存里可能没有那把 key（重开了启动器），从现有的 .env 读回来。
        // 读不回来就宁可报错也不把 hunter key 当成模型 key 写进去。
        let k = if own_key.is_empty() {
            config::parse_env_file(&paths::env_file())
                .get("LLM_API_KEY")
                .cloned()
                .unwrap_or_default()
        } else {
            own_key
        };
        if k.is_empty() || k == hunter_key {
            return Err(AppError::new(
                Code::KeyInvalid,
                "当前是自带模型 key 模式，但读不回那把 key，没法重写 .env".to_string(),
            ));
        }
        crate::redact::register_secret(&k);
        (
            cfg.model.base_url.clone(),
            cfg.model.model.clone(),
            k,
            cfg.model.schema_sanitize,
        )
    } else {
        (
            gateway::LLM_BASE_URL.to_string(),
            gateway::DEFAULT_MODEL.to_string(),
            hunter_key.clone(),
            false,
        )
    };
    config::write_env(&config::EnvInput {
        tag,
        registry_prefix: &cfg.hunter.registry_prefix,
        ports: &cfg.hunter.ports,
        hunter_key: &hunter_key,
        model_mode: &cfg.model.mode,
        llm_base_url: &llm_base,
        llm_model: &llm_model,
        llm_api_key: &llm_key,
        schema_sanitize: sanitize,
    })?;
    config::write_override(&cfg.hunter.ports, &cfg.hunter.base_prefix)
}

fn next_registry(current_prefix: &str, tag: &str) -> Option<&'static registry::Candidate> {
    registry::CANDIDATES
        .iter()
        .filter(|c| c.prefix != current_prefix)
        .find(|c| registry::probe(c, tag, PROBE_TIMEOUT).available)
}

/// 第 6 步：起容器 + 等健康。
pub fn start(
    state: &AppState,
    mut on_tick: impl FnMut(&[ServiceStatus]),
) -> AppResult<Vec<ServiceStatus>> {
    let t0 = std::time::Instant::now();
    compose::up()?;
    let r = compose::wait_healthy(compose::START_TIMEOUT, |v| {
        if let Ok(mut g) = state.services.lock() {
            *g = v.to_vec();
        }
        on_tick(v);
    });
    match r {
        Ok(v) => {
            let mut cfg = state.config();
            cfg.install.done = true;
            cfg.install.at = crate::timefmt::now_shanghai();
            let _ = cfg.save();
            state.set_config(cfg);
            if let Ok(mut g) = state.services.lock() {
                *g = v.clone();
            }
            let remapped = state
                .port_changes
                .lock()
                .map(|g| !g.is_empty())
                .unwrap_or(false);
            state.tele(
                "hunter_started",
                &[
                    (
                        "hunter_tag",
                        crate::telemetry::Field::Enum(state.config().hunter.tag),
                    ),
                    (
                        "duration_ms",
                        crate::telemetry::Field::Int(t0.elapsed().as_millis() as i64),
                    ),
                    ("ports_remapped", crate::telemetry::Field::Bool(remapped)),
                ],
            );
            Ok(v)
        }
        Err(e) => {
            if let Ok(mut g) = state.start_note.lock() {
                *g = Some(e.msg.clone());
            }
            state.tele(
                "error",
                &[
                    ("component", crate::telemetry::Field::Enum("start".into())),
                    (
                        "error_code",
                        crate::telemetry::Field::Enum(e.code.as_str().into()),
                    ),
                ],
            );
            Err(e)
        }
    }
}

// ── 运行面板的数据 ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvRow {
    pub key: String,
    pub value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatus {
    pub hunter_tag: Option<String>,
    pub quota: Option<gateway::QuotaInfo>,
    pub latest_tag: Option<String>,
    pub running: bool,
    pub uptime_seconds: Option<u64>,
    pub web_url: Option<String>,
    pub services: Vec<ServiceStatus>,
    pub env: Vec<EnvRow>,
    pub log: Vec<String>,
    /// api 的 `/api/health` 说这个实例到底有没有配上 key（M0 §3.1 的意外收获）
    pub api_key_configured: Option<bool>,
    pub installed: bool,
    /// 从本机 api 读回来的那几项（数据源卡片靠它）。读不到就是 `reachable=false` + 原因
    pub upstream: crate::upstream::UpstreamFacts,
    /// 「数据源」卡片的主值与副行。主值为 `None` 时界面显示「—」
    pub data_source: Option<String>,
    pub data_source_sub: String,
    /// 上游确实没有、因此只能显示「—」的几项（前端按 id 取说明）
    pub missing: Vec<MissingEndpoint>,
}

/// 一条「需上游配合」的接口。界面与成果文档用的是同一份清单。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MissingEndpoint {
    pub id: String,
    pub endpoint: String,
}

pub fn missing_endpoints() -> Vec<MissingEndpoint> {
    crate::upstream::MISSING_ENDPOINTS
        .iter()
        .map(|(id, ep)| MissingEndpoint {
            id: (*id).to_string(),
            endpoint: (*ep).to_string(),
        })
        .collect()
}

/// 组装运行面板要的全部数据。每一项拿不到就是 `None` + 原因，绝不填假值（红线 1）。
pub fn runtime_status(state: &AppState) -> RuntimeStatus {
    let cfg = state.config();
    let services = compose::ps().unwrap_or_default();
    let running = services.iter().any(|s| s.state == "running");
    let web_port = services
        .iter()
        .find(|s| s.service == "web")
        .and_then(|s| s.port)
        .unwrap_or(cfg.hunter.ports.web);
    let web_url = if running {
        Some(format!("http://localhost:{web_port}"))
    } else {
        None
    };

    let api_port = services
        .iter()
        .find(|s| s.service == "api")
        .and_then(|s| s.port)
        .unwrap_or(cfg.hunter.ports.api);
    let up = crate::upstream::fetch(api_port, Duration::from_secs(5));
    let api_ok = up.api_key_configured;
    let uptime = container_uptime();
    let (data_source, data_source_sub) = crate::upstream::data_source_label(&up);

    let quota = state.hunter_key().and_then(|k| cached_quota(&k));
    let latest_tag = latest_hunter_tag();

    let docker = cfg_docker_label();
    let model_label = if cfg.model.mode == "own" {
        format!(
            "自带 · {}",
            if cfg.model.model.is_empty() {
                "—".into()
            } else {
                cfg.model.model.clone()
            }
        )
    } else {
        format!("网关 · {}", gateway::DEFAULT_MODEL)
    };

    let env = vec![
        EnvRow {
            key: "model".into(),
            value: Some(model_label),
            reason: None,
        },
        EnvRow {
            key: "docker".into(),
            value: docker,
            reason: Some("docker version 读不到".into()),
        },
        EnvRow {
            key: "registry".into(),
            value: registry::by_id(&cfg.hunter.registry_id)
                .map(|c| c.label.to_string())
                .or(Some(cfg.hunter.registry_prefix.clone())),
            reason: None,
        },
        EnvRow {
            key: "autostart".into(),
            value: Some(if cfg.launcher.autostart {
                "on".into()
            } else {
                "off".into()
            }),
            reason: None,
        },
        EnvRow {
            key: "telemetry".into(),
            value: Some(if cfg.telemetry.enabled {
                "on".into()
            } else {
                "off".into()
            }),
            reason: None,
        },
    ];

    let mut log = compose::logs(None, 40).unwrap_or_default();
    if log.is_empty() {
        log = crate::log::tail_file(40);
    }

    RuntimeStatus {
        hunter_tag: Some(cfg.hunter.tag.clone()),
        quota,
        latest_tag,
        running,
        uptime_seconds: uptime,
        web_url,
        services,
        env,
        log,
        api_key_configured: api_ok,
        installed: cfg.install.done,
        upstream: up,
        data_source,
        data_source_sub,
        missing: missing_endpoints(),
    }
}

/// 从 `docker inspect` 读 web 容器的启动时间算运行时长。
fn container_uptime() -> Option<u64> {
    let name = format!("{}-web-1", config::PROJECT);
    let r = crate::proc::run_timeout(
        "docker",
        &["inspect", "-f", "{{.State.StartedAt}}", &name],
        Duration::from_secs(10),
    )
    .ok()?;
    if !r.ok() {
        return None;
    }
    let started = parse_rfc3339_secs(r.stdout.trim())?;
    let now = crate::timefmt::now_unix();
    if now > started {
        Some((now - started) as u64)
    } else {
        Some(0)
    }
}

/// `2026-09-19T05:14:41.123456789Z` → Unix 秒。只吃 UTC 的 `Z` 形式（docker 就是这么输出的）。
pub fn parse_rfc3339_secs(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.len() < 20 || !s.contains('T') {
        return None;
    }
    let (date, rest) = s.split_once('T')?;
    let time = rest.split(['.', 'Z', '+']).next()?;
    let d: Vec<i64> = date.split('-').filter_map(|x| x.parse().ok()).collect();
    let t: Vec<i64> = time.split(':').filter_map(|x| x.parse().ok()).collect();
    if d.len() != 3 || t.len() != 3 {
        return None;
    }
    Some(days_from_civil(d[0], d[1] as u32, d[2] as u32) * 86_400 + t[0] * 3600 + t[1] * 60 + t[2])
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn cfg_docker_label() -> Option<String> {
    let d = crate::runtime::docker::detect();
    d.runtime_label.map(|l| match d.arch {
        Some(a) => format!("{l} · {a}"),
        None => l,
    })
}

/// 切换模型模式（网关 ↔ 自带 key）。里程碑 M3 第 3 项：
/// **切换后重写 `.env` 并重启相关服务**。
///
/// 三步，缺一不可：
///
/// 1. 用**当前配置 + 内存里的 key** 重新渲染一次 `.env`（`write_env` 自带 sticky：
///    JWT_SECRET / 数据库口令 / opencode 口令都会被原样读回来，绝不换新）；
/// 2. `docker compose up -d --force-recreate api opencode llm-shim` ——
///    只有重新创建容器才会带上新的环境变量（见 `compose::up_services` 的注释）；
/// 3. 等这三个服务重新健康。
///
/// web / postgres / redis 不动：它们不读 `LLM_*`，白重启一遍只是让用户多等。
pub fn apply_model_change(state: &AppState) -> AppResult<String> {
    let cfg = state.config();
    let hunter_key = state
        .hunter_key()
        .ok_or_else(|| AppError::new(Code::KeyInvalid, "读不到 hunter key，没法重写 .env"))?;
    let own_key = state
        .own_key
        .lock()
        .ok()
        .and_then(|g| g.clone())
        .unwrap_or_default();

    let (llm_base, llm_model, llm_key, sanitize) = if cfg.model.mode == "own" {
        if own_key.is_empty() {
            // 内存里没有自带 key（比如重开了启动器），从 .env 里读回上一次写进去的那把
            let from_env = config::parse_env_file(&paths::env_file())
                .get("LLM_API_KEY")
                .cloned()
                .unwrap_or_default();
            if from_env.is_empty() || from_env == hunter_key {
                return Err(AppError::new(
                    Code::KeyInvalid,
                    "切到自带 key 模式要先填一次你自己的 key（启动器不保存它的副本）",
                ));
            }
            crate::redact::register_secret(&from_env);
            (
                cfg.model.base_url.clone(),
                cfg.model.model.clone(),
                from_env,
                cfg.model.schema_sanitize,
            )
        } else {
            (
                cfg.model.base_url.clone(),
                cfg.model.model.clone(),
                own_key,
                cfg.model.schema_sanitize,
            )
        }
    } else {
        (
            gateway::LLM_BASE_URL.to_string(),
            gateway::DEFAULT_MODEL.to_string(),
            hunter_key.clone(),
            false,
        )
    };

    config::write_env(&config::EnvInput {
        tag: &cfg.hunter.tag,
        registry_prefix: &cfg.hunter.registry_prefix,
        ports: &cfg.hunter.ports,
        hunter_key: &hunter_key,
        model_mode: &cfg.model.mode,
        llm_base_url: &llm_base,
        llm_model: &llm_model,
        llm_api_key: &llm_key,
        schema_sanitize: sanitize,
    })?;
    linfo!(
        "已按模型模式 {} 重写 .env（模型 {}，地址 {}）",
        cfg.model.mode,
        llm_model,
        crate::http::host_of(&llm_base)
    );

    // 容器没在跑就只写配置，不去硬起 —— 用户可能就是想先改设置再启动
    if !compose::is_up() {
        return Ok(format!(
            "已重写 .env（模型模式 {}）。容器当前没在运行，下次启动就会用新配置。",
            cfg.model.mode
        ));
    }

    compose::up_services(&["api", "opencode", "llm-shim"])?;
    let deadline = std::time::Instant::now() + Duration::from_secs(180);
    loop {
        let list = compose::ps().unwrap_or_default();
        let done = ["api", "opencode", "llm-shim"].iter().all(|n| {
            list.iter()
                .any(|s| s.service == *n && compose::service_ready(s))
        });
        if done {
            return Ok(format!(
                "已重写 .env 并重建 api / opencode / llm-shim，三个服务都已就绪（模型模式 {}，模型 {llm_model}）。",
                cfg.model.mode
            ));
        }
        if std::time::Instant::now() >= deadline {
            let bad: Vec<String> = ["api", "opencode", "llm-shim"]
                .iter()
                .filter(|n| {
                    !list
                        .iter()
                        .any(|s| s.service == **n && compose::service_ready(s))
                })
                .map(|n| (*n).to_string())
                .collect();
            return Err(AppError::new(
                Code::StartTimeout,
                format!(
                    ".env 已经改好了，但等了 180 秒这几个服务还没就绪：{}。用「查看日志」看它们的输出。",
                    bad.join("、")
                ),
            ));
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

/// 额度的缓存时长。运行面板每 10 秒刷一次，额度没必要跟着那么勤。
const QUOTA_TTL: Duration = Duration::from_secs(60);

/// Hunter 的最新版本。**取回来的整条 Release（含 Release Notes）缓存 6 小时**，
/// 实现在 [`crate::upgrade`] 里，这里只是运行面板与托盘用惯了的那个薄壳。
pub fn latest_hunter_tag() -> Option<String> {
    crate::upgrade::latest_release().map(|r| r.tag)
}

/// 用户**亲手点了「检查更新」**时用这个：绕过缓存，真去问一次，并把缓存刷新掉。
pub fn latest_hunter_tag_now() -> Option<String> {
    crate::upgrade::latest_release_now().map(|r| r.tag)
}

/// 一格「值 + 取到它的时刻」。
pub type CacheSlot<T> = Mutex<Option<(std::time::Instant, T)>>;

/// 带 TTL 的一格缓存。没过期就用旧值，过期了才调 `f`。
///
/// 抽成一个函数是为了**能测** —— 时间相关的逻辑直接写在业务函数里就只能靠真等，
/// 而这里只要传一个 0 的 TTL 就能验「过期之后确实重新取了」。
pub fn cached<T: Clone>(slot: &CacheSlot<T>, ttl: Duration, f: impl FnOnce() -> T) -> T {
    if let Ok(g) = slot.lock() {
        if let Some((at, v)) = g.as_ref() {
            if at.elapsed() < ttl {
                return v.clone();
            }
        }
    }
    let fresh = f();
    if let Ok(mut g) = slot.lock() {
        *g = Some((std::time::Instant::now(), fresh.clone()));
    }
    fresh
}

/// 今日额度，**结果缓存 60 秒**。
///
/// 同样是因为运行面板每 10 秒刷一次：额度是一次跨公网的往返，而且这把 key 有
/// 每分钟 20 次的限流 —— 面板自己就能把限流吃掉一小半。
fn cached_quota(key: &str) -> Option<gateway::QuotaInfo> {
    static CACHE: CacheSlot<Option<gateway::QuotaInfo>> = Mutex::new(None);
    cached(&CACHE, QUOTA_TTL, || {
        gateway::quota(key, Duration::from_secs(12)).ok()
    })
}

pub fn human_bytes(n: u64) -> String {
    const U: [(&str, f64); 4] = [("GB", 1e9), ("MB", 1e6), ("KB", 1e3), ("B", 1.0)];
    for (u, s) in U {
        if n as f64 >= s {
            let v = n as f64 / s;
            return if v >= 100.0 || s == 1.0 {
                format!("{v:.0} {u}")
            } else {
                format!("{v:.1} {u}")
            };
        }
    }
    format!("{n} B")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 运行面板每 10 秒刷一次状态。GitHub 对未认证请求的限额是**每小时 60 次** ——
    /// 版本检查不缓存的话，一个用户开着面板十分钟就把额度用光，
    /// 之后「有新版本」角标会无声无息地不再出现。
    ///
    /// 这条测试验的是缓存本身：TTL 内只取一次，TTL 过了才重新取。
    #[test]
    fn 带_ttl_的缓存在有效期内只取一次() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let hits = AtomicUsize::new(0);
        let slot: CacheSlot<u32> = Mutex::new(None);
        let take = || {
            hits.fetch_add(1, Ordering::SeqCst);
            7u32
        };

        assert_eq!(cached(&slot, Duration::from_secs(3600), take), 7);
        assert_eq!(cached(&slot, Duration::from_secs(3600), take), 7);
        assert_eq!(cached(&slot, Duration::from_secs(3600), take), 7);
        assert_eq!(hits.load(Ordering::SeqCst), 1, "TTL 内不该重复去取");

        // TTL 为 0 = 每次都过期，必须重新取
        assert_eq!(cached(&slot, Duration::ZERO, take), 7);
        assert_eq!(hits.load(Ordering::SeqCst), 2, "过期之后必须重新取");
    }

    /// 手动点「检查更新」必须绕过缓存 —— 否则那个按钮等于没有。
    #[test]
    fn 手动检查更新与自动刷新共用一格缓存且手动必定重新取() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let hits = AtomicUsize::new(0);
        let slot: CacheSlot<u32> = Mutex::new(None);
        let take = || hits.fetch_add(1, Ordering::SeqCst) as u32 + 1;

        // 自动刷新：第一次取，之后走缓存
        assert_eq!(cached(&slot, Duration::from_secs(3600), take), 1);
        assert_eq!(cached(&slot, Duration::from_secs(3600), take), 1);
        assert_eq!(hits.load(Ordering::SeqCst), 1);

        // 手动（TTL=0）：必定重新取
        assert_eq!(cached(&slot, Duration::ZERO, take), 2);
        assert_eq!(hits.load(Ordering::SeqCst), 2);

        // 而且刚取回来的值写进了**同一格**，自动刷新拿到的是新值而不是旧的 1
        assert_eq!(cached(&slot, Duration::from_secs(3600), take), 2);
        assert_eq!(hits.load(Ordering::SeqCst), 2, "自动刷新不该又去取一次");
    }

    /// 方案 §11.3 写死的是「缓存 6 小时」。版本检查的那一格搬去 `upgrade` 了，
    /// 这里只盯额度的缓存。
    #[test]
    fn 额度的缓存要比面板刷新间隔长() {
        // 额度的缓存要比面板的刷新间隔（10 秒）长，否则等于没缓存
        assert!(QUOTA_TTL >= Duration::from_secs(30), "{QUOTA_TTL:?}");
    }

    #[test]
    fn 字节可读化() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(999), "999 B");
        assert_eq!(human_bytes(1_500), "1.5 KB");
        assert_eq!(human_bytes(849_198_109), "849 MB");
        assert_eq!(human_bytes(2_100_000_000), "2.1 GB");
    }

    #[test]
    fn rfc3339_解析() {
        // 2026-09-19T05:14:41Z
        assert_eq!(
            parse_rfc3339_secs("2026-09-19T05:14:41.123456789Z"),
            Some(1_789_794_881)
        );
        assert_eq!(
            parse_rfc3339_secs("2026-09-19T05:14:41Z"),
            Some(1_789_794_881)
        );
        assert_eq!(parse_rfc3339_secs("坏数据"), None);
        assert_eq!(parse_rfc3339_secs(""), None);
    }

    /// 红线 4 的自查函数本身要经得起检验
    #[test]
    fn 端口绑定自查能抓出没绑本机的情况() {
        // 形状取自 `docker compose config --format json` 的真实输出
        let good = serde_json::json!({
            "name": "hunter",
            "services": {
                "api": {"ports": [{"mode": "ingress", "host_ip": "127.0.0.1", "target": 8000, "published": "8101", "protocol": "tcp"}]},
                "opencode": {"ports": [{"host_ip": "127.0.0.1", "target": 3901, "published": "3922"}]},
                "postgres": {"ports": [{"host_ip": "127.0.0.1", "target": 5432, "published": "5443"}]},
                "redis": {"ports": [{"host_ip": "127.0.0.1", "target": 6379, "published": "6480"}]},
                "llm-shim": {},
                "web": {"ports": [{"target": 3000, "published": "3101"}]}
            }
        })
        .to_string();
        verify_bindings(&good).expect("全都绑了本机就该通过");

        // api 少了 host_ip = 监听所有网卡
        let bad = serde_json::json!({
            "services": {
                "api": {"ports": [{"target": 8000, "published": "8101"}]},
                "web": {"ports": [{"target": 3000, "published": "3101"}]}
            }
        })
        .to_string();
        let e = verify_bindings(&bad).unwrap_err();
        assert_eq!(e.code, Code::ConfigWrite);
        assert!(
            e.msg.contains("api") && e.msg.contains("所有网卡"),
            "{}",
            e.msg
        );

        // 绑到 0.0.0.0 也要抓出来
        let bad2 = serde_json::json!({
            "services": {"redis": {"ports": [{"host_ip": "0.0.0.0", "published": "6480"}]}}
        })
        .to_string();
        assert!(verify_bindings(&bad2).is_err());

        // web 对外是设计如此，不能被误报
        let only_web = serde_json::json!({"services": {"web": {"ports": [{"published": "3101"}]}}})
            .to_string();
        assert!(verify_bindings(&only_web).is_ok());

        assert!(verify_bindings("不是 JSON").is_err());
    }

    #[test]
    fn 默认安装选项() {
        let o = InstallOptions::default();
        assert_eq!(o.tag, config::BUNDLED_TAG);
        assert!(o.registry.is_none(), "默认是测速自动选，不是写死某个源");
    }

    #[test]
    fn 状态里的_key_一登记就不会进日志() {
        let _g = crate::log::test_lock();
        let st = AppState::new();
        st.set_hunter_key("hunt_tools_q6sKaaaaaaaaaaaaaaaaaaaaaaaaQMo2");
        let line = crate::log::write_line(
            crate::log::Level::Info,
            "key = hunt_tools_q6sKaaaaaaaaaaaaaaaaaaaaaaaaQMo2",
        );
        assert!(!line.contains("q6sK"), "{line}");
    }
}
