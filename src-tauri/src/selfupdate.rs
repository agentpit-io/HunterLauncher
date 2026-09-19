//! 启动器自更新（技术方案 §10「启动器自更新」）。
//!
//! > Release 时 CI 生成 `latest.json` 与各平台安装包及签名；
//! > 启动器每次启动 + 每 24 小时检查；有更新时托盘提示，用户点击下载安装，重启生效。
//!
//! 端点两个，按顺序试（`plan/国内镜像与下载源.md` 的要求：COS 在前、GitHub 在后）：
//!
//! ```text
//! https://hunter-dl-hk-1253756459.cos.ap-hongkong.myqcloud.com/launcher/latest.json
//! https://github.com/agentpit-io/HunterLauncher/releases/latest/download/latest.json
//! ```
//!
//! ⚠ **GitHub 那个端点现在是死的，而且是有原因的**：`releases/latest/download/…` 解析的是
//! 「最新的**正式** Release」，而本项目的 Release 因为没有代码签名**一律标 prerelease**
//! （总控规则红线 7），所以 GitHub 那条路会 404。这不是笔误 ——
//! 它是留给「用户决定买证书、把某一版转成正式版」那天用的，到时候不用改代码就会自己活过来。
//! 在那之前，实际生效的是 COS 那一个端点（它是静态路径，不受 prerelease 影响）。
//! 这意味着**当前自更新只有一条链路**，COS 不可达时查更新会失败并如实报原因（不会谎称已是最新）。
//!
//! 清单是 minisign 签名的，公钥编译进程序（`tauri.conf.json` 的 `plugins.updater.pubkey`），
//! 私钥只在开发机 `~/.hunter-launcher-keys/` 与仓库 secrets 里（总控规则红线 7）。
//! **签名验不过就不装** —— 这是自更新唯一的安全边界：安装包本身没有代码签名证书，
//! 更新通道要是也不验签，等于给任何能劫持 HTTP 的人一个装任意程序的口子。
//!
//! ## Linux 上的一个真实限制：`.deb` 装不了自己
//!
//! Tauri 的 updater 在 Linux 上**只能更新 AppImage** —— AppImage 是一个文件，
//! 替换它就完事了；而 `.deb` 装在 `/usr/lib` 与 `/usr/bin` 下，要 root 才动得了，
//! 一个桌面程序既不该也不能在用户不知情的时候拿 root 改系统目录。
//!
//! 所以这里按格式分成两条路，**都做完整，都不假装**：
//!
//! | 安装方式 | 点「更新」之后 |
//! |---|---|
//! | AppImage / Windows / macOS | 下载 → 验签 → 就地安装 → 重启，全自动 |
//! | `.deb`（以及别的包管理器装的） | 下载新包到 `~/.hunter/updates/` → 给出**一条可以直接粘贴的命令** |
//!
//! 第二条路听起来像半成品，但它是这个格式下唯一诚实的做法：真正装包的那一步
//! 需要用户自己给出 sudo 授权。启动器把能做的都做了（查、下、校验大小、告诉你装哪个文件）。

use std::path::PathBuf;
use std::time::Duration;

use serde::Serialize;

use crate::err::{AppError, AppResult, Code};
use crate::{linfo, lwarn, paths};

/// 检查间隔。方案 §10：「每次启动 + 每 24 小时」。
pub const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 3600);
/// 启动后多久做第一次检查。开机那几秒 CPU 与网络都紧张，让界面先出来。
pub const FIRST_CHECK_DELAY: Duration = Duration::from_secs(20);

/// 前端监听的事件名：有新版本了。
pub const EV_LAUNCHER_UPDATE: &str = "hunter://launcher-update";

/// 发布仓库。下载地址按它拼。
pub const REPO: &str = "agentpit-io/HunterLauncher";

