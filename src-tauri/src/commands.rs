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

/// **把窗口收起来回到托盘**（I11 · U3）。
///
/// 和 [`window_close`] 不是一回事：那个是「关窗口 = 请求退出」，
/// 容器还在跑时会弹「保持后台运行 / 一起停止」。这个只是把界面藏起来 ——
/// 进程不退、托盘还在、正在跑的服务一个都不动。
///
/// 错误页上那个「关闭」按钮走的就是它：用户已经确认没有异常了，
/// 他要的是「别再烦我」，不是「把 Hunter 停掉」。
#[tauri::command]
pub fn window_hide(app: tauri::AppHandle) -> Result<()> {
    main_window(&app)?
        .hide()
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

/// 打开启动器时该进哪一页（I12 · R1 的五行表）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BootRoute {
    /// 全新安装 —— 欢迎页
    Welcome,
    /// 直接进运行面板（健康 / 已停止 / 部分不正常都走这里）
    Dashboard,
    /// 本项目不在了，但上一次的数据卷还在 —— 先给用户看一眼（R5）
    DataFound,
}

/// 启动时问一次：装过没有、现在什么样、该进哪一页。
///
/// # 从「只看标记」改成「标记 + 现状」（I12 · R1）
///
/// 0.1.11 及之前这里只看 `install.done`：
///
/// ```text
/// installed = cfg.install.done && .env 存在
/// ```
///
/// 2026-09-22 23:10 用户 Mac 上的现场把这个判据打穿了 —— 六个容器 6/6 健康跑着、
/// web 127.0.0.1:3100 返回 200，而 `launcher.toml` 里 `done = false`：
/// 那次安装在启动器看来是失败的（`E_START_TIMEOUT`），最后是在**外部**修好的，
/// 没有人回来把标记补上。于是重开启动器，用户看到的是「欢迎使用 Hunter 启动器」。
///
/// 现在的判据是**现状优先**，标记只在现状看不出什么时才起作用：
///
/// | 现状（[`crate::selfcheck::review`]，只读） | 进哪 | 顺手做什么 |
/// |---|---|---|
/// | 6/6 健康 | 运行面板 | `done=false` 就**补写**，并记 `adopted_from_running=true` |
/// | 容器都在但停着 / 有的不正常 | 运行面板 | 同上（装过是事实，只是没跑好） |
/// | 只装了一半 | 运行面板 | 面板上会显示缺哪几个；**不回向导** |
/// | 本项目没有容器，但数据卷还在 | 「检测到上次的数据」 | — |
/// | 什么都没有 | 欢迎页 | — |
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BootState {
    /// 还留着给老前端用：`route != Welcome` 即为真
    pub installed: bool,
    pub running: bool,
    pub locale: String,
    pub hunter_tag: String,
    pub work_dir: String,
    /// 该进哪一页
    pub route: BootRoute,
    /// 复查的结论（界面上「现在是什么情况」直接用它）
    pub posture: crate::selfcheck::Posture,
    /// 一句人话，来自复查的实测结果
    pub headline: String,
    /// 这一次有没有补写安装标记（补写了就在过程流 / 面板上说一句）
    pub adopted: bool,
    /// 配置里记着的那几项（面板上「上次正常运行」那一行）
    pub launcher_version: String,
    pub installed_at: String,
    pub last_healthy_at: String,
    /// 本项目名下还剩几个数据卷（`route == DataFound` 时界面要说）
    pub volume_count: usize,
    /// 这一次判定花了多久（毫秒）。界面上那句「正在检查 Hunter 状态」用它对账
    pub elapsed_ms: u64,
}

/// R1 那张五行表，**做成纯函数**。
///
/// 拆出来的理由和 `runtime::effective::decide` 一样：判定本身不该需要一台
/// 处在那个状态的机器才能测。开发机与测试机上 Hunter 一直好好跑着，
/// 「什么都没有」「只剩数据卷」这两行在真机上根本造不出来 —— 而它们正是
/// 2026-09-22 那次现场最需要钉死的两行。
pub fn route_of(posture: crate::selfcheck::Posture, volumes: usize) -> BootRoute {
    use crate::selfcheck::Posture;
    match posture {
        // 六个容器都在（跑着、停着、有的不正常都算）→ 这台机器上装过，事实清楚。
        // **不管 install.done 写的是什么** —— 那正是 0.1.9 那次判错的地方
        Posture::Healthy | Posture::Partial => BootRoute::Dashboard,
        // 只装了一半：容器有一部分。**也算装过** ——
        // 回向导只会让用户再装一遍，面板上说清缺哪几个才是对的
        Posture::Incomplete => BootRoute::Dashboard,
        // 一个容器都没有。这时候才轮到「上一次的数据卷还在吗」
        Posture::Absent if volumes > 0 => BootRoute::DataFound,
        Posture::Absent => BootRoute::Welcome,
    }
}

