//! Tauri command 层（前端与 Rust 之间的唯一通道）。
//!
//! 约定：失败一律返回 `"<错误码>: <说明>"` 形式的字符串，前端 `src/lib/ipc.ts` 按这个格式
//! 解析出 `IpcError { code, message }`，再由页面显示「—」和原因。
//!
//! **红线 1**：这里没有任何假数据。拿不到就返回 `Err` 或者字段为 `null`，由界面显示「—」。
//! **红线 2**：key 只往 `~/.hunter/app/.env`（600）与进程内存里去，不回显、不进日志。

use std::sync::atomic::Ordering;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager};
use tauri_plugin_opener::OpenerExt;

use crate::compose;
use crate::config::{self, LauncherConfig};
use crate::err::{AppError, Code};
use crate::flow::{self, AppState, InstallOptions, RuntimeStatus};
use crate::gateway;
use crate::registry;
use crate::runtime::docker;
use crate::AppInfo;

type Result<T> = std::result::Result<T, String>;

/// 前端事件名。前端 `ipc.ts` 里 listen 的就是这三个。
pub const EV_PULL: &str = "hunter://pull";
pub const EV_START: &str = "hunter://start";
pub const EV_LOG: &str = "hunter://log";

fn state(app: &tauri::AppHandle) -> tauri::State<'_, AppState> {
    app.state::<AppState>()
}

// ── 基础 ──────────────────────────────────────────────────────────────────

#[tauri::command]
pub fn app_info() -> AppInfo {
    AppInfo {
        launcher_version: env!("CARGO_PKG_VERSION").to_string(),
        platform: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        window_chrome: if cfg!(target_os = "macos") {
            "native"
        } else {
            "custom"
        }
        .to_string(),
    }
}

#[tauri::command]
pub fn window_minimize(app: tauri::AppHandle) -> Result<()> {
    main_window(&app)?
        .minimize()
        .map_err(|e| format!("E_UNKNOWN: {e}"))
}

#[tauri::command]
pub fn window_toggle_maximize(app: tauri::AppHandle) -> Result<()> {
    let w = main_window(&app)?;
    let maximized = w.is_maximized().map_err(|e| format!("E_UNKNOWN: {e}"))?;
    if maximized {
        w.unmaximize()
    } else {
        w.maximize()
    }
    .map_err(|e| format!("E_UNKNOWN: {e}"))
}

#[tauri::command]
pub fn window_close(app: tauri::AppHandle) -> Result<()> {
    main_window(&app)?
        .close()
        .map_err(|e| format!("E_UNKNOWN: {e}"))
}

/// 用系统默认浏览器打开链接。只放行 https 与本机 http，防止被当成任意程序启动器。
#[tauri::command]
pub fn open_external(app: tauri::AppHandle, url: String) -> Result<()> {
    open_url_checked(&app, &url)
}

/// 同一道校验的非 command 版本，托盘那边也走它。
pub fn open_url_checked<R: tauri::Runtime>(app: &tauri::AppHandle<R>, url: &str) -> Result<()> {
    if !url_allowed(url) {
        return Err(format!("E_UNKNOWN: 拒绝打开非 https / 非本机地址：{url}"));
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| format!("E_UNKNOWN: {e}"))
}

/// 放行规则：任意 https，或者**本机**的 http。
///
/// 明文 http 的 authority（`://` 到第一个 `/`、`?`、`#` 之间的那一段）必须**整段**
/// 等于 `localhost` / `127.0.0.1`，后面最多再跟一个纯数字端口。
///
/// 这里不能只比前缀，也不能只看「主机名后面紧跟 `:` 或 `/`」——
/// 两条真实的绕法都被这个函数挡着：
///
/// * `http://localhost.example.com/`（I1 自审发现）：前缀相同但主机不是本机；
/// * `http://localhost:1@evil.com/`（I2 自审发现）：`localhost:1` 在 URL 语法里是
///   **userinfo**，真正的主机是 `evil.com`，浏览器会去开 evil.com。
///   只判「后面紧跟 `:`」的写法会把它放行。
pub fn url_allowed(url: &str) -> bool {
    if url.starts_with("https://") {
        return true;
    }
    let Some(rest) = url.strip_prefix("http://") else {
        return false;
    };
    // authority 到第一个 `/`、`?`、`#` 为止；里面出现 `@` 一律拒绝（那就是 userinfo）
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.contains('@') {
        return false;
    }
    let (host, port) = match authority.split_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (authority, None),
    };
    if !matches!(host, "localhost" | "127.0.0.1") {
        return false;
    }
    match port {
        None => true,
        Some(p) => !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()),
    }
}

/// 在系统文件管理器里定位一个文件（导出诊断包 / 日志之后用）。
/// **只放行 `~/.hunter` 里的路径** —— 这个接口能让网页指使系统打开任意文件，得管住。
#[tauri::command]
pub fn reveal_path(app: tauri::AppHandle, path: String) -> Result<()> {
    let p = std::path::PathBuf::from(&path);
    let root = crate::paths::root();
    let inside = p
        .canonicalize()
        .ok()
        .zip(root.canonicalize().ok())
        .map(|(a, b)| a.starts_with(b))
        .unwrap_or(false);
    if !inside {
        return Err(format!(
            "E_UNKNOWN: 只允许定位 {} 里的文件：{path}",
            root.display()
        ));
    }
    app.opener()
        .reveal_item_in_dir(&p)
        .map_err(|e| format!("E_UNKNOWN: {e}"))
}