/// 「启动器有没有新版本」。
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LauncherUpdate {
    pub available: bool,
    pub current: String,
    /// 新版本号；没有更新时为 `None`
    pub version: Option<String>,
    /// Release Notes（`latest.json` 的 `notes` 字段）
    pub notes: Option<String>,
    /// 发布时间（`latest.json` 的 `pub_date`）
    pub date: Option<String>,
    /// 这台机器上能不能就地装（见模块头的表）
    pub can_self_install: bool,
    /// 当前这份启动器是怎么装的：`appimage` / `deb` / `windows` / `macos` / `unknown`
    pub install_kind: String,
    /// 查不到时的原因，如实显示（红线 1）
    pub reason: Option<String>,
}

/// 下载好但要用户自己装时返回的东西。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManualInstall {
    pub path: String,
    pub bytes: u64,
    /// 可以直接粘贴的一条命令
    pub command: String,
    pub message: String,
}

/// 这份启动器是以什么形式装在机器上的。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallKind {
    AppImage,
    Deb,
    Windows,
    MacOS,
    Unknown,
}

impl InstallKind {
    pub fn as_str(self) -> &'static str {
        match self {
            InstallKind::AppImage => "appimage",
            InstallKind::Deb => "deb",
            InstallKind::Windows => "windows",
            InstallKind::MacOS => "macos",
            InstallKind::Unknown => "unknown",
        }
    }
    /// Tauri 的 updater 能不能就地把它换掉。
    pub fn can_self_install(self) -> bool {
        matches!(
            self,
            InstallKind::AppImage | InstallKind::Windows | InstallKind::MacOS
        )
    }
}

/// 判断安装形式。
///
/// Linux 上靠 `APPIMAGE` 环境变量 —— AppImage 的运行时在启动程序前会设好它，
/// 指向那个 `.AppImage` 文件本身。这也正是 Tauri updater 自己用的判据，
/// 所以这里的结论和它实际能不能装是一致的，不会出现「界面说能装、点了才失败」。
pub fn install_kind() -> InstallKind {
    if cfg!(target_os = "windows") {
        return InstallKind::Windows;
    }
    if cfg!(target_os = "macos") {
        return InstallKind::MacOS;
    }
    if std::env::var_os("APPIMAGE").is_some() {
        return InstallKind::AppImage;
    }
    // Linux 上不是 AppImage：绝大多数就是我们出的那个 .deb
    // （装完在 /usr/bin/hunter-launcher）。认不准就老实说 unknown。
    match std::env::current_exe() {
        Ok(p) if p.starts_with("/usr/") || p.starts_with("/opt/") => InstallKind::Deb,
        _ => InstallKind::Unknown,
    }
}

/// `.deb` 安装包在 GitHub Release 上的地址。命名与 `release.yml` 里写死的一致。
pub fn deb_url_github(version: &str) -> String {
    format!(
        "https://github.com/{REPO}/releases/download/launcher-v{version}/hunter-launcher_{version}_amd64.deb"
    )
}

/// 同一个包的国内地址（COS 香港）。
pub fn deb_url_cn(version: &str) -> String {
    format!(
        "{}/launcher/{version}/hunter-launcher_{version}_amd64.deb",
        crate::config::CN_DOWNLOAD_BASE
    )
}

/// Release 页地址（界面上「看完整说明」跳这里）。
pub fn release_url(version: &str) -> String {
    format!("https://github.com/{REPO}/releases/tag/launcher-v{version}")
}

/// 装 `.deb` 用的那条命令。`apt install ./x.deb` 比 `dpkg -i` 好：会自己补依赖。
pub fn deb_install_command(path: &str) -> String {
    format!("sudo apt install -y {path}")
}

// ── 真去问一次 ────────────────────────────────────────────────────────────

