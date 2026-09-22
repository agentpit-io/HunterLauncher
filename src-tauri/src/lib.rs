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
//! | [`selfupdate`] | §10 | 启动器自更新（Tauri updater + 两个端点 + 签名校验） |
//! | [`upgrade`] | §5.6、§10 | Hunter 版本检查与升级，失败自动回滚 |
//! | [`backup`] | §10 | 升级前的 `pg_dump` 与配置备份 |
//! | [`offline`] | §9 | 离线包导入 / 导出（`docker load` / `save`） |

use serde::Serialize;
use tauri::{Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

pub mod assist;
pub mod autostart;
pub mod backup;
pub mod commands;
pub mod compose;
pub mod config;
pub mod datacheck;
pub mod dockercfg;
pub mod err;
pub mod feedback;
pub mod flow;
pub mod gateway;
pub mod headless;
pub mod http;
pub mod log;
pub mod monitor;
pub mod netproxy;
pub mod offline;
pub mod paths;
pub mod ports;
pub mod proc;
pub mod redact;
pub mod registry;
pub mod runtime;
pub mod secretgen;
pub mod selfcheck;
pub mod selfupdate;
pub mod takeover;
pub mod telemetry;
pub mod timefmt;
pub mod tray;
pub mod upgrade;
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

    // updater：桌面三平台才有。端点与公钥在 tauri.conf.json 的 plugins.updater 里
    #[cfg(all(desktop, not(any(target_os = "android", target_os = "ios"))))]
    {
        builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
    }

    builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(flow::AppState::new())
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::window_minimize,
            commands::window_toggle_maximize,
            commands::window_close,
            commands::window_hide,
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
            commands::check_launcher_update,
            commands::install_launcher_update,
            commands::check_hunter_update,
            commands::upgrade_hunter,
            commands::upgrade_status,
            commands::list_backups,
            commands::create_backup,
            commands::restore_backup,
            commands::pick_offline_tar,
            commands::import_offline,
            commands::offline_ready,
            commands::assist_diagnose,
            commands::assist_ask,
            commands::assist_confirm,
            commands::assist_rule_action,
            commands::assist_reset,
            // I5 · AI 自动驾驶安装
            commands::assist_consent,
            commands::assist_auto_start,
            commands::assist_auto_answer,
            commands::assist_auto_snapshot,
            commands::assist_audit_tail,
            // I11 · 现状复查（错误页靠它自己看见「其实已经好了」）
            commands::self_check,
            // I12 · 装好了就记住：资源监控 · 启停补全 · 重装前检测已有数据
            commands::monitor_host,
            commands::monitor_runtime,
            commands::monitor_services,
            commands::monitor_storage,
            commands::data_check,
            commands::data_check_deep,
            commands::stack_plan,
            commands::stack_op,
            // I7 · 内置运行时 / 接管 / 一键反馈
            commands::builtin_runtime_status,
            commands::builtin_runtime_uninstall,
            commands::takeover_candidates,
            commands::takeover_state,
            commands::takeover_adopt,
            commands::takeover_release,
            commands::takeover_op,
            commands::takeover_confirm_text,
            commands::takeover_logs,
            commands::feedback_one_click,
            // I7 · 只允许本机访问（用户 2026-09-21 19:05 的决定）
            commands::tighten_web_bind,
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

            // 自更新：启动时 + 每 24 小时查一次（方案 §10）。
            // 查到新版本会发 `hunter://launcher-update` 事件并改托盘的 tooltip；
            // 装不装由用户在界面上决定，**绝不自动装**。
            crate::selfupdate::spawn_periodic(handle.clone());

            // **已经装好的机器也要能自愈**（I10 的 P0-3）。
            //
            // 0.1.9 在用户 Mac 上留下的状态是：虚拟机起着、容器起着、
            // 而虚拟机的 /etc/resolv.conf 是本地 Claude 手工改好的。
            // 0.1.10 一起来就该分得清「已经修好 / 还是断链」这两种情况：
            // 前者什么都不做（**不把人家手工改好的推倒重来**），
            // 后者自己修好，并让已经起着的容器重新拿一份。
            //
            // 三道闸门：用户授权过、内置运行时的虚拟机在跑、一个进程里只做一次。
            // 放在后台线程里 —— 这一步要起 `limactl shell` 子进程，不能卡住窗口。
            std::thread::spawn(|| {
                if !crate::config::LauncherConfig::load().assist.consented() {
                    return;
                }
                if crate::runtime::vmdns::applicable().is_err() {
                    return;
                }
                let mut say = |line: &str| linfo!("开机自查虚拟机 DNS：{line}");
                let mut nb = |_: u64, _: u64| {};
                let no_cancel = || false;
                let mut pr = crate::runtime::builtin::Progress {
                    say: &mut say,
                    bytes: &mut nb,
                    cancel: &no_cancel,
                };
                match crate::runtime::vmdns::ensure(&mut pr) {
                    Ok(d) if d.healthy() => linfo!("开机自查：{}", d.one_line()),
                    Ok(d) => lwarn!("开机自查：虚拟机的 DNS 还是不行 —— {}", d.one_line()),
                    Err(e) => lwarn!("开机自查虚拟机 DNS 没做成：{}", e.msg),
                }
            });

            // **端口有没有被谁开到了本机之外**（I11 · U5）。
            //
            // 2026-09-22 22:01 本地 Claude 在用户 Mac 上手工跑
            // `docker compose -p hunter up -d`，漏了启动器的覆盖文件，
            // 容器被重建成 0.0.0.0 —— 五个端口在局域网里可达了约 50 分钟。
            //
            // 启动器每次起来都对一遍：**跑着的容器**绑在哪，和磁盘上那份覆盖文件
            // 说的一不一样。不一样就按覆盖文件重建那几个容器。
            // 这不是替用户做新决定 —— 覆盖文件本来就写着 127.0.0.1，
            // 是现实跑偏了。老机器有意对外的那一档（WebBind::LegacyLan）不在此列。
            std::thread::spawn(|| {
                if !crate::config::LauncherConfig::load().install.done {
                    return;
                }
                let services = match compose::ps() {
                    Ok(v) => v,
                    Err(_) => return,
                };
                let drift = compose::bind_drift(&services);
                if drift.is_empty() {
                    return;
                }
                for d in &drift {
                    lwarn!("开机自查端口绑定：{}", d.human());
                }
                match compose::fix_bind_drift(&drift) {
                    Ok(()) => linfo!(
                        "开机自查：已按覆盖文件把 {} 的端口收回 127.0.0.1",
                        drift
                            .iter()
                            .map(|d| d.service.as_str())
                            .collect::<Vec<_>>()
                            .join("、")
                    ),
                    Err(e) => lwarn!("开机自查：端口没能收回本机 —— {}", e.msg),
                }
            });

            // **电脑或运行环境重启之后，容器要自己回来**（I12 · R3+）。
            //
            // 2026-09-23 在用户 Mac 上实测：内置运行时的虚拟机
            // `limactl stop` / `start` 之后，六个容器一个都没起来，`docker ps` 是空的。
            //
            // 覆盖文件里那条 `restart: unless-stopped` 管不到这一档 ——
            // 它的语义是「除非你手动停过」，而虚拟机停机时容器正是被「手动停」的那一路。
            // 所以这里补一道：**启动器一起来就看一眼**「配置齐了、运行环境在跑、
            // 可是本项目一个容器都没跑着」→ 自己 `up -d` 把它们拉回来，不用用户点。
            //
            // 四道闸门，缺一不可（宁可不做，也不要在不该做的时候动容器）：
            //   1. 这台机器上装过（`install.done` + 配置文件都在）；
            //   2. docker 现在连得上（运行环境确实在跑）；
            //   3. 本项目的容器**存在但一个都没跑着** —— 一个都没有（从没装过）
            //      或者已经有在跑的，都不该走这条路；
            //   4. 不是用户刚刚自己在界面上点的「停止」（`[install] stopped_by_user`）。
            std::thread::spawn(|| {
                let cfg = crate::config::LauncherConfig::load();
                if !cfg.install.done || !crate::selfcheck::installed_on_disk() {
                    return;
                }
                if cfg.install.stopped_by_user {
                    linfo!("开机自查容器：上一次是你自己在界面上点的「停止」，这次不替你起回来");
                    return;
                }
                if !crate::runtime::effective::current().running {
                    return;
                }
                let services = match compose::ps() {
                    Ok(v) => v,
                    Err(_) => return,
                };
                let ours: Vec<_> = services
                    .iter()
                    .filter(|s| crate::selfcheck::EXPECTED.contains(&s.service.as_str()))
                    .collect();
                if ours.is_empty() {
                    return;
                }
                if ours.iter().any(|s| s.state == "running") {
                    return;
                }
                // 「建出来过但一次都没跑起来」的残骸不在此列：它们身上冻着上一次那组端口，
                // `up` 起来照样撞车。那一档归 selfcheck 的 Incomplete 走完整流程（I11 §1.1）
                if ours.iter().all(|s| s.state == "created") {
                    lwarn!("开机自查容器：六个都是 created（上一次装到一半留下的），不替你起");
                    return;
                }
                lwarn!(
                    "开机自查容器：运行环境在跑，可是本项目 {} 个容器一个都没起来 —— 自动拉起",
                    ours.len()
                );
                match compose::up() {
                    Ok(()) => linfo!(
                        "开机自查容器：已经用两份 compose 文件把本项目拉回运行（不需要你点任何按钮）"
                    ),
                    Err(e) => lwarn!("开机自查容器：没能自动拉起 —— {}", e.msg),
                }
            });

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

/// `--tray-menu` 的三种下场。
///
/// 为什么要区分「没写这个参数」和「写了但认不出来」（I3 自审）：
/// 原来两种都返回 `None`，于是 `hunter-launcher --tray-menu stopp`（打错一个字母）
/// 会**照常开一个界面、退出码 0**，脚本里看起来像是成功了。
/// 拿不到结果就得说出来，不能让调用方以为做成了（红线 1 的同一条道理）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayArg {
    /// 命令行里没有 `--tray-menu`
    Absent,
    /// 认出来了，就是这一项
    Id(String),
    /// 写了 `--tray-menu`，但后面跟的东西不是托盘菜单里的 id（或者根本没跟）
    Unknown(String),
}