/// 启动时问一次：装过没有、栈起着没有。决定直接进运行面板还是走向导。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BootState {
    pub installed: bool,
    pub running: bool,
    pub locale: String,
    pub hunter_tag: String,
    pub work_dir: String,
}

#[tauri::command]
pub async fn boot_state(app: tauri::AppHandle) -> Result<BootState> {
    blocking(move || {
        let st = state(&app);
        let cfg = st.config();
        Ok(BootState {
            installed: cfg.install.done && crate::paths::env_file().exists(),
            running: compose::is_up(),
            locale: cfg.launcher.locale.clone(),
            hunter_tag: cfg.hunter.tag.clone(),
            work_dir: crate::paths::root().to_string_lossy().into_owned(),
        })
    })
    .await
}

// ── Docker ────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn detect_docker(app: tauri::AppHandle) -> Result<docker::DockerInfo> {
    blocking(move || {
        let d = docker::detect();
        crate::linfo!(
            "Docker 检测：installed={} daemon={} runtime={:?} server={:?} compose={:?} 最低版本={}",
            d.installed,
            d.daemon_running,
            d.runtime,
            d.server_version,
            d.compose_version,
            d.meets_minimum
        );
        state(&app).tele(
            "docker_detected",
            &[
                (
                    "runtime",
                    crate::telemetry::Field::Enum(format!("{:?}", d.runtime)),
                ),
                (
                    "version",
                    crate::telemetry::Field::Enum(d.server_version.clone().unwrap_or_default()),
                ),
                (
                    "ok",
                    crate::telemetry::Field::Bool(d.installed && d.daemon_running),
                ),
            ],
        );
        Ok(d)
    })
    .await
}

/// Linux 上试着 `systemctl start docker`；mac / Windows 上如实告诉用户要自己点。
#[tauri::command]
pub async fn start_daemon() -> Result<String> {
    blocking(docker::try_start_daemon).await
}

// ── key ───────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn validate_key(app: tauri::AppHandle, key: String) -> Result<gateway::KeyCheckResult> {
    blocking(move || {
        let r = gateway::check_key(&key, Duration::from_secs(20));
        // 只记结论，不记 key 本身（红线 2）
        crate::linfo!("key 校验：valid={} reason={:?}", r.valid, r.reason);
        if r.valid {
            state(&app).set_hunter_key(&key);
        }
        Ok(r)
    })
    .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelChoice {
    /// gateway | own
    pub mode: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub api_key: String,
}

/// 选模型。自带 key 模式下会**真发一次请求**做连通性检查（里程碑要求 3）。
#[tauri::command]
pub async fn set_model(app: tauri::AppHandle, choice: ModelChoice) -> Result<gateway::OwnKeyCheck> {
    blocking(move || {
        let st = state(&app);
        let mut cfg = st.config();
        if choice.mode == "own" {
            let r = gateway::check_own_key(
                &choice.base_url,
                &choice.model,
                &choice.api_key,
                Duration::from_secs(30),
            );
            crate::linfo!(
                "自带 key 连通性检查：ok={} via={:?} sanitize={}",
                r.ok,
                r.via,
                r.schema_sanitize
            );
            if r.ok {
                cfg.model.mode = "own".into();
                cfg.model.base_url = choice.base_url.trim_end_matches('/').to_string();
                cfg.model.model = choice.model.clone();
                cfg.model.schema_sanitize = r.schema_sanitize;
                st.set_own_key(&choice.api_key);
                let _ = cfg.save();
                st.set_config(cfg);
            }
            Ok(r)
        } else {
            cfg.model.mode = "gateway".into();
            cfg.model.base_url = gateway::LLM_BASE_URL.into();
            cfg.model.model = gateway::DEFAULT_MODEL.into();
            cfg.model.schema_sanitize = false;
            let _ = cfg.save();
            st.set_config(cfg);
            Ok(gateway::OwnKeyCheck {
                ok: true,
                via: Some("gateway".into()),
                message: format!("用 Hunter 内置额度网关 · 模型 {}", gateway::DEFAULT_MODEL),
                schema_sanitize: false,
                models: vec![gateway::DEFAULT_MODEL.into(), "hunter-deep".into()],
            })
        }
    })
    .await
}

// ── 镜像源 ────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn probe_registries(tag: Option<String>) -> Result<Vec<registry::ProbeResult>> {
    blocking(move || {
        let t = tag.unwrap_or_else(|| config::BUNDLED_TAG.to_string());
        Ok(registry::probe_all(&t, Duration::from_secs(20)))
    })
    .await
}

// ── 安装：准备 + 拉取 ─────────────────────────────────────────────────────