/// 查有没有新版本。**不下载任何东西。**
pub async fn check<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> LauncherUpdate {
    let kind = install_kind();
    let mut out = LauncherUpdate {
        current: env!("CARGO_PKG_VERSION").to_string(),
        can_self_install: kind.can_self_install(),
        install_kind: kind.as_str().to_string(),
        ..Default::default()
    };

    use tauri_plugin_updater::UpdaterExt;
    let updater = match app.updater() {
        Ok(u) => u,
        Err(e) => {
            out.reason = Some(format!("updater 没配好：{e}"));
            lwarn!("自更新检查失败：{e}");
            return out;
        }
    };
    match updater.check().await {
        Ok(Some(u)) => {
            linfo!(
                "发现启动器新版本 {}（当前 {}）",
                u.version,
                u.current_version
            );
            out.available = true;
            out.version = Some(u.version.clone());
            out.notes = u.body.clone();
            out.date = u.date.map(|d| d.to_string());
        }
        Ok(None) => {
            linfo!("自更新检查：已经是最新版 {}", out.current);
        }
        Err(e) => {
            // 两个端点都不通、清单还没发布、签名不对，都会走到这里。如实写原因。
            out.reason = Some(format!("{e}"));
            lwarn!("自更新检查失败：{e}");
        }
    }
    out
}

/// 装。能就地装就就地装完重启；不能就下载下来给一条命令。
///
/// 返回 `Ok(None)` = 已经就地装好，调用方应当重启进程；
/// 返回 `Ok(Some(manual))` = 包下好了，要用户自己敲那条命令。
pub async fn install<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> AppResult<Option<ManualInstall>> {
    use tauri_plugin_updater::UpdaterExt;
    let updater = app
        .updater()
        .map_err(|e| AppError::new(Code::UpdateFailed, format!("updater 没配好：{e}")))?;
    let update = updater
        .check()
        .await
        .map_err(|e| AppError::new(Code::UpdateFailed, format!("查更新失败：{e}")))?
        .ok_or_else(|| AppError::new(Code::UpdateFailed, "现在没有可装的新版本".to_string()))?;
    let version = update.version.clone();

    if install_kind().can_self_install() {
        linfo!("开始就地安装启动器 {version}");
        update
            .download_and_install(|_chunk, _total| {}, || {})
            .await
            .map_err(|e| {
                AppError::new(
                    Code::UpdateFailed,
                    format!("下载或安装 {version} 失败：{e}（当前版本没有被改动）"),
                )
            })?;
        linfo!("启动器 {version} 已安装，准备重启");
        return Ok(None);
    }

    // .deb 这类：只下载，装由用户自己来
    let manual = download_only(&version)?;
    Ok(Some(manual))
}

/// 把新版 `.deb` 下到 `~/.hunter/updates/`。两个地址按顺序试（国内在前）。
fn download_only(version: &str) -> AppResult<ManualInstall> {
    paths::ensure_dirs()?;
    let name = format!("hunter-launcher_{version}_amd64.deb");
    let dst = paths::updates_dir().join(&name);
    let mut why: Vec<String> = Vec::new();
    for url in [deb_url_cn(version), deb_url_github(version)] {
        let host = crate::http::host_of(&url);
        match download(&url, &dst) {
            Ok(n) if n > 0 => {
                linfo!("已下载 {name}（{n} 字节，来自 {host}）");
                let path = dst.to_string_lossy().into_owned();
                let command = deb_install_command(&path);
                return Ok(ManualInstall {
                    message: format!(
                        "新版本 {version} 的安装包已经下到 {path}（{}）。\
                         这台机器上的启动器是用 .deb 装的，换包要 root 权限，\
                         所以最后一步得你自己来：在终端里跑下面这条命令，装完重新打开启动器就是新版本。",
                        crate::flow::human_bytes(n)
                    ),
                    path,
                    bytes: n,
                    command,
                });
            }
            Ok(_) => why.push(format!("{host} 返回了一个空文件")),
            Err(e) => why.push(format!("{host} {}", e.msg)),
        }
    }
    Err(AppError::new(
        Code::UpdateFailed,
        format!("下载 {name} 失败：{}", why.join("；")),
    ))
}

/// 下一个文件到本地。**边收边写**，不在内存里攒整个安装包。
fn download(url: &str, dst: &PathBuf) -> AppResult<u64> {
    use std::io::Write;
    let r = crate::http::get_bytes(url, Duration::from_secs(900))?;
    if r.is_empty() {
        return Ok(0);
    }
    let mut f = std::fs::File::create(dst).map_err(|e| {
        AppError::new(
            Code::UpdateFailed,
            format!("写 {} 失败：{e}", dst.display()),
        )
    })?;
    f.write_all(&r)
        .map_err(|e| AppError::new(Code::UpdateFailed, format!("写安装包失败：{e}")))?;
    Ok(r.len() as u64)
}

