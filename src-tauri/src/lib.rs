//! Hunter 启动器的 Rust 侧。
//!
//! M2 起这里是真实现：Docker 检测、key 校验、镜像源测速、拉镜像、写配置、起容器
//! 全部走真实调用；拿不到的数据一律返回 `null` 或 `Err`，由界面显示「—」和原因
//! （总控规则红线 1：不用假数据冒充功能）。
//!
//! 模块划分对应技术方案附录 A：
//!
//! | 模块 | 方案节 | 干什么 |
//! |---|---|---|
//! | [`runtime::docker`] | §5.1 | Docker / daemon / compose 检测，三平台安装引导 |
//! | [`compose`] | §5.2、§9 | pull 进度、up、健康轮询、down/stop/restart/logs |
//! | [`config`] | §5.3、§7 | launcher.toml、.env（600）、端口冲突、覆盖文件、compose 文件 |
//! | [`gateway`] | §5.4、§8 | key 校验、额度、模型列表、自带 key 连通性 |
//! | [`registry`] | §5.5 | 候选镜像源探测与测速、manifest 大小 |
//! | [`flow`] | — | 把上面串成一次完整安装，GUI 与 headless 共用 |
//! | [`headless`] | §6 | 纯命令行版 |
//! | [`redact`] / [`log`] | §7、§12.3 | 滚动日志与脱敏（红线 2） |

use serde::Serialize;
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

pub mod commands;
pub mod compose;
pub mod config;
pub mod err;
pub mod flow;
pub mod gateway;
pub mod headless;
pub mod http;
pub mod log;
pub mod paths;
pub mod proc;
pub mod redact;
pub mod registry;
pub mod runtime;
pub mod secretgen;
pub mod timefmt;

/// 主窗口的逻辑尺寸。视觉稿窗体实测 2200×1380 像素（2 倍图），即 1100×690 逻辑像素，
/// 宽高比 1.594。这里按里程碑要求取 1180×760（比例 1.553，肉眼无差），内容区因此比稿子多一点余量。
const WIN_W: f64 = 1180.0;
const WIN_H: f64 = 760.0;
const MIN_W: f64 = 980.0;
const MIN_H: f64 = 660.0;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    launcher_version: String,
    platform: String,
    arch: String,
    /// native = 用系统原生标题栏（macOS 的红绿灯）；custom = 前端自绘三个圆点
    window_chrome: String,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 日志要在任何业务代码之前初始化，否则最早的几行会丢
    let _ = paths::ensure_dirs();
    log::init(paths::launcher_log());
    linfo!(
        "启动器 {} 启动 · {} {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    );

    let mut builder = tauri::Builder::default();

    // 官方要求 single-instance 第一个注册（M0 §7.4 已验证这套骨架能编过）
    #[cfg(all(desktop, not(any(target_os = "android", target_os = "ios"))))]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
        }));
    }

    builder
        .plugin(tauri_plugin_opener::init())
        .manage(flow::AppState::new())
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::window_minimize,
            commands::window_toggle_maximize,
            commands::window_close,
            commands::open_external,
            commands::boot_state,
            commands::detect_docker,
            commands::start_daemon,
            commands::validate_key,
            commands::set_model,
            commands::probe_registries,
            commands::start_install,
            commands::cancel_install,
            commands::pull_progress,
            commands::start_stack,
            commands::starting_status,
            commands::runtime_status,
            commands::stack_action,
            commands::compose_logs,
            commands::launcher_log,
            commands::read_settings,
            commands::write_settings,
            commands::diagnostics,
        ])
        .setup(|app| {
            build_main_window(app.handle())?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("启动器窗口创建失败");
}

/// 建主窗口。标题栏策略按平台分开：
///
/// * **macOS**：保留系统装饰，用 `TitleBarStyle::Overlay` + `hidden_title` 让内容顶到窗口最上面，
///   红绿灯还是系统原生的那三颗 —— mac 用户对它们的位置、悬停图标、双击行为都有肌肉记忆，
///   自绘一套只会哪里都不对。前端拿到 `windowChrome: "native"` 后不画圆点，只留 70px 空位。
/// * **Windows / Linux**：去掉系统装饰，前端自绘视觉稿上那三个圆点。
fn build_main_window(app: &tauri::AppHandle) -> tauri::Result<()> {
    let mut win = WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
        .title("Hunter Launcher")
        .inner_size(WIN_W, WIN_H)
        .min_inner_size(MIN_W, MIN_H)
        .resizable(true)
        .center();

    #[cfg(target_os = "macos")]
    {
        win = win
            .decorations(true)
            .title_bar_style(tauri::TitleBarStyle::Overlay)
            .hidden_title(true);
    }

    #[cfg(not(target_os = "macos"))]
    {
        win = win.decorations(false);
    }

    // 演示数据模式（VITE_DEMO=1 的开发构建）用初始化脚本指定直接打开哪一页，给截图脚本用。
    // 这里只是把值透给前端，**是否采信由前端的 DEMO 常量决定**；发布构建里 DEMO 恒为 false，
    // 这个值读不出任何效果（总控规则红线 1）。
    if let Ok(page) = std::env::var("HUNTER_DEMO_PAGE") {
        if page.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') && page.len() <= 32 {
            win = win.initialization_script(format!("window.__HUNTER_DEMO_PAGE__ = {page:?};"));
        }
    }

    win.build()?;
    Ok(())
}
