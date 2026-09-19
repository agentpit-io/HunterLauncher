//! 托盘（技术方案 §5.8）。
//!
//! 菜单按方案原文逐项落地：
//! **打开 Hunter / 状态 / 启动 / 停止 / 重启 / 查看日志 / 检查更新 / 反馈 / 设置 / 退出**，
//! 外加一个方案同一节要求的「开机自启（默认关）」勾选项。
//!
//! ## 三条设计上的取舍
//!
//! 1. **托盘是增强，不是唯一入口**（M0 §7.3 的提醒）。菜单里每一项在界面上都另有入口：
//!    启动/停止/重启/日志/反馈在运行面板上，设置在标题栏齿轮里。
//!    没有桌面环境（Xvfb、纯 SSH）时托盘建不出来，程序照常能用。
//! 2. **状态项不可点**，它就是一行字：`运行中 · 6/6 健康` / `已停止` / `启动中…`。
//!    后台每 5 秒问一次 `docker compose ps` 刷新它，同时刷新托盘的 tooltip。
//!    刷新用的是和运行面板同一套 `compose::ps()`，不另开一套判定。
//! 3. **退出要问一句**。方案原文：「退出时若容器运行中，询问『保持后台运行』或『一起停止』」。
//!    这个询问做成**界面里的弹窗**而不是系统对话框：一来风格和其余页面一致，
//!    二来 Xvfb 下截得了图、点得动，验收时能真验（系统对话框在无桌面环境里没法自动点）。
//!    托盘点「退出」时先把主窗口叫出来再弹这个框。
//!
//! ## Xvfb 下的已知限制
//!
//! M0 §7.3 实测：没有真正的状态栏时托盘图标**看不见**，只能验证
//! `TrayIconBuilder::build()` 返回 `Ok`。菜单项的真实行为靠把每一项的处理函数
//! 抽成 [`handle_menu`] 这个**不依赖托盘的纯函数**来验 —— 测试直接调它，
//! 和用户点菜单走的是同一条代码路径。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::compose;
use crate::flow::AppState;

/// 菜单项 id。前端与测试都按这些字符串对。
pub const ID_OPEN: &str = "open";
pub const ID_STATUS: &str = "status";
pub const ID_START: &str = "start";
pub const ID_STOP: &str = "stop";
pub const ID_RESTART: &str = "restart";
pub const ID_LOGS: &str = "logs";
pub const ID_UPDATE: &str = "update";
pub const ID_FEEDBACK: &str = "feedback";
pub const ID_SETTINGS: &str = "settings";
pub const ID_AUTOSTART: &str = "autostart";
pub const ID_QUIT: &str = "quit";

/// 托盘菜单的全部 id，顺序即显示顺序。
/// 有了它，「菜单每一项都点一遍」这种验收才有一份机器可读的清单。
pub const MENU_IDS: [&str; 11] = [
    ID_OPEN,
    ID_STATUS,
    ID_START,
    ID_STOP,
    ID_RESTART,
    ID_LOGS,
    ID_UPDATE,
    ID_FEEDBACK,
    ID_SETTINGS,
    ID_AUTOSTART,
    ID_QUIT,
];

/// 前端监听的事件名。托盘只负责「把人带到那一页」，具体的界面逻辑在前端。
pub const EV_NAVIGATE: &str = "hunter://navigate";
/// 退出询问：前端收到就弹「保持后台运行 / 一起停止」。
pub const EV_QUIT_REQUEST: &str = "hunter://quit-request";
/// 托盘触发的栈操作有了结果，让运行面板刷新一次。
pub const EV_TRAY_ACTION: &str = "hunter://tray-action";

/// 托盘状态文字。抽成纯函数，好测。
///
/// * 六个服务全健康 → `运行中 · 6/6 健康`
/// * 有容器在跑但没全健康 → `启动中 · 3/6 健康`
/// * 一个都没跑 → `已停止`
/// * `ps` 本身失败 → `状态未知 · <原因>`（**不猜**，红线 1）
pub fn status_text(services: &[compose::ServiceStatus], err: Option<&str>) -> String {
    if let Some(e) = err {
        return format!("状态未知 · {e}");
    }
    if services.is_empty() {
        return "已停止".to_string();
    }
    let running = services.iter().filter(|s| s.state == "running").count();
    if running == 0 {
        return "已停止".to_string();
    }
    let healthy = services
        .iter()
        .filter(|s| s.health == compose::Health::Healthy)
        .count();
    if healthy == services.len() {
        format!("运行中 · {healthy}/{} 健康", services.len())
    } else {
        format!("启动中 · {healthy}/{} 健康", services.len())
    }
}

