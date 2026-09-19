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
    let allowed = url.starts_with("https://")
        || url.starts_with("http://localhost")
        || url.starts_with("http://127.0.0.1");
    if !allowed {
        return Err(format!("E_UNKNOWN: 拒绝打开非 https / 非本机地址：{url}"));
    }
    app.opener()
        .open_url(url, None::<&str>)
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
pub async fn detect_docker() -> Result<docker::DockerInfo> {
    blocking(|| {
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
    Ok(crate::log::tail(tail))
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
        c.launcher.autostart = settings.autostart;
        c.launcher.check_update_hours = if settings.check_update { 24 } else { 0 };
        c.telemetry.enabled = settings.telemetry;
        if let Some(cand) = registry::by_id(&settings.registry) {
            c.apply_registry(cand);
        } else if !settings.registry.is_empty() && settings.registry.contains('/') {
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
        autostart: c.launcher.autostart,
        check_update: c.launcher.check_update_hours > 0,
        registry: c.hunter.registry_id.clone(),
        hunter_tag: c.hunter.tag.clone(),
        work_dir: crate::paths::root().to_string_lossy().into_owned(),
        telemetry: c.telemetry.enabled,
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
        for l in crate::log::tail(80) {
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