#[tauri::command]
pub async fn boot_state(app: tauri::AppHandle) -> Result<BootState> {
    blocking(move || {
        let t0 = std::time::Instant::now();
        let st = state(&app);
        let mut cfg = st.config();

        // 每次启动都把「现在跑的是哪一版启动器」记下来（R1 第 3 条）。
        // 老配置里的 `[launcher] version` 在 `LauncherConfig::load` 里已经迁移过来了
        let mut dirty = cfg.stamp_launcher_version();

        // 只读地问一句现在到底怎么样。浅查（不起一次性容器）—— 这是开机第一屏，
        // 方案第三节要求「不超过 1 秒」，深查动辄十几秒
        let rv = crate::selfcheck::review(false);

        // 没有容器时才需要问一句「数据卷还在吗」（那一步要起一个 docker 子进程）
        let volumes = if rv.posture == crate::selfcheck::Posture::Absent {
            crate::compose::volumes_of_project()
                .map(|v| v.len())
                .unwrap_or(0)
        } else {
            0
        };
        let route = route_of(rv.posture, volumes);
        let adopted = route == BootRoute::Dashboard && !cfg.install.done;
        if route == BootRoute::Dashboard {
            cfg.mark_installed(adopted);
            dirty = true;
            // 「上一次好好的是什么时候」—— 只有真的 6/6 健康才刷，
            // 不健康的时候刷它就是在记一个假时间（红线 1）
            if rv.posture == crate::selfcheck::Posture::Healthy {
                cfg.touch_healthy();
            }
        }
        let volume_count = if route == BootRoute::DataFound {
            volumes
        } else {
            0
        };

        cfg.save_if(dirty);
        st.set_config(cfg.clone());

        if adopted {
            crate::linfo!(
                "开机判定：{}，而配置里 install.done 还是 false —— 已补写为已安装（adopted_from_running=true）",
                rv.headline
            );
        } else {
            crate::linfo!("开机判定：{}（{} 毫秒）", rv.headline, rv.elapsed_ms);
        }

        Ok(BootState {
            installed: route != BootRoute::Welcome,
            running: rv.posture == crate::selfcheck::Posture::Healthy
                || rv.services.iter().any(|s| s.state == "running"),
            locale: cfg.launcher.locale.clone(),
            hunter_tag: cfg.hunter.tag.clone(),
            work_dir: crate::paths::root().to_string_lossy().into_owned(),
            route,
            posture: rv.posture,
            headline: rv.headline.clone(),
            adopted,
            launcher_version: cfg.install.launcher_version.clone(),
            installed_at: cfg.install.at.clone(),
            last_healthy_at: cfg.install.last_healthy_at.clone(),
            volume_count,
            elapsed_ms: t0.elapsed().as_millis() as u64,
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

/// 「上次保留下来的那把 key」的现状（I14 · F3）。
#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct KeptKey {
    /// `~/.hunter/app/.env` 里有一把格式对得上的 key
    pub present: bool,
    /// 打码后的样子（红线 2：完整的 key 不出这个进程）
    pub masked: String,
    /// 它在哪个文件里（路径里的家目录已打码）
    pub path: String,
    /// 拿去问过网关的结果。`present` 为假时是 `None`
    pub check: Option<gateway::KeyCheckResult>,
}

/// 安装向导的 key 页先问一句：上次有没有留下一把能用的 key。
///
/// 「只删除应用，保留数据」那一档保留了 `.env`，界面上承诺过
/// 「重新安装会直接沿用」—— 这个命令就是那句承诺的实现。
/// 验得过就**直接放进内存**，用户一个字都不用重新输；
/// 验不过就如实把原因交给界面，照常让他填一把新的。
///
/// **完整的 key 不返回给前端**：只给打码后的样子与校验结论。
#[tauri::command]
pub async fn kept_key(app: tauri::AppHandle) -> Result<KeptKey> {
    blocking(move || {
        let Some(k) = crate::config::kept_hunter_key() else {
            return Ok(KeptKey::default());
        };
        let r = gateway::check_key(&k, Duration::from_secs(20));
        crate::linfo!("上次保留的 key：valid={} reason={:?}", r.valid, r.reason);
        if r.valid {
            state(&app).set_hunter_key(&k);
        }
        Ok(KeptKey {
            present: true,
            masked: crate::redact::mask_key(&k),
            path: crate::redact::mask_home(&crate::paths::env_file().to_string_lossy()),
            check: Some(r),
        })
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
                // 带上真实错误码：安装阶段失败的原因不止「拉不下来」一种
                g.error_code = Some(e.code.as_str().to_string());
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

// ── 资源监控（I12 · R2）──────────────────────────────────────────────────

/// 这台电脑。5 秒一次（窗口看得见的时候）。
#[tauri::command]
pub async fn monitor_host() -> Result<crate::monitor::HostMetrics> {
    blocking(move || Ok(crate::monitor::host())).await
}

/// Hunter 运行环境（内置运行时的虚拟机）。30 秒一次。
#[tauri::command]
pub async fn monitor_runtime() -> Result<crate::monitor::RuntimeMetrics> {
    blocking(move || Ok(crate::monitor::runtime())).await
}

/// Hunter 各服务。10 秒一次。
#[tauri::command]
pub async fn monitor_services() -> Result<crate::monitor::ServiceMetrics> {
    blocking(move || Ok(crate::monitor::services())).await
}

/// 数据卷与镜像。5 分钟一次（`docker system df -v` 要遍历所有卷，慢）。
#[tauri::command]
pub async fn monitor_storage() -> Result<crate::monitor::StorageMetrics> {
    blocking(move || Ok(crate::monitor::storage())).await
}

// ── 重装前检测已有数据（I12 · R5）───────────────────────────────────────

/// 浅查（不起任何容器，**一个字节都不写**）。开机判定与「检测到上次的数据」那一页用它。
#[tauri::command]
pub async fn data_check() -> Result<crate::datacheck::DataCheck> {
    blocking(move || Ok(crate::datacheck::probe())).await
}

/// 深查（会起一个 `--rm` 的一次性 postgres 问表数与迁移版本）。
/// 用户在那一页上点「看看里面有什么」时才做 —— 十几秒，不能放在开机第一屏。
#[tauri::command]
pub async fn data_check_deep() -> Result<crate::datacheck::DataCheck> {
    blocking(move || Ok(crate::datacheck::probe_deep())).await
}

// ── 停止 / 启动 / 重启（I12 · R3）────────────────────────────────────────

/// 操作过程中的一步。界面上按顺序显示，**每一条都是刚刚真的做过的事**。
pub const EV_STACK: &str = "hunter://stack";

/// 界面在按下按钮**之前**要知道的事：这台机器上要不要问「顺便停运行环境吗」。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StackPlan {
    /// 这台机器跑的是内置运行时，而且它现在在跑 —— 停止那一步才需要多问一句
    pub builtin_running: bool,
    /// 停掉它大约释放多少内存（GB）。**来自我们自己启动虚拟机时定的参数**，不是估的
    pub builtin_mem_gb: u32,
    /// 六个服务现在各是什么状态（单服务重启的下拉框用它）
    pub services: Vec<String>,
}

#[tauri::command]
pub async fn stack_plan() -> Result<StackPlan> {
    blocking(move || {
        let builtin_running = crate::runtime::builtin::is_running()
            && crate::runtime::effective::builtin_is_effective();
        let (_, mem, _) = crate::runtime::builtin::vm_params();
        let services = compose::ps()
            .unwrap_or_default()
            .into_iter()
            .map(|s| s.service)
            .collect();
        Ok(StackPlan {
            builtin_running,
            builtin_mem_gb: mem,
            services,
        })
    })
    .await
}

/// 一次停止 / 启动 / 重启的结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StackOpResult {
    /// 一句人话的结论
    pub headline: String,
    /// 逐条做过的事（和事件流里推过去的是同一批）
    pub steps: Vec<String>,
    /// 做完之后六个服务里有几个就绪
    pub ready: usize,
    pub total: usize,
    pub elapsed_ms: u64,
}

/// 停止 / 启动 / 重启（I12 · R3）。
///
/// 和老的 [`stack_action`] 的区别是三件事：
///
/// 1. **停止时可以顺带停掉内置运行时的虚拟机** —— 不停的话那 4 GB 内存还占着，
///    用户点了「停止」却发现内存没降下来，只会以为启动器在骗人；
/// 2. **启动时运行环境没起就先起它**（含 I10 的 DNS 检查）——
///    0.1.9 之前这一步会直接 `compose up`，虚拟机没起时报一句看不懂的 docker 错误；
/// 3. **单个服务可以单独重启**，不必把六个一起推倒。
///
/// 三条路都只碰 compose 项目 `hunter`，compose 调用一律经 [`crate::compose::run`]
/// （它永远带两份 `-f`，见 `compose::full_args`）。
#[tauri::command]
pub async fn stack_op(
    app: tauri::AppHandle,
    action: String,
    also_runtime: bool,
    service: Option<String>,
) -> Result<StackOpResult> {
    blocking(move || {
        let t0 = std::time::Instant::now();
        let mut steps: Vec<String> = Vec::new();
        let h = app.clone();
        let mut say = move |line: &str| {
            crate::linfo!("栈操作：{line}");
            let _ = h.emit(EV_STACK, line.to_string());
        };

        macro_rules! step {
            ($($t:tt)*) => {{
                let line = format!($($t)*);
                say(&line);
                steps.push(line);
            }};
        }

        match action.as_str() {
            "stop" => {
                compose::stop()?;
                step!("六个服务已经停下来（容器、数据卷、配置全都留着）");
                // 记一笔「这是你自己停的」—— 下次打开启动器时 R3+ 的自愈要绕开它
                config::mark_stopped_by_user(true);
                if also_runtime {
                    if crate::runtime::builtin::is_running()
                        && crate::runtime::effective::builtin_is_effective()
                    {
                        match crate::runtime::builtin::stop() {
                            Ok(t) => step!("{t}"),
                            // 停不掉运行环境不该让整次操作算失败 —— 容器已经停了，
                            // 那是用户真正要的结果。如实说一句就好
                            Err(e) => step!("运行环境没停下来（容器已经停了）：{}", e.msg),
                        }
                    } else {
                        step!("这台机器用的不是内置运行时，没有要停的虚拟机");
                    }
                }
            }
            "start" => {
                config::mark_stopped_by_user(false);
                ensure_runtime_up(&mut steps, &mut say)?;
                compose::up()?;
                step!("六个容器已经起来，正在等它们变健康");
                let cfg = LauncherConfig::load();
                match compose::wait_healthy(cfg.hunter.start_timeout(), |_| {}) {
                    Ok(v) => {
                        let ready = v.iter().filter(|s| compose::service_ready(s)).count();
                        step!("{ready} / {} 健康", v.len());
                    }
                    Err(e) => {
                        step!("等健康超时了：{}", e.msg);
                        return Err(e);
                    }
                }
            }
            "restart" => match service.as_deref().filter(|s| !s.is_empty()) {
                Some(svc) => {
                    if !crate::selfcheck::EXPECTED.contains(&svc) {
                        return Err(AppError::new(
                            Code::Unknown,
                            format!("「{svc}」不是这一套里的服务，不重启它"),
                        ));
                    }
                    compose::restart_services(&[svc])?;
                    step!("{svc} 已经重启（容器没换，端口映射与数据卷都没动）");
                }
                None => {
                    compose::restart()?;
                    step!("六个服务都重启过了");
                }
            },
            other => {
                return Err(AppError::new(
                    Code::Unknown,
                    format!("不认识的操作：{other}"),
                ))
            }
        }

        // 做完之后回头看一眼现状。**结论来自复查，不是来自「我们刚才发了什么命令」**
        let rv = crate::selfcheck::review(false);
        Ok(StackOpResult {
            headline: rv.headline.clone(),
            steps,
            ready: rv.ready,
            total: rv.total,
            elapsed_ms: t0.elapsed().as_millis() as u64,
        })
    })
    .await
}

/// 启动之前先把运行环境弄好（R3 的「启动」那一行）。
///
/// 只对**内置运行时**做事：用户自己的 OrbStack / Docker Desktop 该不该起
/// 是他自己的事，启动器不去替他开别的程序（这一条 I9 起就是这样）。
fn ensure_runtime_up(
    steps: &mut Vec<String>,
    say: &mut impl FnMut(&str),
) -> crate::err::AppResult<()> {
    let eff = crate::runtime::effective::current();
    if eff.running {
        return Ok(());
    }
    if !eff.builtin_down() {
        // 内置运行时不是这台机器的主角，而 docker 又不通 —— 这是另一个问题，
        // 交给它自己的错误路径去说，别在这里装作能修
        return Ok(());
    }
    let mut line = |t: &str| {
        say(t);
        steps.push(t.to_string());
    };
    line("运行环境（虚拟机）没在跑，先把它起起来");
    let mut s = |t: &str| {
        crate::linfo!("启动运行环境：{t}");
    };
    let mut nb = |_: u64, _: u64| {};
    let no_cancel = || false;
    let mut pr = crate::runtime::builtin::Progress {
        say: &mut s,
        bytes: &mut nb,
        cancel: &no_cancel,
    };
    let started = crate::runtime::builtin::start(&mut pr)?;
    line(&started);

    // I10：虚拟机起来了不等于它有 DNS。这一步在 0.1.9 的现场上是决定性的
    let mut s2 = |t: &str| crate::linfo!("检查虚拟机 DNS：{t}");
    let mut pr2 = crate::runtime::builtin::Progress {
        say: &mut s2,
        bytes: &mut nb,
        cancel: &no_cancel,
    };
    match crate::runtime::vmdns::ensure(&mut pr2) {
        Ok(d) if d.healthy() => line(&format!("虚拟机的 DNS 没问题（{}）", d.one_line())),
        Ok(d) => line(&format!("虚拟机的 DNS 还是不行：{}", d.one_line())),
        Err(e) => line(&format!("没查成虚拟机的 DNS：{}", e.msg)),
    }
    Ok(())
}

/// 一键把网页端口收回本机（I7 · 用户 2026-09-21 19:05 的决定第三点）。
///
/// 只有「升级前就对局域网开放」的老机器会看到这个按钮。做三件事：
/// 1. 用 [`config::write_override_local`] 重写覆盖文件（唯一一个显式指定 `Local` 的入口）；
/// 2. `launcher.toml` 里那一项遗迹也顺手改成 `local`，免得日志里一直报「已忽略」；
/// 3. `up -d --force-recreate web` —— 改端口绑定必须**重建**容器，`restart` 不够
///    （与切模型那件事同一个原因，见 `compose::up_services` 的注释）。
///
/// **单向**：收紧之后 `WebBind::detect()` 永远返回 `Local`，
/// 而免费版没有任何一个入口能写回去（局域网访问是付费版功能）。
#[tauri::command]
pub async fn tighten_web_bind(app: tauri::AppHandle) -> Result<String> {
    blocking(move || {
        let st = state(&app);
        let mut c = st.config();
        crate::config::write_override_local(&c.hunter.ports, &c.hunter.base_prefix)?;
        c.hunter.web_bind = config::WEB_BIND_LOCAL.into();
        c.save()?;
        st.set_config(c.clone());
        crate::linfo!(
            "已把网页端口收回 127.0.0.1（端口 {}），正在重建 web 容器",
            c.hunter.ports.web
        );
        // 没在跑就只改配置文件 —— 下次启动自然是本机绑定，不去无端把容器拉起来
        if compose::ps()
            .unwrap_or_default()
            .iter()
            .any(|s| s.service == "web" && s.state == "running")
        {
            compose::up_services(&["web"])?;
            Ok(format!(
                "已收紧：网页现在只有这台电脑能打开（http://localhost:{}）。",
                c.hunter.ports.web
            ))
        } else {
            Ok("已收紧：下次启动时网页就只有这台电脑能打开了。".to_string())
        }
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
    /// 网页端口现在是不是**不止本机**能打开。**只读** ——
    /// 免费版只允许本机访问（用户 2026-09-21 19:05 的决定），设置页里没有开关，
    /// 只有一行说明「局域网访问为付费版功能」。
    ///
    /// 为 `true` 的唯一情形是「这台机器升级前就对外，本轮有意没动它」，
    /// 那时设置页与运行面板各给一个「只允许本机访问」按钮（收紧是单向的）。
    #[serde(default)]
    pub web_lan_exposed: bool,
    /// AI 诊断助手（I4）。**默认开**；关掉之后只用确定性规则，一个 token 也不花
    #[serde(default)]
    pub assist: bool,
    /// I5 授权档位：`auto` 自动驾驶 / `confirm` 逐步确认 / `off` 关闭
    #[serde(default)]
    pub assist_mode: String,
    /// 用户是哪一刻做的授权（上海时间）。空 = 还没授权过
    #[serde(default)]
    pub assist_consented_at: String,
    /// 现在实际用的 docker 可执行文件路径。只读，给界面显示用
    /// （红线 1：读不到就是 `None`，界面显示「—」）
    #[serde(default)]
    pub docker_path: Option<String>,
    /// I7：一次授权页上那一项勾 —— 没有 Docker 时允许 AI 自动装一套
    #[serde(default)]
    pub allow_install_runtime: bool,
    /// I7：没有 Docker 时走哪条路。`builtin`（默认，零点击）/ `orbstack`（备选）
    #[serde(default)]
    pub install_route: String,
    /// I7：内置运行时装了没有（只读）
    #[serde(default)]
    pub builtin_runtime_installed: bool,
    /// I7：内置运行时的虚拟机在跑没有（只读）
    #[serde(default)]
    pub builtin_runtime_running: bool,
    /// I7：现在在管理哪一套别人的 Hunter。空 = 没有接管（只读）
    #[serde(default)]
    pub takeover_project: String,
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
        // I7：「谁能打开 Hunter」不再是设置页能改的东西 —— 免费版只允许本机访问。
        // 前端传什么过来都不看（旧版本的界面、脚本、`--set` 之类都可能还在传）。
        // 要收紧走 `tighten_web_bind`（单向），那是唯一一个能改绑定的入口。
        c.assist.enabled = settings.assist;
        // I5：设置页也能改授权档位。**改档位算一次新的授权**，所以重新盖时间戳、写审计
        if !settings.assist_mode.trim().is_empty() {
            let m = crate::assist::guard::Mode::parse(&settings.assist_mode);
            if m.as_str() != c.assist.mode().as_str() {
                crate::linfo!(
                    "设置页改了授权档位：{} → {}",
                    c.assist.mode().as_str(),
                    m.as_str()
                );
                crate::assist::guard::audit(
                    "consent",
                    &std::collections::BTreeMap::new(),
                    crate::assist::guard::Proposer::User,
                    None,
                    &format!("设置页改档位为 {}（{}）", m.as_str(), m.cn()),
                );
                c.assist.consented_at = crate::timefmt::now_shanghai();
            }
            c.assist.mode = m.as_str().to_string();
            c.assist.enabled = m != crate::assist::guard::Mode::Off;
        }

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
        c.assist.allow_install_runtime = settings.allow_install_runtime;
        // 认不得的值落到默认（`builtin`），不是照抄进去
        c.runtime.install_route = crate::config::InstallRoute::parse(&settings.install_route)
            .as_str()
            .to_string();
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
        // 现状，不是配置里的意图（红线 1）
        web_lan_exposed: crate::config::WebBind::detect().lan_exposed(),
        assist: c.assist.enabled,
        assist_mode: c.assist.mode().as_str().to_string(),
        assist_consented_at: c.assist.consented_at.clone(),
        // 用**当前真实的定位结果**，不是配置里记的那一行（红线 1）
        docker_path: crate::runtime::which::docker_probe().resolved,
        allow_install_runtime: c.assist.allow_install_runtime,
        install_route: c.runtime.route().as_str().to_string(),
        // 这两项同样是**当场读的真实状态**，不是配置里记的
        builtin_runtime_installed: crate::runtime::builtin::is_installed(),
        builtin_runtime_running: crate::runtime::builtin::is_running(),
        takeover_project: c.takeover.project.clone(),
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

/// 装启动器的新版本。**两条路都由启动器自己装完**（I8）。
///
/// * AppImage / Windows / macOS → Tauri 的 updater 就地换掉
/// * `.deb` → 下好包，走 polkit 的原生授权框 `pkexec dpkg -i`
///
/// 成功时装完**自动重启进程**，这个调用不会返回。
#[tauri::command]
pub async fn install_launcher_update(app: tauri::AppHandle) -> Result<()> {
    match crate::selfupdate::install(&app).await {
        Ok(()) => {
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
///
/// 做完顺手按保留策略轮换一次：用户点一次备份就多一份，
/// 「保留最近 3 天」这件事不该只在定时任务那条路上生效。
#[tauri::command]
pub async fn create_backup(app: tauri::AppHandle) -> Result<crate::backup::BackupMeta> {
    blocking(move || {
        let tag = state(&app).config().hunter.tag;
        let r = crate::backup::create(crate::backup::Kind::Manual, &tag, |_| {});
        match &r {
            Ok(_) => {
                crate::backup::record_result(true, None);
                crate::backup::prune();
            }
            Err(e) => {
                crate::backup::record_result(false, Some(&e.msg));
            }
        }
        r
    })
    .await
}

/// 从某次备份把数据库灌回去。**只在用户显式要求时做**。
///
/// 0.1.13 起它走的是 [`crate::backup::restore`] 那条完整的路
/// （先备份当前 → 停服务 → `pg_restore` → 卷与 `JWT_SECRET` → 起回来 → 等健康），
/// 而不是 0.1.12 的「只灌一次 SQL」。`confirm` 必须逐字是「恢复数据」。
#[tauri::command]
pub async fn restore_backup(
    app: tauri::AppHandle,
    id: String,
    confirm: String,
) -> Result<crate::backup::RestoreReport> {
    blocking(move || {
        if confirm.trim() != crate::backup::RESTORE_PHRASE {
            return Err(AppError::new(
                Code::NotImplemented,
                format!(
                    "要恢复得逐字输入「{}」。恢复会把现在的数据替换掉，这一步是故意做成这样的。",
                    crate::backup::RESTORE_PHRASE
                ),
            ));
        }
        crate::backup::restore(&id, |t| {
            let _ = app.emit("hunter://backup", t.to_string());
        })
    })
    .await
}

// ── I13 · R6 数据备份与恢复 ───────────────────────────────────────────────

/// 设置页「数据备份」那一分区读到的东西。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupSettings {
    pub enabled: bool,
    pub time: String,
    /// 用户填的目录（空 = 用建议值）
    pub dir: String,
    /// **现在实际用的**那个目录（只读，界面显示它）
    #[serde(default)]
    pub effective_dir: String,
    pub keep_days: u32,
    pub include_sessions: bool,
    pub include_skills: bool,
    /// 下面这些都是只读的现状
    #[serde(default)]
    pub last_ok_at: String,
    #[serde(default)]
    pub last_error: String,
    #[serde(default)]
    pub last_run_at: String,
    #[serde(default)]
    pub fail_streak: u32,
    #[serde(default)]
    pub count: usize,
    #[serde(default)]
    pub total_bytes: u64,
    /// 备份目录所在盘还剩多少（查不到就是 `None` → 界面「—」）
    #[serde(default)]
    pub disk_free_bytes: Option<u64>,
    /// 按平台建议的目录
    #[serde(default)]
    pub suggested_dir: String,
    /// 检测到的外接盘上的建议位置（**只建议，不自动选**）
    #[serde(default)]
    pub external_suggestions: Vec<String>,
}

fn to_backup_settings(c: &LauncherConfig) -> BackupSettings {
    let eff = c.backup.effective_dir();
    let all = crate::backup::list();
    BackupSettings {
        enabled: c.backup.enabled,
        time: c.backup.time.clone(),
        dir: c.backup.dir.clone(),
        effective_dir: eff.to_string_lossy().into_owned(),
        keep_days: c.backup.keep_days,
        include_sessions: c.backup.include_sessions,
        include_skills: c.backup.include_skills,
        last_ok_at: c.backup.last_ok_at.clone(),
        last_error: c.backup.last_error.clone(),
        last_run_at: c.backup.last_run_at.clone(),
        fail_streak: c.backup.fail_streak,
        count: all.len(),
        total_bytes: crate::backup::total_bytes(),
        disk_free_bytes: crate::monitor::disk_for(&eff).map(|(_, f, _)| f),
        suggested_dir: crate::config::suggested_backup_dir()
            .to_string_lossy()
            .into_owned(),
        external_suggestions: crate::backup::external_suggestions(),
    }
}

#[tauri::command]
pub async fn read_backup_settings(app: tauri::AppHandle) -> Result<BackupSettings> {
    blocking(move || Ok(to_backup_settings(&state(&app).config()))).await
}

/// 存备份设置，并**当场把系统里的定时任务改成一致的样子**。
///
/// 「配置里写着几点」和「系统里真的几点跑」永远不该是两件事 ——
/// 所以这两件事在同一个命令里完成，不给它们分开的机会。
#[tauri::command]
pub async fn write_backup_settings(
    app: tauri::AppHandle,
    settings: BackupSettings,
) -> Result<BackupSettings> {
    blocking(move || {
        let st = state(&app);
        let mut c = st.config();
        if crate::config::parse_hhmm(&settings.time).is_none() {
            return Err(AppError::new(
                Code::ConfigWrite,
                format!(
                    "「{}」不是一个时间。要的是 24 小时制的 HH:MM，例如 00:00 或 07:30。",
                    crate::redact::redact(&settings.time)
                ),
            ));
        }
        // 目录：写得进去才算数 —— 存一个写不进去的路径等于埋一个只有半夜才炸的雷
        let want = crate::config::expand_home(&settings.dir);
        if !settings.dir.trim().is_empty() {
            std::fs::create_dir_all(&want).map_err(|e| {
                AppError::new(
                    Code::ConfigWrite,
                    format!(
                        "建不了备份目录 {}：{e}。换一个目录，或者先把那块盘接上。",
                        crate::redact::mask_home(&want.to_string_lossy())
                    ),
                )
            })?;
            let probe = want.join(".hunter-write-test");
            std::fs::write(&probe, b"ok").map_err(|e| {
                AppError::new(
                    Code::ConfigWrite,
                    format!(
                        "{} 写不进去：{e}。换一个你有写权限的目录。",
                        crate::redact::mask_home(&want.to_string_lossy())
                    ),
                )
            })?;
            let _ = std::fs::remove_file(&probe);
            if want.starts_with(crate::paths::root()) {
                return Err(AppError::new(
                    Code::ConfigWrite,
                    "备份目录不能放在 Hunter 的工作目录里 —— 「删除应用」会把那里整个删掉，                     备份会跟着一起没。换一个别的地方。"
                        .to_string(),
                ));
            }
        }
        c.backup.enabled = settings.enabled;
        c.backup.time = settings.time.trim().to_string();
        c.backup.dir = settings.dir.trim().to_string();
        c.backup.keep_days = settings.keep_days.clamp(1, 30);
        c.backup.include_sessions = settings.include_sessions;
        c.backup.include_skills = settings.include_skills;
        c.save()?;
        st.set_config(c.clone());
        // 系统里的定时任务跟着改。装不上（例如没有 systemd）**不算保存失败** ——
        // 设置该存下来，界面上用 schedule_status 如实说明它现在跑不起来
        if let Err(e) = crate::schedule::sync(|t| crate::linfo!("定时备份：{t}")) {
            crate::lwarn!("定时备份任务没装成：{}", e.msg);
        }
        Ok(to_backup_settings(&c))
    })
    .await
}

/// 定时任务的现状（**现查系统**，不读配置里的备忘）。
#[tauri::command]
pub async fn backup_schedule_status() -> Result<crate::schedule::Status> {
    blocking(|| Ok(crate::schedule::status())).await
}

/// 恢复前的只读体检。
#[tauri::command]
pub async fn restore_preflight(id: String) -> Result<crate::backup::RestorePreflight> {
    blocking(move || crate::backup::preflight(&id)).await
}

/// 列某个任意目录里的备份（换电脑迁移：把备份拷过来，指给启动器看）。
#[tauri::command]
pub async fn backups_in_dir(dir: String) -> Result<Vec<crate::backup::BackupMeta>> {
    blocking(move || {
        let p = crate::config::expand_home(&dir);
        if !p.is_dir() {
            return Err(AppError::new(
                Code::NotImplemented,
                format!("{} 不是一个目录。", crate::redact::mask_home(&dir)),
            ));
        }
        Ok(crate::backup::list_in(&p))
    })
    .await
}

/// 弹系统目录选择框挑备份目录。挑完只返回路径，**不改任何设置**。
#[tauri::command]
pub async fn pick_backup_dir(app: tauri::AppHandle) -> Result<Option<String>> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = std::sync::mpsc::channel();
    app.dialog()
        .file()
        .set_title("选一个放备份的文件夹")
        .pick_folder(move |p| {
            let _ = tx.send(p.map(|x| x.to_string()));
        });
    Ok(rx.recv().unwrap_or(None))
}

/// 恢复要逐字输入的那句话（界面拿它填提示，不自己另写一套）。
#[tauri::command]
pub async fn restore_confirm_text() -> Result<String> {
    Ok(crate::backup::RESTORE_PHRASE.to_string())
}

// ── I13 · R4 删除应用 ─────────────────────────────────────────────────────

/// 这次删除会动到什么（只读）。`deep` 为真时连表数与最近写入一起问出来。
#[tauri::command]
pub async fn uninstall_plan(scope: String, deep: bool) -> Result<crate::uninstall::Plan> {
    blocking(move || {
        Ok(crate::uninstall::plan(
            crate::uninstall::Scope::parse(&scope),
            deep,
        ))
    })
    .await
}

/// 这一档要逐字输入的那句话。
#[tauri::command]
pub async fn uninstall_confirm_text(scope: String) -> Result<String> {
    Ok(crate::uninstall::confirm_phrase(crate::uninstall::Scope::parse(&scope)).to_string())
}

/// 真的删。过程逐条推 `hunter://uninstall`。
#[tauri::command]
pub async fn uninstall_run(
    app: tauri::AppHandle,
    options: crate::uninstall::Options,
) -> Result<crate::uninstall::Report> {
    blocking(move || {
        let r = crate::uninstall::run(&options, |t| {
            let _ = app.emit("hunter://uninstall", t.to_string());
        });
        if r.is_ok() {
            let st = state(&app);
            st.set_config(crate::config::LauncherConfig::load());
        }
        r
    })
    .await
}

// ── I13 · R7 异常监测 ─────────────────────────────────────────────────────

/// 跑一遍七类监测，返回提醒列表（运行面板的横幅读它）。
#[tauri::command]
pub async fn monitor_alerts() -> Result<crate::monitor::Alerts> {
    blocking(|| Ok(crate::monitor::alerts())).await
}

/// 能安全清掉多少（只读）。
#[tauri::command]
pub async fn cleanup_plan() -> Result<crate::cleanup::Plan> {
    blocking(|| Ok(crate::cleanup::plan())).await
}

/// 一键清理。**只清本项目旧镜像、悬空卷、过期备份**，别的一律不动。
#[tauri::command]
pub async fn cleanup_run(app: tauri::AppHandle) -> Result<crate::cleanup::Done> {
    blocking(move || {
        crate::cleanup::run(|t| {
            let _ = app.emit("hunter://cleanup", t.to_string());
        })
    })
    .await
}

/// 规则层判不了的那几条，把**脱敏指标快照**交给诊断助手（沿用现有预算与动作表）。
#[tauri::command]
pub async fn assist_resource_ask(app: tauri::AppHandle, alert_id: String) -> Result<String> {
    blocking(move || {
        let a = crate::monitor::alerts();
        let Some(one) = a.alerts.iter().find(|x| x.id == alert_id) else {
            return Err(AppError::new(
                Code::NotImplemented,
                format!(
                    "现在已经没有「{}」这条提醒了 —— 可能它自己好了。",
                    crate::redact::redact(&alert_id)
                ),
            ));
        };
        let snap = crate::monitor::snapshot_for_ai(&a);
        crate::assist::ask_resource(&state(&app), one, &snap)
    })
    .await
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
    use super::{route_of, BootRoute};
    use crate::selfcheck::Posture;

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

    /// **R1 的五行表，一行一条**（方案第二节 R1）。
    ///
    /// 这张表的要害是最后一列：`install.done` 写的是什么**都不影响**前三行 ——
    /// 2026-09-22 23:10 用户 Mac 上正是「6/6 健康 + done=false」，
    /// 而 0.1.11 把它判成了「没装过」，于是从欢迎页开始。
    #[test]
    fn r1_五行表_逐行钉死() {
        // ① 本项目 6/6 健康 → 运行面板
        assert_eq!(route_of(Posture::Healthy, 0), BootRoute::Dashboard);
        assert_eq!(route_of(Posture::Healthy, 6), BootRoute::Dashboard);
        // ② 本项目存在但已停止 / ③ 部分不健康 → 运行面板（两种都落在 Partial）
        assert_eq!(route_of(Posture::Partial, 0), BootRoute::Dashboard);
        assert_eq!(route_of(Posture::Partial, 6), BootRoute::Dashboard);
        // 只装了一半也算装过 —— 不回向导
        assert_eq!(route_of(Posture::Incomplete, 0), BootRoute::Dashboard);
        assert_eq!(route_of(Posture::Incomplete, 6), BootRoute::Dashboard);
        // ④ 本项目不存在，但数据卷还在 → 「检测到上次的数据」
        assert_eq!(route_of(Posture::Absent, 6), BootRoute::DataFound);
        assert_eq!(route_of(Posture::Absent, 1), BootRoute::DataFound);
        // ⑤ 什么都没有 → 欢迎页
        assert_eq!(route_of(Posture::Absent, 0), BootRoute::Welcome);
    }

    /// 「补写安装标记」只在**该进运行面板**而且标记确实是假的时候发生。
    #[test]
    fn 只有进运行面板那几行才补写安装标记() {
        for (posture, vols) in [
            (Posture::Healthy, 0),
            (Posture::Partial, 0),
            (Posture::Incomplete, 0),
        ] {
            assert_eq!(route_of(posture, vols), BootRoute::Dashboard);
        }
        // 只剩数据卷 / 全新这两行绝不补写 —— 那两台机器上确实没有装好的 Hunter
        assert_ne!(route_of(Posture::Absent, 3), BootRoute::Dashboard);
        assert_ne!(route_of(Posture::Absent, 0), BootRoute::Dashboard);
    }
}

// ── AI 诊断助手（I4 §三） ─────────────────────────────────────────────────
//
// 两层的调用顺序全在 `crate::assist`，这里只是把它摆到 IPC 上。
// 所有耗时（跑 docker、读日志、问网关）都在 blocking 线程里，界面不会被卡住。

/// 第一层 · 确定性规则。零 token，任何时候都能调。
#[tauri::command]
pub async fn assist_diagnose(
    app: tauri::AppHandle,
    error_code: Option<String>,
    error_message: Option<String>,
    stage: Option<String>,
) -> Result<crate::assist::AssistState> {
    blocking(move || {
        // key 也要带上：界面右下角要据此显示「还没填 key」还是不显示，
        // 而向导途中 key 只在内存里（见 assist::ask 的注释）
        let k = state(&app).hunter_key();
        let st = crate::assist::diagnose_and_fix(
            error_code.as_deref(),
            error_message.as_deref(),
            stage.as_deref(),
            k,
        )?;
        maybe_resume_install(&app, &st);
        Ok(st)
    })
    .await
}

/// 第二层 · 问一轮 AI。任何一条前提不满足都会在返回值的 `degraded` 里说明原因。
#[tauri::command]
pub async fn assist_ask(app: tauri::AppHandle) -> Result<crate::assist::AssistState> {
    blocking(move || {
        // key 从 AppState 拿：I4 把「输入 key」挪到了「检测 Docker」前面，
        // 这时候 `.env` 还没写出来，key 只在内存里（见 assist::ask 的注释）
        let k = state(&app).hunter_key();
        crate::assist::ask(k)
    })
    .await
}

/// 用户确认执行一个**会改动机器**的动作，执行完自动复验并接着问下一轮。
#[tauri::command]
pub async fn assist_confirm(
    app: tauri::AppHandle,
    action_id: String,
    next_round: bool,
) -> Result<crate::assist::AssistState> {
    blocking(move || {
        let k = state(&app).hunter_key();
        let st = crate::assist::confirm(&action_id, next_round, k)?;
        maybe_resume_install(&app, &st);
        Ok(st)
    })
    .await
}

/// 执行**规则层**给的动作（这条路不经过模型）。执行完重新跑一遍规则。
#[tauri::command]
pub async fn assist_rule_action(
    app: tauri::AppHandle,
    action_id: String,
) -> Result<crate::assist::AssistState> {
    blocking(move || {
        let k = state(&app).hunter_key();
        let st = crate::assist::run_rule_action(&action_id, k)?;
        maybe_resume_install(&app, &st);
        Ok(st)
    })
    .await
}

/// **修好了就接着装**（I9 的 P0-3）。
///
/// 诊断助手（规则层或模型）跑完任何一个动作之后调一次：只要 docker 变成可用、
/// 而这一套 Hunter 还没装完，就**自己把安装主流程重新跑起来**，
/// 并发一条 `assist://resume` 让界面回到过程流那一页。
///
/// 0.1.8 在用户 Mac 上缺的正是这一步：14:54:27 内置运行时已经起来了、
/// docker 29.5.2 可用，可 `~/.hunter/app/` 到 14:58 还是空的 ——
/// 没写 `.env`、没写 compose、没拉镜像、容器 0 个，界面停在「没能自动装好」。
/// 修好了却不往下走，对用户来说和没修一样。
///
/// **三道闸门**防止它乱来：
/// * 已经有一次安装在跑（`busy`）就什么都不做；
/// * `can_resume_install()` 说装完了就什么都不做；
/// * 一个启动器进程里最多自动续装 [`MAX_AUTO_RESUME`] 次。
///
/// 第三道是必须的，而且不是理论上的谨慎。没有它就是一个真的死循环：
/// 安装失败 → 错误页挂载 → 界面调 `assist_diagnose` → 这里判「docker 可用、
/// 这一套没装完」→ 重新开跑 → 又在同一个地方失败 → 错误页又挂载 → …
/// 卡在「起容器」那一类问题上时（例如待办池 P1-29 的旧卷口令对不上），
/// 它会一直转下去，而用户看到的是界面自己在反复闪。
///
/// 到头之后**如实停下**：如实写进日志，界面停在失败页 —— 那时候
/// 「反复自动重试」已经证明没用了，继续转只是把问题藏起来。
fn maybe_resume_install(app: &tauri::AppHandle, st: &crate::assist::AssistState) {
    if !st.can_resume_install {
        return;
    }
    // **没授权过就不开跑。** 按现在的向导路线走不到这里（授权页在 Docker 那一关
    // 前面），但「不经用户同意不开始安装」这件事不该靠前端路由保证 ——
    // 路由是会改的，这一句不会。
    if !crate::config::LauncherConfig::load().assist.consented() {
        return;
    }
    if state(app).busy.load(Ordering::SeqCst) {
        return;
    }
    let used = auto_resumes().fetch_add(1, Ordering::SeqCst);
    if used >= MAX_AUTO_RESUME {
        crate::lwarn!(
            "Docker 可用、这一套还没装完，但这个进程里已经自动续装过 {MAX_AUTO_RESUME} 次了 ——              不再自动重来（再转下去只是把问题藏起来）。"
        );
        return;
    }
    crate::linfo!(
        "Docker 现在可用了，而这一套还没装完 —— 自动接着往下装（第 {} 次）",
        used + 1
    );
    let _ = app.emit(EV_ASSIST_RESUME, ());
    if let Err(e) = assist_auto_start(app.clone(), None) {
        crate::lwarn!("自动接着装没能起来：{e}");
    }
}

/// 一个启动器进程里最多自动续装几次（I9）。
///
/// 为什么是 2 而不是更多：第一次是「刚把 Docker 修好，接着装」——
/// 这正是 P0-3 要的那一次；第二次留给「修好之后又撞上另一个能自动解决的问题」。
/// 再多就不是「接着装」了，是在同一个地方打转。
pub const MAX_AUTO_RESUME: usize = 2;

fn auto_resumes() -> &'static std::sync::atomic::AtomicUsize {
    static C: std::sync::OnceLock<std::sync::atomic::AtomicUsize> = std::sync::OnceLock::new();
    C.get_or_init(|| std::sync::atomic::AtomicUsize::new(0))
}

#[tauri::command]
pub async fn assist_reset() -> Result<()> {
    blocking(crate::assist::reset).await
}

// ── I5 · AI 自动驾驶安装（设计文档 §二～八） ──────────────────────────────

/// 把事件推到窗口上的 sink。headless 那条路用 [`crate::assist::events::Stdout`]，
/// 两条路共用同一份事件与同一个 [`Bus`]。
struct TauriSink(tauri::AppHandle);

impl crate::assist::events::Sink for TauriSink {
    fn event(&self, e: &crate::assist::events::Event) {
        let _ = self.0.emit(crate::assist::events::EVENT, e);
    }
    fn summary(&self, s: &crate::assist::events::Summary) {
        let _ = self.0.emit(EV_ASSIST_SUMMARY, s);
    }
}

pub const EV_ASSIST_SUMMARY: &str = "assist://summary";
pub const EV_ASSIST_DONE: &str = "assist://done";
/// 「修好了，接着装」——后端已经自己把安装重新跑起来了，界面回到过程流那一页（I9）。
pub const EV_ASSIST_RESUME: &str = "assist://resume";

/// 正在跑的那一次自动安装。用户点「需要你」卡片时要找得到它。
type AutoSlot = std::sync::Mutex<Option<crate::assist::auto::Handle>>;

fn auto_slot() -> &'static AutoSlot {
    static S: std::sync::OnceLock<AutoSlot> = std::sync::OnceLock::new();
    S.get_or_init(|| std::sync::Mutex::new(None))
}

/// 用户在一次授权页上做的选择（设计文档 §3.1）。
///
/// **写日志、写配置、写审计**：授权这件事必须留痕，用户以后要能查到
/// 「我是什么时候、同意了哪一档」。
#[tauri::command]
/// `allow_install_runtime` 是一次授权页上那一项勾（I7）：
/// 「电脑上没有 Docker 时，允许 AI 为你安装」。默认勾着；
/// `None` 表示旧版界面没传，按默认（勾着）处理。
pub async fn assist_consent(
    app: tauri::AppHandle,
    mode: String,
    allow_install_runtime: Option<bool>,
) -> Result<LauncherSettings> {
    blocking(move || {
        let m = crate::assist::guard::Mode::parse(&mode);
        let allow = allow_install_runtime.unwrap_or(true);
        let st = state(&app);
        let mut cfg = st.config();
        cfg.assist.mode = m.as_str().to_string();
        cfg.assist.enabled = m != crate::assist::guard::Mode::Off;
        cfg.assist.consented_at = crate::timefmt::now_shanghai();
        cfg.assist.allow_install_runtime = allow;
        cfg.save()?;
        st.set_config(cfg.clone());
        crate::linfo!(
            "用户授权：档位 {}（{}），没有 Docker 时允许自动安装={}，时间 {}",
            m.as_str(),
            m.cn(),
            allow,
            cfg.assist.consented_at
        );
        crate::assist::guard::audit(
            "consent",
            &std::collections::BTreeMap::from([(
                "allow_install_runtime".to_string(),
                allow.to_string(),
            )]),
            crate::assist::guard::Proposer::User,
            None,
            &format!(
                "授权档位 {}（{}）；没有 Docker 时{}自动安装运行时",
                m.as_str(),
                m.cn(),
                if allow { "允许" } else { "不允许" }
            ),
        );
        Ok(to_settings(&cfg))
    })
    .await
}

// ── I7 · 内置运行时 ───────────────────────────────────────────────────────

/// 内置运行时现在什么情况（设置页用）。**只读**。
#[tauri::command]
pub async fn builtin_runtime_status() -> Result<crate::runtime::builtin::Status> {
    blocking(move || Ok(crate::runtime::builtin::status())).await
}

/// 卸载内置运行时（设置页的「卸载内置运行时」）。
///
/// 走的是动作表那条路：`uninstall_builtin_runtime` 是 `Safe` 级
/// （只动 `~/.hunter/runtime`，那棵树完全是启动器自己生成的），
/// 但**界面上仍然会先弹一次确认** —— 删虚拟机磁盘这种事值得多问一句。
#[tauri::command]
pub async fn builtin_runtime_uninstall(app: tauri::AppHandle) -> Result<String> {
    blocking(move || {
        let out = crate::assist::actions::execute_as(
            &crate::assist::actions::Call::new("uninstall_builtin_runtime"),
            crate::assist::guard::Mode::Confirm,
            true,
            crate::assist::guard::Proposer::User,
        )?;
        let st = state(&app);
        st.set_config(crate::config::LauncherConfig::load());
        Ok(out.text)
    })
    .await
}

// ── I7 · 接管本机已有的那一套 ─────────────────────────────────────────────

/// 本机上可以接管的那几套（只读探测）。
#[tauri::command]
pub async fn takeover_candidates() -> Result<Vec<crate::takeover::Candidate>> {
    blocking(move || Ok(crate::takeover::candidates())).await
}

/// 当前接管态（运行面板用）。
#[tauri::command]
pub async fn takeover_state() -> Result<crate::takeover::State> {
    blocking(move || Ok(crate::takeover::state())).await
}

/// 「直接用它，不再装一套」。走动作表 → 守卫 → 审计。
#[tauri::command]
pub async fn takeover_adopt(app: tauri::AppHandle, project: String) -> Result<String> {
    blocking(move || {
        let out = crate::assist::actions::execute_as(
            &crate::assist::actions::Call::with("reuse_existing_hunter", "project", &project),
            crate::assist::guard::Mode::Confirm,
            true,
            crate::assist::guard::Proposer::User,
        )?;
        let st = state(&app);
        st.set_config(crate::config::LauncherConfig::load());
        Ok(out.text)
    })
    .await
}

/// 撤回接管，回到「自己装一套」。**不动被接管的那一套一个字节。**
#[tauri::command]
pub async fn takeover_release(app: tauri::AppHandle) -> Result<String> {
    blocking(move || {
        let t = crate::takeover::release()?;
        let st = state(&app);
        st.set_config(crate::config::LauncherConfig::load());
        Ok(t)
    })
    .await
}

/// 对被接管那一套做 stop / start / restart。**`confirmed` 必须为真**，
/// 而且这一层拒绝之后界面上要再弹一次二次确认（文案由 `confirm_text` 给）。
#[tauri::command]
pub async fn takeover_op(op: String, confirmed: bool) -> Result<String> {
    blocking(move || {
        let o = crate::takeover::Op::parse(&op).ok_or_else(|| {
            crate::err::AppError::new(
                crate::err::Code::NotImplemented,
                format!("认不得的操作「{}」。", crate::redact::redact(&op)),
            )
        })?;
        crate::takeover::run(o, confirmed)
    })
    .await
}

/// 二次确认要给用户看的那句话（界面拿它填弹窗，不自己另写一套）。
#[tauri::command]
pub async fn takeover_confirm_text(op: String) -> Result<String> {
    blocking(move || {
        let cfg = crate::config::LauncherConfig::load();
        let o = crate::takeover::Op::parse(&op).ok_or_else(|| {
            crate::err::AppError::new(
                crate::err::Code::NotImplemented,
                format!("认不得的操作「{}」。", crate::redact::redact(&op)),
            )
        })?;
        Ok(o.confirm_text(&cfg.takeover.project))
    })
    .await
}

/// 被接管那一套的日志（**只读**，已脱敏）。
#[tauri::command]
pub async fn takeover_logs(service: Option<String>, lines: Option<usize>) -> Result<Vec<String>> {
    blocking(move || crate::takeover::logs(service.as_deref(), lines.unwrap_or(200))).await
}

// ── I7 · 一键反馈直达 ─────────────────────────────────────────────────────

/// 生成脱敏诊断包 + 预填 issue。**不打开浏览器、不发送任何东西。**
#[tauri::command]
pub async fn feedback_one_click(
    error_code: Option<String>,
    error_message: Option<String>,
) -> Result<crate::feedback::OneClick> {
    blocking(move || {
        crate::feedback::one_click(
            error_code.as_deref().unwrap_or(""),
            error_message.as_deref().unwrap_or(""),
        )
    })
    .await
}

/// 开始一次**全自动**安装。立刻返回，过程走 `assist://event`。
#[tauri::command]
pub fn assist_auto_start(app: tauri::AppHandle, registry_id: Option<String>) -> Result<()> {
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
        let bus = std::sync::Arc::new(crate::assist::events::Bus::new(
            Box::new(TauriSink(handle.clone())),
            true,
        ));
        let mut orch = crate::assist::auto::Orchestrator::new(
            bus,
            cfg.assist.mode(),
            st.hunter_key(),
            st.cancel.clone(),
        );
        if let Ok(mut g) = auto_slot().lock() {
            *g = Some(orch.handle());
        }
        let outcome = orch.run(&st, &opts);
        let _ = handle.emit(EV_ASSIST_DONE, &outcome);
        if let Ok(mut g) = auto_slot().lock() {
            *g = None;
        }
        st.busy.store(false, Ordering::SeqCst);
    });
    Ok(())
}

/// 用户点了「需要你」卡片上的某个按钮。
#[tauri::command]
pub fn assist_auto_answer(value: String) -> Result<bool> {
    let g = auto_slot().lock().ok().and_then(|g| g.clone());
    Ok(match g {
        Some(o) => o.answer(&value),
        None => false,
    })
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoSnapshot {
    pub running: bool,
    pub events: Vec<crate::assist::events::Event>,
    pub summary: crate::assist::events::Summary,
}

/// 页面刚挂载时先拉一份整棵树（事件可能在挂载前就发过了）。
#[tauri::command]
pub fn assist_auto_snapshot() -> Result<AutoSnapshot> {
    let g = auto_slot().lock().ok().and_then(|g| g.clone());
    Ok(match g {
        Some(o) => AutoSnapshot {
            running: true,
            events: o.bus.snapshot(),
            summary: o.bus.summary_now(),
        },
        None => AutoSnapshot {
            running: false,
            events: Vec::new(),
            summary: crate::assist::events::Summary::default(),
        },
    })
}

/// **现状复查**：Hunter 现在到底在不在跑（I11 · U1）。
///
/// 只读。错误页拿它做低频复查 —— 一旦查到六个服务全就绪、网页也打得开，
/// 界面自己切到运行面板，而不是把用户留在一张 41 分钟前的失败卡片上
/// （0.1.9 在用户 Mac 上就是那样）。
///
/// `deep` 为真时多探一次「容器连不连得上模型网关」，要几秒到几十秒 ——
/// 界面上的轮询一律用浅查。
#[tauri::command]
pub async fn self_check(deep: Option<bool>) -> Result<crate::selfcheck::Review> {
    blocking(move || Ok(crate::selfcheck::review(deep.unwrap_or(false)))).await
}

/// 审计日志的末尾若干条（设置页「AI 都做过什么」）。
#[tauri::command]
pub async fn assist_audit_tail(lines: usize) -> Result<Vec<String>> {
    blocking(move || Ok(crate::assist::guard::audit_tail(lines.min(500)))).await
}