/// 托盘 tooltip。鼠标悬停时看到的一行。
pub fn tooltip(status: &str, web_url: Option<&str>) -> String {
    tooltip_with_update(status, web_url, None)
}

/// 带「启动器有新版本」提示的 tooltip（方案 §10：「有更新时托盘提示」）。
pub fn tooltip_with_update(
    status: &str,
    web_url: Option<&str>,
    launcher_update: Option<&str>,
) -> String {
    let mut s = format!("Hunter 启动器 · {status}");
    if let Some(u) = web_url {
        s.push('\n');
        s.push_str(u);
    }
    if let Some(v) = launcher_update {
        s.push_str(&format!("\n启动器有新版本 v{v}，点开界面可以更新"));
    }
    s
}

/// 后台查到的「启动器有新版本」。托盘的 tooltip 与菜单项文字都看它。
fn pending_launcher_update() -> &'static std::sync::Mutex<Option<String>> {
    static P: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
    &P
}

pub fn launcher_update_pending() -> Option<String> {
    pending_launcher_update()
        .lock()
        .ok()
        .and_then(|g| g.clone())
}

/// [`crate::selfupdate`] 的后台检查查到新版本时叫这个：
/// 记下来、把菜单项改成「启动器有新版本 v0.1.1」、刷新 tooltip。
pub fn note_launcher_update<R: Runtime>(app: &AppHandle<R>, version: Option<&str>) {
    if let Ok(mut g) = pending_launcher_update().lock() {
        *g = version.map(str::to_string);
    }
    if let Some(h) = app.try_state::<TrayHandles<R>>() {
        let text = match version {
            Some(v) => format!("检查更新（启动器有新版本 v{v}）"),
            None => "检查更新".to_string(),
        };
        let _ = h.update_item.set_text(text);
    }
    refresh_status(app);
}