/// 启动一次安装。**立刻返回**，进度通过 `hunter://pull` 事件推给前端，
/// 也可以随时用 `pull_progress` 拿一份快照（给刚挂载上来的页面用）。
#[tauri::command]
pub fn start_install(app: tauri::AppHandle, registry_id: Option<String>) -> Result<()> {
    {
        let st = state(&app);
        if st.busy.swap(true, Ordering::SeqCst) {
            return Err(AppError::new(Code::Unknown, "已经有一次安装在进行中").to_string());
        }
        st.cancel.store(false, Ordering::SeqCst);
    }
    let handle = app.clone();
    std::thread::spawn(move || {
        let st = handle.state::<AppState>();
        let cfg = st.config();
        let opts = InstallOptions {
            registry: registry_id,
            tag: cfg.hunter.tag.clone(),
        };

        let h2 = handle.clone();
        let emit_log = move |line: &str| {
            crate::linfo!("{line}");
            let _ = h2.emit(EV_LOG, line.to_string());
        };

        let result = (|| -> crate::err::AppResult<()> {
            let prep = flow::prepare(&st, &opts, emit_log)?;
            {
                let h = handle.clone();
                flow::pull(&st, &prep, &opts, move |p| {
                    let _ = h.emit(EV_PULL, p);
                })?;
            }
            Ok(())
        })();

        if let Err(e) = result {
            crate::lerror!("安装失败：{}", e.msg);
            if let Ok(mut g) = st.pull.lock() {
                g.phase = compose::PullPhase::Failed;
                g.error = Some(crate::redact::redact(&e.msg));
            }
            let snap = st
                .pull
                .lock()
                .map(|g| g.clone())
                .unwrap_or_else(|_| compose::PullProgress::empty());
            let _ = handle.emit(EV_PULL, &snap);
        }
        st.busy.store(false, Ordering::SeqCst);
    });
    Ok(())
}

#[tauri::command]
pub fn cancel_install(app: tauri::AppHandle) -> Result<()> {
    state(&app).cancel.store(true, Ordering::SeqCst);
    Ok(())
}

#[tauri::command]
pub fn pull_progress(app: tauri::AppHandle) -> Result<compose::PullProgress> {
    Ok(state(&app)
        .pull
        .lock()
        .map(|g| g.clone())
        .unwrap_or_else(|_| compose::PullProgress::empty()))
}

// ── 安装：起容器 ──────────────────────────────────────────────────────────

/// `up -d` + 轮询健康。同样立刻返回，进度走 `hunter://start` 事件。
#[tauri::command]
pub fn start_stack(app: tauri::AppHandle) -> Result<()> {
    {
        let st = state(&app);
        if st.busy.swap(true, Ordering::SeqCst) {
            return Err(AppError::new(Code::Unknown, "已经有一次操作在进行中").to_string());
        }
    }
    let handle = app.clone();
    std::thread::spawn(move || {
        let st = handle.state::<AppState>();
        let h = handle.clone();
        let r = flow::start(&st, move |v| {
            let _ = h.emit(EV_START, v);
        });
        match &r {
            Ok(v) => {
                crate::linfo!("六个服务全部就绪");
                let _ = handle.emit(EV_START, v);
            }
            Err(e) => {
                crate::lerror!("启动失败：{}", e.msg);
                let _ = handle.emit(EV_LOG, e.to_string());
            }
        }
        st.busy.store(false, Ordering::SeqCst);
    });
    Ok(())
}

/// 启动过程中的健康快照。前端每秒拉一次（事件可能在页面挂载前就发过了）。
#[tauri::command]
pub async fn starting_status(app: tauri::AppHandle) -> Result<RuntimeStatus> {
    blocking(move || {
        let st = state(&app);
        let mut s = flow::runtime_status(&st);
        // 启动过程中额度接口的往返没必要，去掉免得每秒一次
        s.quota = None;
        if let Some(note) = st.start_note.lock().ok().and_then(|g| g.clone()) {
            s.log.push(note);
        }
        Ok(s)
    })
    .await
}

// ── 运行面板 ──────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn runtime_status(app: tauri::AppHandle) -> Result<RuntimeStatus> {
    blocking(move || Ok(flow::runtime_status(&state(&app)))).await
}

#[tauri::command]
pub async fn stack_action(action: String) -> Result<String> {
    blocking(move || {
        let r = match action.as_str() {
            "stop" => compose::stop().map(|_| "已停止".to_string()),
            "start" => compose::up().map(|_| "已启动".to_string()),
            "restart" => compose::restart().map(|_| "已重启".to_string()),
            // down 不带 -v：绝不删用户的数据卷
            "down" => compose::down().map(|_| "已移除容器（数据卷保留）".to_string()),
            other => Err(AppError::new(
                Code::Unknown,
                format!("不认识的操作：{other}"),
            )),
        }?;
        crate::linfo!("栈操作 {action}：{r}");
        Ok(r)
    })
    .await
}

#[tauri::command]
pub async fn compose_logs(service: Option<String>, tail: Option<usize>) -> Result<Vec<String>> {
    blocking(move || compose::logs(service.as_deref(), tail.unwrap_or(200))).await
}

#[tauri::command]
pub fn launcher_log(tail: usize) -> Result<Vec<String>> {
    Ok(crate::log::tail_file(tail))
}

// ── 设置 ──────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LauncherSettings {
    pub locale: String,
    pub autostart: bool,
    pub check_update: bool,
    pub registry: String,
    pub hunter_tag: String,
    pub work_dir: String,
    pub telemetry: bool,
    /// 上报端点。**默认空 = 暂未开启上报**，界面上要如实这么写
    #[serde(default)]
    pub telemetry_endpoint: String,
    /// gateway | own
    #[serde(default)]
    pub model_mode: String,
    #[serde(default)]
    pub model_base_url: String,
    #[serde(default)]
    pub model_name: String,
    /// 当前镜像源的完整前缀（自定义源时界面要显示它）
    #[serde(default)]
    pub registry_prefix: String,
    /// web 端口只允许本机访问。**默认 false**（= 绑所有网卡，与 I1 之前的行为一致）。
    /// 改这一项要重新生成覆盖文件并重启容器才生效，界面上写清楚了。
    #[serde(default)]
    pub web_local_only: bool,
}