/// 从一串命令行参数里认出 `--tray-menu <id>`。
///
/// 只认**托盘菜单里真有的那些 id**（[`tray::MENU_IDS`]）—— 这个入口能让任何本机进程
/// 指使启动器停容器、开自启，不能让它变成一个「随便传个字符串进来看看会怎样」的口子。
pub fn tray_menu_arg_kind(argv: &[String]) -> TrayArg {
    let mut it = argv.iter();
    while let Some(a) = it.next() {
        let val = if a == "--tray-menu" {
            it.next().map(|s| s.as_str())
        } else {
            a.strip_prefix("--tray-menu=")
        };
        if let Some(v) = val {
            if tray::MENU_IDS.contains(&v) {
                return TrayArg::Id(v.to_string());
            }
            lwarn!("--tray-menu 收到一个不认识的菜单项：{v}");
            return TrayArg::Unknown(v.to_string());
        }
        // `--tray-menu` 是最后一个参数、后面什么都没跟
        if a == "--tray-menu" {
            lwarn!("--tray-menu 后面没有跟菜单项");
            return TrayArg::Unknown(String::new());
        }
    }
    TrayArg::Absent
}

pub fn tray_menu_arg(argv: &[String]) -> Option<String> {
    match tray_menu_arg_kind(argv) {
        TrayArg::Id(v) => Some(v),
        _ => None,
    }
}

