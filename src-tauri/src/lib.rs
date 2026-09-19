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
//! | [`tray`] | §5.8 | 托盘菜单、状态随服务变化、退出询问 |
//! | [`autostart`] | §5.8 | 开机自启（三平台，默认关） |
//! | [`telemetry`] | §12 | 本地事件队列（默认关、默认不上报） |
//! | [`feedback`] / [`zip`] | §11.1、§12.3 | 诊断包收集、脱敏自查、导出 zip |
//! | [`upstream`] | §13 | 从本机 api 读面板要的数字（读不到就 `—`） |

use serde::Serialize;
use tauri::{Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

pub mod autostart;
pub mod commands;
pub mod compose;
pub mod config;
pub mod err;
pub mod feedback;
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
pub mod telemetry;
pub mod timefmt;
pub mod tray;
pub mod upstream;
pub mod zip;

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
    // `--tray-menu` 那一路只是个转发器：single-instance 会把参数交给已经在跑的实例，
    // 它自己几毫秒后就退了。写「启动器启动」会让日志里凭空多出一堆假的启动记录。
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if let Some(id) = tray_menu_arg(&argv) {
        linfo!("转发托盘指令 {id} 给已经在跑的实例");
    } else {
        linfo!(
            "启动器 {} 启动 · {} {}",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH
        );
    }

    let mut builder = tauri::Builder::default();

    // 官方要求 single-instance 第一个注册（M0 §7.4 已验证这套骨架能编过）
    #[cfg(all(desktop, not(any(target_os = "android", target_os = "ios"))))]
    {
        // 第二个进程带 `--tray-menu <id>` 时，由**正在跑的这个实例**去执行那一项菜单动作。
        // 这既是给 SSH 用户的遥控口（`hunter-launcher --tray-menu stop`），
        // 也让「托盘菜单每一项点一遍」在 Xvfb 下真的验得了 ——
        // 无桌面环境里托盘图标是看不见的（M0 §7.3），但走的是同一个 `tray::run_action`。
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            if let Some(id) = tray_menu_arg(&argv) {
                linfo!("收到另一个进程的托盘指令：{id}");
                let h = app.clone();
                std::thread::spawn(move || tray::run_action(&h, &id));
                return;
            }
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
            commands::telemetry_view,
            commands::telemetry_clear,
            commands::set_autostart,
            commands::switch_model,
            commands::diagnostics_sections,
            commands::export_diagnostics,
            commands::feedback_issue_url,
            commands::export_logs,
            commands::tray_invoke,
            commands::quit_app,
            commands::missing_endpoints,
            commands::reveal_path,
        ])
        .setup(|app| {
            let handle = app.handle();
            build_main_window(handle)?;

            // 托盘建不出来不是致命错误：没有状态栏的环境（Xvfb、精简桌面）本来就建不了，
            // 而托盘只是增强入口，界面上每一项都另有入口（M0 §7.3）。
            match tray::build(handle) {
                Ok(()) => linfo!("托盘已建立：{} 个菜单项", tray::MENU_IDS.len()),
                Err(e) => lwarn!("托盘没建起来（不影响使用，界面上每一项都另有入口）：{e}"),
            }

            // 关窗口 = 请求退出。容器还在跑就先问一句「保持后台运行 / 一起停止」（方案 §5.8）。
            if let Some(w) = handle.get_webview_window("main") {
                let h = handle.clone();
                w.on_window_event(move |e| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = e {
                        if compose::is_up() {
                            api.prevent_close();
                            let _ = h.emit(tray::EV_QUIT_REQUEST, true);
                        }
                    }
                });
            }

            // 第一条遥测事件。开关默认关着，这一句在绝大多数机器上什么都不会写。
            {
                let st = handle.state::<flow::AppState>();
                let cfg = st.config();
                telemetry::record(
                    cfg.telemetry.enabled,
                    &cfg.telemetry.install_id,
                    "launcher_start",
                    &[
                        (
                            "version",
                            telemetry::Field::Enum(env!("CARGO_PKG_VERSION").into()),
                        ),
                        ("os", telemetry::Field::Enum(std::env::consts::OS.into())),
                        (
                            "arch",
                            telemetry::Field::Enum(std::env::consts::ARCH.into()),
                        ),
                        (
                            "locale",
                            telemetry::Field::Enum(cfg.launcher.locale.clone()),
                        ),
                    ],
                );
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("启动器窗口创建失败");
}

/// 从一串命令行参数里认出 `--tray-menu <id>`。
///
/// 只认**托盘菜单里真有的那些 id**（[`tray::MENU_IDS`]）—— 这个入口能让任何本机进程
/// 指使启动器停容器、开自启，不能让它变成一个「随便传个字符串进来看看会怎样」的口子。
pub fn tray_menu_arg(argv: &[String]) -> Option<String> {
    let mut it = argv.iter();
    while let Some(a) = it.next() {
        let val = if a == "--tray-menu" {
            it.next().map(|s| s.as_str())
        } else {
            a.strip_prefix("--tray-menu=")
        };
        if let Some(v) = val {
            if tray::MENU_IDS.contains(&v) {
                return Some(v.to_string());
            }
            lwarn!("--tray-menu 收到一个不认识的菜单项：{v}");
            return None;
        }
    }
    None
}

/// 开机自启拉起来的那一次带 `--minimized`：**进程起来、托盘出现，但不弹窗**。
pub fn minimized_arg(argv: &[String]) -> bool {
    argv.iter().any(|a| a == "--minimized")
}

/// 建主窗口。标题栏策略按平台分开：
///
/// * **macOS**：保留系统装饰，用 `TitleBarStyle::Overlay` + `hidden_title` 让内容顶到窗口最上面，
///   红绿灯还是系统原生的那三颗 —— mac 用户对它们的位置、悬停图标、双击行为都有肌肉记忆，
///   自绘一套只会哪里都不对。前端拿到 `windowChrome: "native"` 后不画圆点，只留 70px 空位。
/// * **Windows / Linux**：去掉系统装饰，前端自绘视觉稿上那三个圆点。
fn build_main_window(app: &tauri::AppHandle) -> tauri::Result<()> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut win = WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
        .title("Hunter Launcher")
        .inner_size(WIN_W, WIN_H)
        .min_inner_size(MIN_W, MIN_H)
        .resizable(true)
        // 开机自启那一次只出托盘，不弹窗（`autostart` 写进自启项的就是这个参数）
        .visible(!minimized_arg(&argv))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 认得出_tray_menu_参数的两种写法() {
        assert_eq!(
            tray_menu_arg(&["--tray-menu".into(), "stop".into()]),
            Some("stop".into())
        );
        assert_eq!(
            tray_menu_arg(&["--tray-menu=restart".into()]),
            Some("restart".into())
        );
        assert_eq!(tray_menu_arg(&["--minimized".into()]), None);
        assert_eq!(tray_menu_arg(&[]), None);
    }

    #[test]
    fn 菜单项之外的值一律不认() {
        // 这个入口能指使启动器停容器，不能变成任意字符串的口子
        assert_eq!(tray_menu_arg(&["--tray-menu".into(), "rm-rf".into()]), None);
        assert_eq!(tray_menu_arg(&["--tray-menu".into(), "".into()]), None);
    }

    #[test]
    fn minimized_参数() {
        assert!(minimized_arg(&["--minimized".into()]));
        assert!(!minimized_arg(&["--headless".into()]));
    }
}
