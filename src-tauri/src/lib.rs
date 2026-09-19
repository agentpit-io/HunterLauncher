//! Hunter 启动器的 Rust 侧。
//!
//! M1 只做壳：建窗口（三平台的标题栏策略不同）、暴露少量**真实**命令、其余业务命令一律
//! 返回 `E_NOT_IMPLEMENTED`，由前端显示「—」和原因（总控规则红线 1：不用假数据冒充功能）。
//! Docker 检测、key 校验、拉镜像、起容器这些在 M2 / M3 里实现。

use serde::Serialize;
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

mod commands;

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
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::window_minimize,
            commands::window_toggle_maximize,
            commands::window_close,
            commands::open_external,
            commands::detect_docker,
            commands::validate_key,
            commands::pull_progress,
            commands::runtime_status,
            commands::starting_status,
            commands::read_settings,
            commands::launcher_log,
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
/// * **Windows / Linux**：去掉系统装饰，前端自绘视觉稿上那三个圆点
///   （左=关闭、中=最小化、右=最大化，语义与 macOS 一致；平时是灰蓝的，悬停整组才显色）。
///   标题栏本身带 `data-tauri-drag-region`，拖拽与双击最大化都照常。
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

    // 演示数据模式（VITE_DEMO=1 的开发构建）用环境变量指定直接打开哪一页，给截图脚本用。
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