/// 可用菜单项，给命令行的报错用。
pub fn tray_menu_ids() -> String {
    tray::MENU_IDS.join(" | ")
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

    /// I3 自审：「没写这个参数」和「写了但认不出来」必须分得开 ——
    /// 原来两种都是 `None`，于是打错一个字母的 `--tray-menu stopp` 会照常开一个界面、
    /// 退出码 0，脚本里看起来像是成功了，实际上什么都没做。
    #[test]
    fn 没写和写错要分得开() {
        assert_eq!(tray_menu_arg_kind(&[]), TrayArg::Absent);
        assert_eq!(tray_menu_arg_kind(&["--minimized".into()]), TrayArg::Absent);
        assert_eq!(
            tray_menu_arg_kind(&["--tray-menu".into(), "stop".into()]),
            TrayArg::Id("stop".into())
        );
        assert_eq!(
            tray_menu_arg_kind(&["--tray-menu".into(), "stopp".into()]),
            TrayArg::Unknown("stopp".into())
        );
        assert_eq!(
            tray_menu_arg_kind(&["--tray-menu=rm-rf".into()]),
            TrayArg::Unknown("rm-rf".into())
        );
        // `--tray-menu` 是最后一个参数、后面什么都没跟
        assert_eq!(
            tray_menu_arg_kind(&["--tray-menu".into()]),
            TrayArg::Unknown(String::new())
        );
    }

    #[test]
    fn 报错里列得出全部菜单项() {
        let s = tray_menu_ids();
        for id in tray::MENU_IDS {
            assert!(s.contains(id), "报错里少了 {id}：{s}");
        }
    }

    #[test]
    fn minimized_参数() {
        assert!(minimized_arg(&["--minimized".into()]));
        assert!(!minimized_arg(&["--headless".into()]));
    }
}
