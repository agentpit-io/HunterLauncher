//! 「这台电脑上没有 Docker」的**兜底链**（I8 · 任务书一.3）。
//!
//! ## 之前是什么样
//!
//! I7 只有一条路：内置运行时（Colima）。它失败了就发一张「需要你」卡片：
//!
//! > 请在终端里执行：brew install --cask orbstack（或者去 orbstack.dev 下载）
//!
//! 0.1.7 在用户 Mac 上就停在这张卡片上。用户的原则很清楚：
//! **所有能替他做的事都不要让他自己做**。所以这一轮把「一条路 + 一张卡片」
//! 换成「三条路依次自动走完」，每一条都是启动器自己执行：
//!
//! | 顺序 | 路线 | 要下多少 | 要密码吗 |
//! |---|---|---|---|
//! | ① | **内置运行时**（Colima + Lima + docker CLI + compose + 虚拟机镜像） | 约 436 MB | 不要 |
//! | ② | **OrbStack 官方安装包**（下 dmg → 验苹果签名与公证 → 装 → 打开 → 等就绪） | 约 200 MB | 装到「应用程序」时可能要 |
//! | ③ | **Homebrew**（没有就先替你装 brew）→ `brew install --cask orbstack` | 视情况 | 会要（brew 要写系统目录） |
//!
//! 前一条失败了才走下一条，**每一条开始前都在界面上说一句人话 + 预计下载量**，
//! 要密码的那一步先说清楚为什么要。
//!
//! ## 什么算「这条路成功了」
//!
//! 只有一个判据：**`docker version` 真的连得上后台服务**。
//! 装好了、打开了、进程在跑 —— 都不算。这是 I5 起就定下的规矩（验证员不问模型
//! 「好了吗」，而是重跑那一步），这里同样适用。

use std::time::{Duration, Instant};

use crate::err::{AppError, AppResult, Code};

use super::builtin;

/// 一条路线。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// 内置运行时（默认，全程用户态）
    Builtin,
    /// OrbStack 官方 dmg
    OrbStack,
    /// Homebrew（没有就先装 brew）
    Homebrew,
}

impl Route {
    pub fn as_str(self) -> &'static str {
        match self {
            Route::Builtin => "builtin",
            Route::OrbStack => "orbstack",
            Route::Homebrew => "homebrew",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Route::Builtin => "内置运行时",
            Route::OrbStack => "OrbStack 官方安装包",
            Route::Homebrew => "Homebrew",
        }
    }
    /// 这条路线开始前，界面上那一句人话（**含预计下载量，数字来自清单**）。
    pub fn intro(self) -> String {
        match self {
            Route::Builtin => {
                let items = super::manifest::for_host();
                let missing = builtin::missing_items();
                let bytes = super::manifest::total_bytes(&missing);
                if items.is_empty() {
                    "正在装内置的容器运行时".to_string()
                } else if bytes == 0 {
                    "内置运行时要的文件都已经下好了，直接起虚拟机".to_string()
                } else {
                    format!(
                        "先试第 1 条路：装一套内置的容器运行时（要下 {}，装在 Hunter 自己的文件夹里，不用密码）",
                        crate::assist::probe::human_bytes(bytes)
                    )
                }
            }
            Route::OrbStack => "内置运行时这条路没走通，改试第 2 条：从官方地址下 OrbStack 安装包\
                 （约 200 MB，会先验苹果的签名与公证），装好之后由启动器替你打开"
                .to_string(),
            Route::Homebrew => "前两条路都没走通，改试第 3 条：用 Homebrew 装 OrbStack。\
                 这台电脑上没有 Homebrew 的话，启动器会先替你把它装上 —— \
                 那一步要往系统目录里写文件，macOS 会弹出它自己的密码框，\
                 输一次开机密码就行（密码交给系统，启动器看不到也不会保存）"
                .to_string(),
        }
    }
}

/// 这台机器上按顺序要试哪几条路。
///
/// 用户在设置里把 `install_route` 选成 `orbstack` 时，把 OrbStack 提到最前面 ——
/// **顺序变，链条不变**：每一条仍然会依次试下去。
pub fn routes() -> Vec<Route> {
    if !cfg!(target_os = "macos") {
        // Linux / Windows 上装 Docker 都要管理员权限装系统级服务，
        // 这一轮没有做那条链路 —— **如实返回空**，不假装有办法
        return Vec::new();
    }
    let prefer_orbstack = crate::config::LauncherConfig::load().runtime.route()
        == crate::config::InstallRoute::OrbStack;
    if prefer_orbstack {
        vec![Route::OrbStack, Route::Builtin, Route::Homebrew]
    } else {
        vec![Route::Builtin, Route::OrbStack, Route::Homebrew]
    }
}