#[tauri::command]
pub async fn read_settings(app: tauri::AppHandle) -> Result<LauncherSettings> {
    blocking(move || {
        let c = state(&app).config();
        Ok(to_settings(&c))
    })
    .await
}

#[tauri::command]
pub async fn write_settings(
    app: tauri::AppHandle,
    settings: LauncherSettings,
) -> Result<LauncherSettings> {
    blocking(move || {
        let st = state(&app);
        let mut c = st.config();
        c.launcher.locale = settings.locale;
        c.launcher.check_update_hours = if settings.check_update { 24 } else { 0 };
        c.hunter.web_bind = if settings.web_local_only {
            config::WEB_BIND_LOCAL.into()
        } else {
            config::WEB_BIND_ALL.into()
        };

        // 开机自启：**真去动系统**（Linux 的 .desktop / mac 的 LaunchAgent / Windows 注册表），
        // 然后把系统里的真实状态记回配置，而不是把用户点的那一下直接当成结果（红线 1）。
        if settings.autostart != crate::autostart::status() {
            match crate::autostart::set(settings.autostart) {
                Ok(m) => crate::linfo!("开机自启：{m}"),
                Err(e) => crate::lwarn!("开机自启设置失败：{}", e.msg),
            }
        }
        c.launcher.autostart = crate::autostart::status();

        // 遥测：**关掉的那一刻就把本地队列删干净**（方案 §11.2）。
        // 开启时补一个 install_id（随机 UUID，与 key 无关，方案 §12.1）。
        let was = c.telemetry.enabled;
        c.telemetry.enabled = settings.telemetry;
        if settings.telemetry && c.telemetry.install_id.is_empty() {
            c.telemetry.install_id = crate::telemetry::new_install_id();
        }
        if was && !settings.telemetry {
            match crate::telemetry::clear() {
                Ok(()) => crate::linfo!("遥测已关闭，本地队列已清空"),
                Err(e) => crate::lwarn!("清空遥测队列失败：{}", e.msg),
            }
        }

        if let Some(cand) = registry::by_id(&settings.registry) {
            c.apply_registry(cand);
        } else if !settings.registry.trim().is_empty() {
            // 手填的自定义源要先过形状校验 —— 它会原样进 .env 与覆盖文件（I2 自审）
            if !registry::prefix_ok(&settings.registry) {
                return Err(AppError::new(
                    Code::ConfigWrite,
                    format!(
                        "自定义镜像源「{}」的写法不对。要的是「主机[:端口]/路径」，\
                         例如 registry.example.com/agentpit 或 10.0.0.2:5000/hunter；\
                         不要带镜像名、tag、空格或换行。",
                        settings.registry.trim()
                    ),
                ));
            }
            let cand = registry::custom(&settings.registry);
            c.apply_registry(&cand);
        }
        c.save()?;
        st.set_config(c.clone());
        Ok(to_settings(&c))
    })
    .await
}

fn to_settings(c: &LauncherConfig) -> LauncherSettings {
    LauncherSettings {
        locale: c.launcher.locale.clone(),
        // 读的是**系统里真实的自启项**，不是配置文件里记的
        autostart: crate::autostart::status(),
        check_update: c.launcher.check_update_hours > 0,
        registry: c.hunter.registry_id.clone(),
        hunter_tag: c.hunter.tag.clone(),
        work_dir: crate::paths::root().to_string_lossy().into_owned(),
        telemetry: c.telemetry.enabled,
        telemetry_endpoint: c.telemetry.endpoint.clone(),
        model_mode: c.model.mode.clone(),
        model_base_url: c.model.base_url.clone(),
        model_name: c.model.model.clone(),
        registry_prefix: c.hunter.registry_prefix.clone(),
        web_local_only: c.hunter.web_local_only(),
    }
}

// ── 诊断包（反馈页用） ────────────────────────────────────────────────────

/// 导出一份**脱敏后**的诊断文本。红线 2：里面不能有 key。
/// 这里不做任何上报 —— 只把文本交给用户，由他自己决定贴到哪。
#[tauri::command]
pub async fn diagnostics(app: tauri::AppHandle) -> Result<String> {
    blocking(move || {
        let st = state(&app);
        let cfg = st.config();
        let d = docker::detect();
        let ps = compose::ps().unwrap_or_default();
        let mut s = String::new();
        s.push_str(&format!(
            "# Hunter 启动器诊断 · {}\n\n",
            crate::timefmt::now_shanghai()
        ));
        s.push_str(&format!(
            "启动器 {} · {} {}\n",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH
        ));
        s.push_str(&format!(
            "Docker {:?} · server {:?} · compose {:?}\n",
            d.runtime, d.server_version, d.compose_version
        ));
        s.push_str(&format!(
            "Hunter tag {} · 镜像源 {}\n",
            cfg.hunter.tag, cfg.hunter.registry_prefix
        ));
        s.push_str(&format!(
            "端口 web {} · api {} · opencode {} · postgres {} · redis {}\n",
            cfg.hunter.ports.web,
            cfg.hunter.ports.api,
            cfg.hunter.ports.opencode,
            cfg.hunter.ports.postgres,
            cfg.hunter.ports.redis
        ));
        s.push_str(&format!(
            "模型模式 {} · 模型 {}\n\n",
            cfg.model.mode,
            if cfg.model.model.is_empty() {
                "—"
            } else {
                &cfg.model.model
            }
        ));
        s.push_str("## docker compose ps\n");
        for p in &ps {
            s.push_str(&format!(
                "{} · {} · {} · 端口 {:?}\n",
                p.service,
                p.state,
                compose::health_cn(p.health),
                p.port
            ));
        }
        s.push_str("\n## 启动器日志（最近 80 行）\n");
        for l in crate::log::tail_file(80) {
            s.push_str(&l);
            s.push('\n');
        }
        s.push_str("\n## 容器日志（最近 60 行）\n");
        for l in compose::logs(None, 60).unwrap_or_default() {
            s.push_str(&l);
            s.push('\n');
        }
        // 最后再整体过一遍脱敏 —— 上面每一段其实都已经过了，这里是第二道保险
        Ok(crate::redact::redact(&s))
    })
    .await
}