/// 点一个菜单项以后该干什么。**不碰托盘、不碰窗口**，只回一个指令 ——
/// 这样测试能直接调它，走的是和真实点击完全一样的分支。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// 用浏览器打开 Hunter
    OpenHunter,
    /// 跳到界面的某一页（overlay 名或 `dashboard`）
    Navigate(&'static str),
    /// 对容器做一次操作（`start` / `stop` / `restart`）
    Stack(&'static str),
    /// 查一次 Hunter 有没有新版本
    CheckUpdate,
    /// 切开机自启
    ToggleAutostart,
    /// 请求退出（要不要停容器由界面上的弹窗决定）
    RequestQuit,
    /// 什么都不做（状态项）
    Nothing,
}

pub fn handle_menu(id: &str) -> Action {
    match id {
        ID_OPEN => Action::OpenHunter,
        ID_START => Action::Stack("start"),
        ID_STOP => Action::Stack("stop"),
        ID_RESTART => Action::Stack("restart"),
        ID_LOGS => Action::Navigate("logs"),
        ID_FEEDBACK => Action::Navigate("feedback"),
        ID_SETTINGS => Action::Navigate("settings"),
        ID_UPDATE => Action::CheckUpdate,
        ID_AUTOSTART => Action::ToggleAutostart,
        ID_QUIT => Action::RequestQuit,
        ID_STATUS => Action::Nothing,
        _ => Action::Nothing,
    }
}

/// 托盘建好之后留下的把手，后台线程靠它刷新状态行。
pub struct TrayHandles<R: Runtime> {
    pub status_item: MenuItem<R>,
    pub autostart_item: CheckMenuItem<R>,
    /// 「检查更新」那一项。查到启动器有新版本时它的文字会变（方案 §10 的「托盘提示」）
    pub update_item: MenuItem<R>,
    pub tray_id: tauri::tray::TrayIconId,
}

/// 建托盘。**失败不致命** —— 没有状态栏的环境（Xvfb、精简桌面）建不出来是正常的，
/// 记一条 warn 然后继续，程序照常能用。
pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, ID_OPEN, "打开 Hunter", true, None::<&str>)?;
    let status = MenuItem::with_id(app, ID_STATUS, "状态：读取中…", false, None::<&str>)?;
    let start = MenuItem::with_id(app, ID_START, "启动", true, None::<&str>)?;
    let stop = MenuItem::with_id(app, ID_STOP, "停止", true, None::<&str>)?;
    let restart = MenuItem::with_id(app, ID_RESTART, "重启", true, None::<&str>)?;
    let logs = MenuItem::with_id(app, ID_LOGS, "查看日志", true, None::<&str>)?;
    let update = MenuItem::with_id(app, ID_UPDATE, "检查更新", true, None::<&str>)?;
    let feedback = MenuItem::with_id(app, ID_FEEDBACK, "反馈", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, ID_SETTINGS, "设置", true, None::<&str>)?;
    let autostart = CheckMenuItem::with_id(
        app,
        ID_AUTOSTART,
        "开机自启",
        true,
        crate::autostart::status(), // 读系统里真实的状态，不读配置文件
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, ID_QUIT, "退出", true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let sep3 = PredefinedMenuItem::separator(app)?;

    let menu = Menu::with_items(
        app,
        &[
            &open, &status, &sep1, &start, &stop, &restart, &sep2, &logs, &update, &feedback,
            &settings, &autostart, &sep3, &quit,
        ],
    )?;

    let mut builder = TrayIconBuilder::with_id("hunter-tray")
        .menu(&menu)
        .tooltip(tooltip("读取中…", None))
        // 左键点图标直接把窗口叫出来（Windows / Linux 的习惯）
        .show_menu_on_left_click(false);
    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }
    let tray = builder
        .on_menu_event(|app: &AppHandle<R>, event| {
            let id = event.id().as_ref().to_string();
            let handle = app.clone();
            // 菜单事件在 UI 线程上，栈操作要几十秒，必须挪走
            std::thread::spawn(move || run_action(&handle, &id));
        })
        .on_tray_icon_event(|tray, event| {
            if let tauri::tray::TrayIconEvent::Click {
                button: tauri::tray::MouseButton::Left,
                button_state: tauri::tray::MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;

    app.manage(TrayHandles {
        status_item: status,
        autostart_item: autostart,
        update_item: update,
        tray_id: tray.id().clone(),
    });
    spawn_status_poller(app.clone());
    Ok(())
}

/// 真正执行一个菜单动作。GUI 侧的入口，内部全部走 [`handle_menu`] 定下来的分支。
pub fn run_action<R: Runtime>(app: &AppHandle<R>, id: &str) {
    match handle_menu(id) {
        Action::Nothing => {}
        Action::OpenHunter => {
            let st = app.state::<AppState>();
            let s = crate::flow::runtime_status(&st);
            match s.web_url {
                Some(u) => {
                    if let Err(e) = crate::commands::open_url_checked(app, &u) {
                        crate::lwarn!("托盘打开 Hunter 失败：{e}");
                    }
                }
                None => {
                    crate::lwarn!("托盘：Hunter 还没在运行，打不开");
                    show_main(app);
                    let _ = app.emit(EV_NAVIGATE, "dashboard");
                }
            }
        }
        Action::Navigate(page) => {
            show_main(app);
            let _ = app.emit(EV_NAVIGATE, page);
        }
        Action::Stack(action) => {
            crate::linfo!("托盘：{action}");
            let r = match action {
                "start" => compose::up().map(|_| "已启动".to_string()),
                "stop" => compose::stop().map(|_| "已停止".to_string()),
                _ => compose::restart().map(|_| "已重启".to_string()),
            };
            let msg = match r {
                Ok(m) => m,
                Err(e) => {
                    crate::lerror!("托盘 {action} 失败：{}", e.msg);
                    e.to_string()
                }
            };
            let _ = app.emit(EV_TRAY_ACTION, msg);
            refresh_status(app);
        }
        Action::CheckUpdate => {
            show_main(app);
            let _ = app.emit(EV_NAVIGATE, "dashboard");
            // 「检查更新」要一次把**两件事**都查了：Hunter 有没有新版本、启动器自己有没有。
            // 用户点它的时候心里想的是「有没有什么要更新的」，分成两个入口只会让人漏掉一个。
            //
            // 用户亲手点的这一下要绕过 6 小时缓存 —— 他点它正是想知道「现在」怎么样
            let latest = crate::flow::latest_hunter_tag_now();
            let cur = app.state::<AppState>().config().hunter.tag;
            let hunter = match latest {
                Some(t) if crate::upgrade::is_newer(&t, &cur) => {
                    format!("Hunter 有新版本 v{t}（当前 v{cur}）")
                }
                Some(t) => format!("Hunter 已经是最新版 v{t}"),
                None => "查不到 Hunter 的最新版本（GitHub 接口没返回）".to_string(),
            };
            let launcher = check_launcher_update_blocking(app);
            let msg = format!("{hunter}；{launcher}");
            crate::linfo!("托盘检查更新：{msg}");
            let _ = app.emit(EV_TRAY_ACTION, msg);
        }
        Action::ToggleAutostart => {
            let now = crate::autostart::status();
            let msg = match crate::autostart::set(!now) {
                Ok(m) => m,
                Err(e) => e.to_string(),
            };
            crate::linfo!("托盘切开机自启：{msg}");
            // 把配置文件里的记录也对齐（设置页读的是同一个值）
            let st = app.state::<AppState>();
            let mut cfg = st.config();
            cfg.launcher.autostart = crate::autostart::status();
            let _ = cfg.save();
            st.set_config(cfg);
            if let Some(h) = app.try_state::<TrayHandles<R>>() {
                let _ = h.autostart_item.set_checked(crate::autostart::status());
            }
            let _ = app.emit(EV_TRAY_ACTION, msg);
        }
        Action::RequestQuit => {
            // 容器还在跑就把窗口叫出来问一句；没在跑就直接退
            let running = compose::is_up();
            crate::linfo!("托盘退出：容器在运行={running}");
            if running {
                show_main(app);
                let _ = app.emit(EV_QUIT_REQUEST, true);
            } else {
                app.exit(0);
            }
        }
    }
}

/// 托盘里同步地查一次「启动器自己有没有新版本」。
///
/// [`crate::selfupdate::check`] 是 async 的（Tauri 的 updater 就是 async）。
/// 这里在托盘的工作线程上用 `block_on` 等它 —— 这条线程是 `run_action` 自己起的普通线程，
/// 不是运行时的线程，阻塞它不会死锁。
fn check_launcher_update_blocking<R: Runtime>(app: &AppHandle<R>) -> String {
    let u = tauri::async_runtime::block_on(crate::selfupdate::check(app));
    note_launcher_update(app, u.version.as_deref());
    match (&u.version, &u.reason) {
        (Some(v), _) => format!("启动器有新版本 v{v}（当前 v{}）", u.current),
        (None, Some(r)) => format!("查不到启动器的最新版本：{r}"),
        (None, None) => format!("启动器已经是最新版 v{}", u.current),
    }
}

fn show_main<R: Runtime>(app: &AppHandle<R>) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// 立刻刷一次状态行与 tooltip。
pub fn refresh_status<R: Runtime>(app: &AppHandle<R>) {
    let (services, err) = match compose::ps() {
        Ok(v) => (v, None),
        Err(e) => (Vec::new(), Some(e.msg)),
    };
    let text = status_text(&services, err.as_deref());
    let url = services
        .iter()
        .find(|s| s.service == "web" && s.state == "running")
        .and_then(|s| s.port)
        .map(|p| format!("http://localhost:{p}"));
    let pending = launcher_update_pending();
    if let Some(h) = app.try_state::<TrayHandles<R>>() {
        let _ = h.status_item.set_text(format!("状态：{text}"));
        if let Some(tray) = app.tray_by_id(&h.tray_id) {
            let _ = tray.set_tooltip(Some(tooltip_with_update(
                &text,
                url.as_deref(),
                pending.as_deref(),
            )));
        }
    }
}

/// 托盘状态的轮询间隔。
///
/// 每次刷新是一个 `docker compose ps` 子进程：测试机上实测 **0.17 秒墙钟、约 60% 单核**
/// （≈0.1 CPU 秒）。5 秒一次就是持续占掉一个核的 2%，对一个常驻托盘的程序来说太多了；
/// 运行面板自己还会每 10 秒刷一次。15 秒对「托盘上那行状态字」足够新鲜，
/// 只开着托盘时的开销降到约 0.7%。
const POLL_INTERVAL: Duration = Duration::from_secs(15);

/// 后台定期刷新状态（方案 §5.8「状态随服务变化」）。
fn spawn_status_poller<R: Runtime>(app: AppHandle<R>) {
    static RUNNING: AtomicBool = AtomicBool::new(false);
    if RUNNING.swap(true, Ordering::SeqCst) {
        return; // 只起一条
    }
    let stop = Arc::new(AtomicBool::new(false));
    std::thread::spawn(move || {
        while !stop.load(Ordering::SeqCst) {
            refresh_status(&app);
            std::thread::sleep(POLL_INTERVAL);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::{Health, ServiceStatus};

    fn svc(name: &str, state: &str, health: Health) -> ServiceStatus {
        ServiceStatus {
            service: name.into(),
            state: state.into(),
            health,
            port: if name == "web" { Some(3101) } else { None },
            exit_code: None,
        }
    }

    #[test]
    fn 菜单项覆盖方案_5_8_列出的每一项() {
        // 方案原文：打开 Hunter / 状态 / 启动 / 停止 / 重启 / 查看日志 / 检查更新 / 反馈 / 设置 / 退出
        for id in [
            ID_OPEN,
            ID_STATUS,
            ID_START,
            ID_STOP,
            ID_RESTART,
            ID_LOGS,
            ID_UPDATE,
            ID_FEEDBACK,
            ID_SETTINGS,
            ID_QUIT,
        ] {
            assert!(MENU_IDS.contains(&id), "菜单里少了 {id}");
        }
        // 外加方案同一节要求的开机自启
        assert!(MENU_IDS.contains(&ID_AUTOSTART));
        assert_eq!(MENU_IDS.len(), 11);
    }

    #[test]
    fn 每一项都映射到一个明确的动作() {
        assert_eq!(handle_menu(ID_OPEN), Action::OpenHunter);
        assert_eq!(handle_menu(ID_START), Action::Stack("start"));
        assert_eq!(handle_menu(ID_STOP), Action::Stack("stop"));
        assert_eq!(handle_menu(ID_RESTART), Action::Stack("restart"));
        assert_eq!(handle_menu(ID_LOGS), Action::Navigate("logs"));
        assert_eq!(handle_menu(ID_FEEDBACK), Action::Navigate("feedback"));
        assert_eq!(handle_menu(ID_SETTINGS), Action::Navigate("settings"));
        assert_eq!(handle_menu(ID_UPDATE), Action::CheckUpdate);
        assert_eq!(handle_menu(ID_AUTOSTART), Action::ToggleAutostart);
        assert_eq!(handle_menu(ID_QUIT), Action::RequestQuit);
        assert_eq!(handle_menu(ID_STATUS), Action::Nothing);
        // 没有一项是「还没做」
        for id in MENU_IDS {
            let a = handle_menu(id);
            assert!(
                a != Action::Nothing || id == ID_STATUS,
                "{id} 没有对应的动作"
            );
        }
    }

    #[test]
    fn 状态文字随服务变化() {
        assert_eq!(status_text(&[], None), "已停止");

        let all_healthy: Vec<_> = ["web", "api", "opencode", "llm-shim", "postgres", "redis"]
            .iter()
            .map(|n| svc(n, "running", Health::Healthy))
            .collect();
        assert_eq!(status_text(&all_healthy, None), "运行中 · 6/6 健康");

        let mut starting = all_healthy.clone();
        starting[3].health = Health::Starting;
        starting[4].health = Health::Starting;
        starting[5].health = Health::Starting;
        assert_eq!(status_text(&starting, None), "启动中 · 3/6 健康");

        let exited: Vec<_> = all_healthy
            .iter()
            .cloned()
            .map(|mut s| {
                s.state = "exited".into();
                s
            })
            .collect();
        assert_eq!(status_text(&exited, None), "已停止");
    }

    #[test]
    fn ps_失败时如实说未知而不是猜已停止() {
        let t = status_text(&[], Some("Cannot connect to the Docker daemon"));
        assert!(t.starts_with("状态未知"), "{t}");
        assert!(t.contains("Docker daemon"), "原因要带上：{t}");
    }

    #[test]
    fn tooltip_带地址() {
        assert_eq!(
            tooltip("运行中 · 6/6 健康", Some("http://localhost:3101")),
            "Hunter 启动器 · 运行中 · 6/6 健康\nhttp://localhost:3101"
        );
        assert_eq!(tooltip("已停止", None), "Hunter 启动器 · 已停止");
    }

    /// 方案 §10：「有更新时托盘提示」。
    #[test]
    fn 启动器有新版本时_tooltip_会说一句() {
        let t = tooltip_with_update(
            "运行中 · 6/6 健康",
            Some("http://localhost:3101"),
            Some("0.1.1"),
        );
        assert!(t.contains("启动器有新版本 v0.1.1"), "{t}");
        // 没有新版本时不能凭空多一行
        let t0 = tooltip_with_update("已停止", None, None);
        assert_eq!(t0, "Hunter 启动器 · 已停止");
    }
}