// ── 后台检查 ──────────────────────────────────────────────────────────────

/// 启动时 + 每 24 小时查一次（方案 §10）。查到就发一条事件给界面，并把托盘 tooltip 改掉。
///
/// 用一条普通线程而不是 async 定时器：检查之间隔的是 24 小时，为它引一个定时器框架不值得；
/// 线程里用 [`tauri::async_runtime::block_on`] 调那个 async 的检查函数就够了
/// （这条线程不是运行时自己的线程，不会死锁）。
pub fn spawn_periodic<R: tauri::Runtime>(app: tauri::AppHandle<R>) {
    std::thread::spawn(move || {
        std::thread::sleep(FIRST_CHECK_DELAY);
        loop {
            // 用户在设置里关掉「自动检查更新」就只睡觉不查
            let enabled = {
                use tauri::Manager;
                app.state::<crate::flow::AppState>()
                    .config()
                    .launcher
                    .check_update_hours
                    > 0
            };
            if enabled {
                let u = tauri::async_runtime::block_on(check(&app));
                if u.available {
                    use tauri::Emitter;
                    let _ = app.emit(EV_LAUNCHER_UPDATE, u.clone());
                    crate::tray::note_launcher_update(&app, u.version.as_deref());
                }
            }
            std::thread::sleep(CHECK_INTERVAL);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 能不能就地装按格式分() {
        assert!(InstallKind::AppImage.can_self_install());
        assert!(InstallKind::Windows.can_self_install());
        assert!(InstallKind::MacOS.can_self_install());
        // .deb 装在 /usr 下，换它要 root —— 启动器不该悄悄拿 root
        assert!(!InstallKind::Deb.can_self_install());
        assert!(!InstallKind::Unknown.can_self_install());
    }

    #[test]
    fn 下载地址与_release_yml_的命名一致() {
        // release.yml 里改名那一步产出的就是这个文件名，两边对不上会变成 404
        assert_eq!(
            deb_url_github("0.1.0"),
            "https://github.com/agentpit-io/HunterLauncher/releases/download/launcher-v0.1.0/hunter-launcher_0.1.0_amd64.deb"
        );
        assert!(deb_url_cn("0.1.0").ends_with("/launcher/0.1.0/hunter-launcher_0.1.0_amd64.deb"));
        assert!(deb_url_cn("0.1.0").starts_with("https://hunter-dl-hk-"));
    }

    #[test]
    fn 安装包地址里没有空格() {
        // Tauri 默认用 productName（"Hunter Launcher"）当文件名，带空格，
        // 下载地址会变成 %20。CI 里已经改名，这条测试盯住别改回去
        for v in ["0.1.0", "0.1.0-rc.1"] {
            assert!(!deb_url_github(v).contains(' '), "{v}");
            assert!(!deb_url_github(v).contains("%20"), "{v}");
        }
    }

    #[test]
    fn 预发布版本号也能拼出地址() {
        assert!(deb_url_github("0.1.0-rc.1").contains("launcher-v0.1.0-rc.1"));
    }

    #[test]
    fn 装_deb_的命令用_apt_不用_dpkg() {
        // dpkg -i 不会补依赖，缺 libwebkit2gtk 时会留下一个半装的包
        let c = deb_install_command("/home/u/.hunter/updates/hunter-launcher_0.1.0_amd64.deb");
        assert!(c.starts_with("sudo apt install"), "{c}");
        assert!(c.contains("hunter-launcher_0.1.0_amd64.deb"));
    }

    #[test]
    fn 检查间隔与方案一致() {
        assert_eq!(CHECK_INTERVAL, Duration::from_secs(24 * 3600));
    }

    #[test]
    fn 查不到的时候不会谎称已是最新() {
        let u = LauncherUpdate {
            available: false,
            reason: Some("两个端点都不通".into()),
            ..Default::default()
        };
        assert!(!u.available);
        assert!(u.reason.is_some(), "拿不到要给原因（红线 1）");
    }
}