// ── 遥测（本地队列，默认不上报） ──────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TelemetryView {
    pub enabled: bool,
    /// 空 = 暂未开启上报。界面按这个如实写文案（红线 1）
    pub endpoint: String,
    pub queue_path: String,
    pub lines: Vec<String>,
    /// 会收集哪些事件 —— 设置页要能一眼看全（方案 §12.1「透明」）
    pub events: Vec<String>,
}

/// 「查看本机将要发送的数据」。返回的就是 `queue.jsonl` 的原文，一个字都不加工。
#[tauri::command]
pub async fn telemetry_view(app: tauri::AppHandle) -> Result<TelemetryView> {
    blocking(move || {
        let c = state(&app).config();
        Ok(TelemetryView {
            enabled: c.telemetry.enabled,
            endpoint: c.telemetry.endpoint.clone(),
            queue_path: crate::paths::telemetry_queue()
                .to_string_lossy()
                .into_owned(),
            lines: crate::telemetry::queue_lines(),
            events: crate::telemetry::EVENTS
                .iter()
                .map(|e| e.to_string())
                .collect(),
        })
    })
    .await
}

#[tauri::command]
pub async fn telemetry_clear() -> Result<String> {
    blocking(|| {
        crate::telemetry::clear()?;
        Ok("本地队列已清空".to_string())
    })
    .await
}

// ── 开机自启 ──────────────────────────────────────────────────────────────

/// 切开机自启，返回**系统里真实的**状态（不是用户点的那个值）。
#[tauri::command]
pub async fn set_autostart(app: tauri::AppHandle, on: bool) -> Result<bool> {
    blocking(move || {
        let msg = crate::autostart::set(on)?;
        crate::linfo!("开机自启：{msg}");
        let real = crate::autostart::status();
        let st = state(&app);
        let mut c = st.config();
        c.launcher.autostart = real;
        c.save()?;
        st.set_config(c);
        Ok(real)
    })
    .await
}

// ── 模型模式切换（设置页） ────────────────────────────────────────────────

/// 切换网关 ↔ 自带 key。先做连通性检查（`set_model` 那一套），
/// 通过之后**重写 `.env` 并重建 api / opencode / llm-shim**（里程碑 M3 第 3 项）。
#[tauri::command]
pub async fn switch_model(app: tauri::AppHandle, choice: ModelChoice) -> Result<String> {
    blocking(move || {
        let st = state(&app);
        let mut cfg = st.config();
        if choice.mode == "own" {
            let r = gateway::check_own_key(
                &choice.base_url,
                &choice.model,
                &choice.api_key,
                Duration::from_secs(30),
            );
            crate::linfo!("切自带 key：连通性 ok={} via={:?}", r.ok, r.via);
            if !r.ok {
                return Err(AppError::new(Code::KeyInvalid, r.message));
            }
            cfg.model.mode = "own".into();
            cfg.model.base_url = choice.base_url.trim_end_matches('/').to_string();
            cfg.model.model = choice.model.clone();
            cfg.model.schema_sanitize = r.schema_sanitize;
            st.set_own_key(&choice.api_key);
        } else {
            cfg.model.mode = "gateway".into();
            cfg.model.base_url = gateway::LLM_BASE_URL.into();
            cfg.model.model = gateway::DEFAULT_MODEL.into();
            cfg.model.schema_sanitize = false;
        }
        cfg.save()?;
        st.set_config(cfg);
        let msg = flow::apply_model_change(&st)?;
        crate::linfo!("模型模式切换完成：{msg}");
        Ok(msg)
    })
    .await
}

// ── 反馈与诊断包 ──────────────────────────────────────────────────────────

/// 收一份诊断，**逐节**交给界面预览。用户可以逐节勾掉不带（方案 §11.1）。
#[tauri::command]
pub async fn diagnostics_sections(app: tauri::AppHandle) -> Result<Vec<crate::feedback::Section>> {
    blocking(move || {
        let c = state(&app).config();
        Ok(crate::feedback::collect(&c, env!("CARGO_PKG_VERSION")))
    })
    .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    pub path: String,
    pub bytes: usize,
}

/// 导出 zip 到 `~/.hunter/diagnostics/`。**不上传任何东西**。
#[tauri::command]
pub async fn export_diagnostics(
    app: tauri::AppHandle,
    include: Vec<String>,
    form: crate::feedback::Form,
) -> Result<ExportResult> {
    blocking(move || {
        let c = state(&app).config();
        let sections = crate::feedback::collect(&c, env!("CARGO_PKG_VERSION"));
        let (path, bytes) =
            crate::feedback::export_zip(&sections, &include, &form, env!("CARGO_PKG_VERSION"))?;
        Ok(ExportResult { path, bytes })
    })
    .await
}