/// 等 docker 后台服务真的起来。**这是每条路线唯一的成功判据。**
pub fn wait_daemon(max: Duration, say: &mut dyn FnMut(&str)) -> bool {
    let t0 = Instant::now();
    let mut said = false;
    loop {
        super::which::invalidate();
        super::env::invalidate();
        if super::docker::detect().daemon_running {
            return true;
        }
        if t0.elapsed() >= max {
            return false;
        }
        if !said && t0.elapsed() > Duration::from_secs(10) {
            said = true;
            say("已经装好了，正在等 Docker 后台服务就绪（通常十几秒到一分钟）");
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

/// 跑完一条路线（装 + 起 + 等就绪）。
fn run_route(route: Route, p: &mut builtin::Progress) -> AppResult<String> {
    match route {
        Route::Builtin => {
            if builtin::needs_download() {
                builtin::install(p)?;
            }
            let started = builtin::start(p)?;
            // 装好之后把 docker 路径钉进设置：下次开启动器不用再探一遍
            pin_docker_path();
            if !wait_daemon(Duration::from_secs(180), p.say) {
                return Err(AppError::new(
                    Code::DaemonDown,
                    "虚拟机起来了，但 180 秒内 docker 还是连不上它的后台服务。".to_string(),
                ));
            }
            Ok(started)
        }
        Route::OrbStack => {
            let msg = super::orbstack::install(p.say, p.cancel)?;
            (p.say)(&msg);
            // I8：**装完由启动器自己打开它**，不再把这一步丢给用户
            (p.say)(super::orbstack::FIRST_RUN_NOTE);
            super::orbstack::open_app(p.say)?;
            pin_docker_path();
            if !wait_daemon(Duration::from_secs(300), p.say) {
                return Err(AppError::new(
                    Code::DaemonDown,
                    "OrbStack 已经装好并打开了，但 5 分钟内 docker 还是连不上后台服务\
                     （它自己的窗口可能还等着你点一下默认选项）。"
                        .to_string(),
                ));
            }
            Ok("OrbStack 已经装好、打开，docker 可以用了".to_string())
        }
        Route::Homebrew => {
            let brewed = super::brew::install_homebrew(p.say, p.cancel)?;
            (p.say)(&brewed);
            let msg = super::brew::install_orbstack_cask(p.say)?;
            (p.say)(&msg);
            (p.say)(super::orbstack::FIRST_RUN_NOTE);
            super::orbstack::open_app(p.say)?;
            pin_docker_path();
            if !wait_daemon(Duration::from_secs(300), p.say) {
                return Err(AppError::new(
                    Code::DaemonDown,
                    "OrbStack 已经用 Homebrew 装好并打开了，但 5 分钟内 docker 还是连不上后台服务。"
                        .to_string(),
                ));
            }
            Ok("已经用 Homebrew 装好 OrbStack，docker 可以用了".to_string())
        }
    }
}

/// 把探到的 docker 路径写进设置（成功之后做，失败了不写）。
fn pin_docker_path() {
    super::which::invalidate();
    super::env::invalidate();
    let found = builtin::docker_bin()
        .map(|p| p.to_string_lossy().into_owned())
        .or_else(|| super::which::docker_probe().resolved);
    let Some(path) = found else { return };
    let mut cfg = crate::config::LauncherConfig::load();
    if cfg.runtime.docker_path == path {
        return;
    }
    cfg.runtime.docker_path = path;
    if let Err(e) = cfg.save() {
        crate::lwarn!("把 docker 路径写进设置失败：{}", e.msg);
    }
    super::which::invalidate();
    super::env::invalidate();
}

/// 一次兜底链的结果。
pub struct Outcome {
    /// 最后是哪条路成的
    pub route: Route,
    /// 给人看的那一句
    pub message: String,
    /// 前面失败过的路线与原话（**如实留着**，报告与诊断包要用）
    pub failed: Vec<(Route, String)>,
}

/// 替用户把 Docker 装好并起来。**一条路失败就自动走下一条，中间不问人。**
pub fn install_docker(p: &mut builtin::Progress) -> AppResult<Outcome> {
    // ① 先问本机已有的：有在跑的就一个字节都不下
    match builtin::decide() {
        builtin::Decision::AlreadyRunning(who) => {
            return Ok(Outcome {
                route: Route::Builtin,
                message: format!("{who} 正在运行，一个字节都不用下。"),
                failed: Vec::new(),
            })
        }
        builtin::Decision::StartExisting(app) => {
            // 装了没起：先把它点起来，这比再装一套快得多
            (p.say)(&format!(
                "这台电脑上已经装了 {}，只是没启动 —— 先替你把它打开",
                crate::assist::actions::runtime_label(&app)
            ));
            match crate::assist::actions::execute_as(
                &crate::assist::actions::Call::with("start_runtime", "app", &app),
                crate::assist::guard::Mode::Auto,
                true,
                crate::assist::guard::Proposer::Orchestrator,
            ) {
                Ok(_) => {
                    if wait_daemon(Duration::from_secs(180), p.say) {
                        return Ok(Outcome {
                            route: Route::Builtin,
                            message: format!(
                                "{} 已经打开，docker 可以用了",
                                crate::assist::actions::runtime_label(&app)
                            ),
                            failed: Vec::new(),
                        });
                    }
                    (p.say)("它没能在 3 分钟内就绪，改走装一套新的那条路");
                }
                Err(e) => (p.say)(&format!("打开它没成功（{}），改走装一套新的那条路", e.msg)),
            }
        }
        _ => {}
    }

    let list = routes();
    if list.is_empty() {
        return Err(AppError::new(
            Code::NotImplemented,
            platform_cannot_install(),
        ));
    }
    let mut failed: Vec<(Route, String)> = Vec::new();
    for route in list {
        if (p.cancel)() {
            return Err(AppError::new(Code::Unknown, "安装被取消了。".to_string()));
        }
        (p.say)(&route.intro());
        crate::assist::guard::audit(
            "install_runtime_route",
            &std::collections::BTreeMap::from([("route".to_string(), route.as_str().to_string())]),
            crate::assist::guard::Proposer::Orchestrator,
            Some(crate::assist::guard::Level::Sensitive),
            "开始这条路线",
        );
        match run_route(route, p) {
            Ok(message) => {
                crate::assist::guard::audit(
                    "install_runtime_route",
                    &std::collections::BTreeMap::from([(
                        "route".to_string(),
                        route.as_str().to_string(),
                    )]),
                    crate::assist::guard::Proposer::Orchestrator,
                    Some(crate::assist::guard::Level::Sensitive),
                    &format!("成功：{message}"),
                );
                return Ok(Outcome {
                    route,
                    message,
                    failed,
                });
            }
            Err(e) => {
                crate::lwarn!("{} 这条路没走通：{}", route.label(), e.msg);
                (p.say)(&format!("{} 这条路没走通：{}", route.label(), e.msg));
                crate::assist::guard::audit(
                    "install_runtime_route",
                    &std::collections::BTreeMap::from([(
                        "route".to_string(),
                        route.as_str().to_string(),
                    )]),
                    crate::assist::guard::Proposer::Orchestrator,
                    Some(crate::assist::guard::Level::Sensitive),
                    &format!("失败：{}", e.msg),
                );
                failed.push((route, e.msg));
            }
        }
    }
    // 三条路都没走通。**把每一条的原话都带上** —— 这份文字会进诊断包，
    // 而诊断包是用户最后那条出路
    let detail = failed
        .iter()
        .map(|(r, e)| format!("{}：{e}", r.label()))
        .collect::<Vec<_>>()
        .join("\n");
    Err(AppError::new(
        Code::DockerMissing,
        format!("三条路都试过了，都没能把 Docker 装起来：\n{detail}"),
    ))
}

/// 这个平台上启动器还不能替用户装 Docker 时，那一句**如实**的说明。
pub fn platform_cannot_install() -> String {
    if cfg!(target_os = "linux") {
        "这台机器是 Linux：装 Docker 要改系统服务、要管理员权限，\
         启动器这一版还没有做这条链路。你可以先用系统自带的包管理器装好 Docker，\
         再回来点一次「重新检测」；启动器会自己接着往下装。"
            .to_string()
    } else if cfg!(target_os = "windows") {
        "这台机器是 Windows：装 Docker Desktop 要 WSL 与管理员权限，\
         启动器这一版还没有做这条链路。装好 Docker Desktop 之后回来点一次「重新检测」，\
         启动器会自己接着往下装。"
            .to_string()
    } else {
        "这个平台上启动器还不能替你装 Docker。".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 顺序就是任务书里写的那个顺序。
    #[test]
    fn 三条路的顺序() {
        if cfg!(target_os = "macos") {
            let r = routes();
            assert_eq!(r.len(), 3, "{r:?}");
            assert_eq!(r[0], Route::Builtin);
            assert_eq!(r[1], Route::OrbStack);
            assert_eq!(r[2], Route::Homebrew);
        } else {
            assert!(routes().is_empty(), "别的平台上如实返回空");
        }
    }

    /// 每条路线开始前那句话里**必须有「要下多少」或者「要不要密码」**，
    /// 不能只说「正在安装」。
    #[test]
    fn 每条路线都先说人话() {
        for r in [Route::Builtin, Route::OrbStack, Route::Homebrew] {
            let s = r.intro();
            assert!(!s.is_empty());
            assert!(
                s.contains("MB")
                    || s.contains("下好了")
                    || s.contains("密码")
                    || s.contains("正在装"),
                "{}：{s}",
                r.label()
            );
        }
        // 要密码那条必须**先说清楚为什么**
        let h = Route::Homebrew.intro();
        assert!(h.contains("密码框"), "{h}");
        assert!(h.contains("启动器看不到也不会保存"), "{h}");
    }

    /// 平台不支持时那句话里**不许**出现「请你在终端里执行」这类引导
    /// （I8 第〇节：界面上永远不出现这种话）。
    #[test]
    fn 平台不支持时不叫用户去敲命令() {
        let s = platform_cannot_install();
        for bad in ["终端", "命令行", "复制", "sudo", "apt-get", "curl"] {
            assert!(!s.contains(bad), "这句话里不该出现「{bad}」：{s}");
        }
    }
}