/// 拼一个预填好的 GitHub issue 链接。**诊断包不自动上传**，正文里只放概要那一节。
#[tauri::command]
pub async fn feedback_issue_url(
    app: tauri::AppHandle,
    form: crate::feedback::Form,
) -> Result<String> {
    blocking(move || {
        let c = state(&app).config();
        let sections = crate::feedback::collect(&c, env!("CARGO_PKG_VERSION"));
        let summary = sections
            .iter()
            .find(|s| s.id == "summary")
            .map(|s| s.body.clone())
            .unwrap_or_default();
        Ok(crate::feedback::issue_url(&form, &summary))
    })
    .await
}

// ── 日志导出 ──────────────────────────────────────────────────────────────

/// 把当前这一档日志**脱敏后**导出成一个文本文件。返回落地路径。
#[tauri::command]
pub async fn export_logs(
    source: String,
    service: Option<String>,
    tail: usize,
) -> Result<ExportResult> {
    blocking(move || {
        let lines = if source == "launcher" {
            crate::log::tail_file(tail)
        } else {
            compose::logs(service.as_deref(), tail)?
        };
        let mut body = format!(
            "# Hunter 启动器日志导出 · {}\n# 来源：{}{}\n# 已按技术方案 §12.3 脱敏\n\n",
            crate::timefmt::now_shanghai(),
            source,
            service
                .as_deref()
                .map(|s| format!(" / {s}"))
                .unwrap_or_default()
        );
        body.push_str(&lines.join("\n"));
        body.push('\n');
        // 出口再过一道（容器日志里还可能有对话内容）
        let body = crate::redact::redact(&crate::redact::mask_content_fields(&body));
        if let Some(bad) = crate::feedback::assert_clean(&body) {
            return Err(AppError::new(
                Code::Unknown,
                format!("日志自查没通过，已取消导出：{bad}"),
            ));
        }
        crate::paths::ensure_dirs()?;
        let name = format!(
            "hunter-logs-{}-{}.txt",
            source,
            crate::timefmt::now_shanghai()
                .replace(['-', ':'], "")
                .replace(' ', "-")
        );
        let path = crate::paths::diagnostics_dir().join(&name);
        std::fs::write(&path, body.as_bytes()).map_err(|e| {
            AppError::new(
                Code::ConfigWrite,
                format!("写 {} 失败：{e}", path.display()),
            )
        })?;
        crate::paths::chmod_600(&path)?;
        crate::linfo!("日志已导出：{}", path.display());
        Ok(ExportResult {
            path: path.to_string_lossy().into_owned(),
            bytes: body.len(),
        })
    })
    .await
}

// ── 托盘与退出 ────────────────────────────────────────────────────────────

/// 前端（或自动化测试）触发一次托盘菜单动作。走的是和真点菜单**完全相同**的分支。
#[tauri::command]
pub fn tray_invoke(app: tauri::AppHandle, id: String) -> Result<()> {
    std::thread::spawn(move || crate::tray::run_action(&app, &id));
    Ok(())
}

/// 退出。`stop_containers = true` 时先把容器停掉再退（方案 §5.8 的两条分支）。
#[tauri::command]
pub fn quit_app(app: tauri::AppHandle, stop_containers: bool) {
    std::thread::spawn(move || {
        if stop_containers {
            crate::linfo!("退出：用户选了「一起停止」");
            match compose::stop() {
                Ok(()) => crate::linfo!("容器已停止，准备退出"),
                Err(e) => crate::lerror!("退出前停容器失败：{}", e.msg),
            }
        } else {
            crate::linfo!("退出：用户选了「保持后台运行」，容器不动");
        }
        app.exit(0);
    });
}

/// 「上游缺哪些接口」的清单。界面上把它摆出来，和成果文档是同一份。
#[tauri::command]
pub fn missing_endpoints() -> Vec<flow::MissingEndpoint> {
    flow::missing_endpoints()
}

// ── M4 · 启动器自更新 ─────────────────────────────────────────────────────

/// 查启动器自己有没有新版本（方案 §10）。**不下载任何东西。**
#[tauri::command]
pub async fn check_launcher_update(
    app: tauri::AppHandle,
) -> Result<crate::selfupdate::LauncherUpdate> {
    let u = crate::selfupdate::check(&app).await;
    crate::tray::note_launcher_update(&app, u.version.as_deref());
    Ok(u)
}

/// 装启动器的新版本。
///
/// * 能就地装（AppImage / Windows / macOS）→ 装完**自动重启进程**，这个调用不会返回。
/// * 装不了（`.deb`）→ 把包下到 `~/.hunter/updates/` 并返回一条要用户自己敲的命令。
#[tauri::command]
pub async fn install_launcher_update(
    app: tauri::AppHandle,
) -> Result<Option<crate::selfupdate::ManualInstall>> {
    match crate::selfupdate::install(&app).await {
        Ok(Some(m)) => Ok(Some(m)),
        Ok(None) => {
            crate::linfo!("自更新完成，重启启动器");
            // 重启前把容器留着 —— 用户更新的是启动器，不是 Hunter
            app.restart();
        }
        Err(e) => Err(e.to_string()),
    }
}

// ── M4 · Hunter 版本检查与升级 ────────────────────────────────────────────

/// Hunter 有没有新版本 + Release Notes 摘要（方案 §10 第 1 条）。
/// `force` 为真时绕过 6 小时缓存（用户亲手点「检查更新」）。
#[tauri::command]
pub async fn check_hunter_update(
    app: tauri::AppHandle,
    force: Option<bool>,
) -> Result<crate::upgrade::UpgradeCheck> {
    blocking(move || {
        let cur = state(&app).config().hunter.tag;
        Ok(crate::upgrade::check(&cur, force.unwrap_or(false)))
    })
    .await
}

/// 升级过程中的实时状态。前端每秒拉一次。
#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeStatus {
    /// 还在进行中
    pub running: bool,
    /// 每一步的文字，最新的在最后
    pub steps: Vec<String>,
    /// 做完了的结果；还没做完就是 `None`
    pub result: Option<crate::upgrade::UpgradeResult>,
    /// 失败时的 `E_UPDATE_FAILED: …`
    pub error: Option<String>,
}

fn upgrade_slot() -> &'static std::sync::Mutex<UpgradeStatus> {
    static ONCE: std::sync::OnceLock<std::sync::Mutex<UpgradeStatus>> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| std::sync::Mutex::new(UpgradeStatus::default()))
}

/// 升级事件：每一步的文字。前端也可以只靠 [`upgrade_status`] 轮询。
pub const EV_UPGRADE: &str = "hunter://upgrade";

/// 开始升级。立刻返回，进度走 `hunter://upgrade`（步骤文字）与 `hunter://pull`（拉取进度）。
#[tauri::command]
pub fn upgrade_hunter(app: tauri::AppHandle, tag: String) -> Result<()> {
    {
        let st = state(&app);
        if st.busy.swap(true, Ordering::SeqCst) {
            return Err(AppError::new(Code::Unknown, "已经有一次操作在进行中").to_string());
        }
        st.cancel.store(false, Ordering::SeqCst);
    }
    if let Ok(mut g) = upgrade_slot().lock() {
        *g = UpgradeStatus {
            running: true,
            ..Default::default()
        };
    }
    let handle = app.clone();
    std::thread::spawn(move || {
        let st = handle.state::<AppState>();
        let h_step = handle.clone();
        let note = move |line: &str| {
            let line = crate::redact::redact(line);
            crate::linfo!("升级：{line}");
            if let Ok(mut g) = upgrade_slot().lock() {
                g.steps.push(line.clone());
            }
            let _ = h_step.emit(EV_UPGRADE, line);
        };
        let h_pull = handle.clone();
        let r = crate::upgrade::upgrade(&st, &tag, note, move |p| {
            let _ = h_pull.emit(EV_PULL, p);
        });
        if let Ok(mut g) = upgrade_slot().lock() {
            g.running = false;
            match r {
                Ok(res) => g.result = Some(res),
                Err(e) => {
                    crate::lerror!("升级失败：{}", e.msg);
                    g.error = Some(e.to_string());
                }
            }
        }
        st.busy.store(false, Ordering::SeqCst);
        crate::tray::refresh_status(&handle);
    });
    Ok(())
}

#[tauri::command]
pub fn upgrade_status() -> Result<UpgradeStatus> {
    Ok(upgrade_slot().lock().map(|g| g.clone()).unwrap_or_default())
}

/// 备份清单（设置页显示）。
#[tauri::command]
pub async fn list_backups() -> Result<Vec<crate::backup::BackupMeta>> {
    blocking(|| Ok(crate::backup::list())).await
}

/// 手动做一次备份（不升级也能备份 —— 换机器、动配置前都用得上）。
#[tauri::command]
pub async fn create_backup(app: tauri::AppHandle) -> Result<crate::backup::BackupMeta> {
    blocking(move || {
        let tag = state(&app).config().hunter.tag;
        let m = crate::backup::create(&tag, |_| {})?;
        let _ = crate::backup::write_readme(&m.id);
        Ok(m)
    })
    .await
}

/// 从某次备份把数据库灌回去。**只在用户显式要求时做**（见 [`crate::backup`] 的模块头）。
#[tauri::command]
pub async fn restore_backup(id: String) -> Result<String> {
    blocking(move || crate::backup::restore_db(&id)).await
}

// ── M4 · 离线包 ───────────────────────────────────────────────────────────

/// 弹系统文件选择框挑一个 `.tar`。挑完只返回路径，**不做任何导入**。
///
/// 对话框由 Rust 弹（不是前端），所以不用给前端开 `dialog:` 权限：
/// 前端能做的事仍然只有「调这个 command」。
#[tauri::command]
pub async fn pick_offline_tar(app: tauri::AppHandle) -> Result<Option<String>> {
    blocking(move || {
        use tauri_plugin_dialog::DialogExt;
        let picked = app
            .dialog()
            .file()
            .set_title("选择离线镜像包（docker save 出来的 .tar）")
            .add_filter("镜像包", &["tar"])
            .blocking_pick_file();
        Ok(picked.map(|p| p.to_string()))
    })
    .await
}

/// 导入离线包（方案 §9）。导入成功且六个镜像齐了，就把镜像源与版本改成包里的那一套，
/// 安装流程会因此跳过拉取。
#[tauri::command]
pub async fn import_offline(
    app: tauri::AppHandle,
    path: String,
) -> Result<crate::offline::ImportResult> {
    blocking(move || {
        let h = app.clone();
        let r = crate::offline::import(std::path::Path::new(&path), move |line| {
            let _ = h.emit(EV_LOG, line.to_string());
        })?;
        if r.complete {
            // 把配置对齐到包里的那一套，否则 compose 还是会去拉网上的镜像
            let st = state(&app);
            let mut cfg = st.config();
            if let Some(p) = &r.registry_prefix {
                // 包里的前缀正好是我们认得的两个候选源之一就用它（界面上能显示「腾讯云 · 香港」
                // 这种人话）；不是的话当成自定义源，如实显示前缀本身
                let owned;
                let cand: &registry::Candidate =
                    match registry::CANDIDATES.iter().find(|c| c.prefix == p.as_str()) {
                        Some(c) => c,
                        None => {
                            owned = registry::custom(p);
                            &owned
                        }
                    };
                cfg.apply_registry(cand);
                // postgres / redis 的前缀按包里实际的来
                if let Some(b) = &r.base_prefix {
                    cfg.hunter.base_prefix = b.clone();
                }
            }
            if let Some(t) = &r.tag {
                cfg.hunter.tag = t.clone();
            }
            // **落盘**，不只放内存：界面上导入之后可能要过一会儿才点「开始安装」，
            // 中间用户完全可能把启动器关了再开（headless 更是两个进程）
            cfg.install.offline = true;
            cfg.save()?;
            st.set_config(cfg);
            st.offline.store(true, Ordering::SeqCst);
            crate::linfo!(
                "离线包导入完成，已把镜像源改成 {:?}、版本改成 {:?}，安装时会跳过拉取",
                r.registry_prefix,
                r.tag
            );
        }
        Ok(r)
    })
    .await
}

/// 六个镜像在本机齐了没有。拉取页靠它决定要不要显示「已导入，跳过拉取」。
#[tauri::command]
pub async fn offline_ready(app: tauri::AppHandle) -> Result<crate::offline::ImportResult> {
    blocking(move || {
        let cfg = state(&app).config();
        let specs = config::images(
            &cfg.hunter.registry_prefix,
            &cfg.hunter.base_prefix,
            &cfg.hunter.tag,
        );
        let (ok, missing) = crate::offline::all_present(&specs);
        let matched = specs
            .iter()
            .filter(|s| !missing.contains(&s.service))
            .map(|s| crate::offline::MatchedImage {
                service: s.service.clone(),
                reference: s.reference.clone(),
                bytes: crate::offline::inspect_size(&s.reference),
            })
            .collect();
        Ok(crate::offline::ImportResult {
            path: String::new(),
            bytes: 0,
            seconds: 0,
            loaded: Vec::new(),
            matched,
            missing,
            registry_prefix: Some(cfg.hunter.registry_prefix.clone()),
            base_prefix: Some(cfg.hunter.base_prefix.clone()),
            tag: Some(cfg.hunter.tag.clone()),
            complete: ok,
        })
    })
    .await
}

// ── 工具 ──────────────────────────────────────────────────────────────────

/// 把阻塞活儿挪到工作线程，别卡住 UI。
async fn blocking<T, F>(f: F) -> Result<T>
where
    F: FnOnce() -> crate::err::AppResult<T> + Send + 'static,
    T: Send + 'static,
{
    match tauri::async_runtime::spawn_blocking(f).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(e.to_string()),
        Err(e) => Err(format!("E_UNKNOWN: 后台任务崩了：{e}")),
    }
}

fn main_window(app: &tauri::AppHandle) -> Result<tauri::WebviewWindow> {
    app.get_webview_window("main")
        .ok_or_else(|| "E_UNKNOWN: 找不到主窗口".to_string())
}

#[cfg(test)]
mod tests {
    use super::url_allowed;

    #[test]
    fn https_一律放行() {
        assert!(url_allowed("https://github.com/agentpit-io/HunterLauncher"));
        assert!(url_allowed("https://hunter.agentpit.io/dev/api-keys"));
    }

    #[test]
    fn 本机_http_放行() {
        assert!(url_allowed("http://localhost:3101"));
        assert!(url_allowed("http://127.0.0.1:8101/api/health"));
        assert!(url_allowed("http://localhost"));
        assert!(url_allowed("http://127.0.0.1/"));
    }

    #[test]
    fn 假冒本机的域名不能放行() {
        // 只比前缀的话这四个都会被当成本机（I1 自审发现的实际缺口）
        assert!(!url_allowed("http://localhost.example.com/x"));
        assert!(!url_allowed("http://localhost-evil.test"));
        assert!(!url_allowed("http://127.0.0.1.example.com/"));
        assert!(!url_allowed("http://127.0.0.10:80"));
    }

    #[test]
    fn 把本机写成_userinfo_的不能放行() {
        // I2 自审发现：`localhost:1` 在 URL 语法里是 userinfo，真正的主机是 @ 后面那个。
        // I1 修完的版本（只判「主机名后紧跟 : 或 /」）会把前两条放行。
        assert!(!url_allowed("http://localhost:1@evil.com/"));
        assert!(!url_allowed("http://localhost:3101@evil.com"));
        assert!(!url_allowed("http://127.0.0.1@evil.com/x"));
        assert!(!url_allowed("http://user@localhost:3101/"));
    }

    #[test]
    fn 端口必须是纯数字() {
        assert!(url_allowed("http://localhost:3101/x?y=1"));
        assert!(!url_allowed("http://localhost:/x"));
        assert!(!url_allowed("http://localhost:80a/x"));
        assert!(!url_allowed("http://localhost:3101x"));
    }

    #[test]
    fn 非_http_协议一律拒绝() {
        assert!(!url_allowed("file:///etc/passwd"));
        assert!(!url_allowed("http://example.com"));
        assert!(!url_allowed("javascript:alert(1)"));
        assert!(!url_allowed("/usr/bin/xcalc"));
        assert!(!url_allowed(""));
    }
}
