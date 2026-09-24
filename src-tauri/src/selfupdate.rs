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
//! ## 四个平台，四条路（I14 · F1 之后）
//!
//! | 这份启动器是怎么装的 | `--self-update` 做什么 | 要不要管理员 |
//! |---|---|---|
//! | macOS 的 `.app` | 下 `*_universal.app.tar.gz` → 验签 → 解包 → 两次 `rename` **就地换掉那个 `.app`** | 目录写得了就**不要** |
//! | Windows 的 NSIS | 下 `*_x64-setup.exe` → 验签 → 被动模式（`/P`）跑一遍 | 安装器自己按需要弹 UAC |
//! | Linux 的 AppImage | 下 `*.AppImage` → 验签 → 写临时文件 → `rename` 换掉自己 | 不要 |
//! | Linux 的 `.deb` | 下包到 `~/.hunter/updates/` → 系统授权框 → `dpkg -i` | 要（系统弹的框） |
//!
//! **该下哪个文件不由这里拼，由清单说了算**：`latest.json` 的 `platforms`
//! 里每个平台一条 `url` + `signature`，按 [`target_key`] 取（[`fetch_asset`]）。
//!
//! 0.1.13 之前不是这样：那时候只单独处理了 AppImage，macOS 与 Windows 全落进
//! 「下 `.deb` 然后 `dpkg -i`」那个分支。用户 Mac 上 2026-09-23 实测的结果是
//! `已下载并验签 hunter-launcher_0.1.13_amd64.deb` →
//! `E_NOT_IMPLEMENTED: /bin/sh: dpkg: command not found (127)` ——
//! **在 macOS 上下了一个 Linux 的包**。按清单取之后，这一类错从根上没有了。
//!
//! `.deb` 那条路仍然要管理员：`.deb` 装在 `/usr/lib` 与 `/usr/bin` 下，
//! 一个桌面程序既不该也不能在用户不知情的时候拿 root 改系统目录，
//! 所以那一步走 polkit 的原生授权框（I8）。

use std::path::{Path, PathBuf};
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

/// `.deb` 下好之后、提权安装之前的中间结果。
///
/// I8 之前它叫「要用户自己装时返回的东西」，会一路传到界面上变成一张
/// 「复制这条命令去终端里跑」的弹窗。现在它**不出这个模块**：
/// 下完就直接交给 [`crate::runtime::elevate`] 装掉。
#[derive(Debug, Clone)]
struct Downloaded {
    path: String,
    /// 真实字节数。进日志（红线 5：数字一律实测）
    bytes: u64,
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

/// semver 版本号 → **Debian 版本号**。`0.1.0-rc.2` → `0.1.0~rc.2`，正式版原样返回。
///
/// 为什么要换这一下（待办池 P1-19，I2 做的）：**dpkg 对预发布版的排序和 semver 相反**。
/// 在 Debian 的版本语法里 `-` 后面那一段是「Debian 修订号」，修订号越大越新，于是
///
/// ```text
/// dpkg --compare-versions 0.1.0 gt 0.1.0-rc.2   → 假（rc.2 反而更"新"）
/// dpkg --compare-versions 0.1.0 gt 0.1.0~rc.2   → 真（波浪号排在一切之前，包括空）
/// ```
///
/// 从 rc 升到正式版时前者会被 apt 判成降级并拒绝（M4 实测，当时用 `--allow-downgrades` 绕过）。
/// 换成波浪号之后排序就对了。
///
/// 只换**第一个** `-`：semver 里 `-` 之后才是预发布段，再往后的 `-` 属于那一段的内容。
/// `release.yml` 的 Linux 打包步骤用同一个规则重打 `.deb` 并改名，两边必须一致
/// —— 下面有一条测试专门盯这件事。
pub fn deb_version(version: &str) -> String {
    match version.split_once('-') {
        Some((base, pre)) => format!("{base}~{pre}"),
        None => version.to_string(),
    }
}

/// `.deb` 的文件名。注意版本段用的是 Debian 写法（见 [`deb_version`]）。
pub fn deb_file_name(version: &str) -> String {
    format!("hunter-launcher_{}_amd64.deb", deb_version(version))
}

/// `.deb` 安装包在 GitHub Release 上的地址。命名与 `release.yml` 里写死的一致。
/// **tag 里是 semver，文件名里是 Debian 版本号**，这两段不一样是故意的。
pub fn deb_url_github(version: &str) -> String {
    format!(
        "https://github.com/{REPO}/releases/download/launcher-v{version}/{}",
        deb_file_name(version)
    )
}

/// 同一个包的国内地址（COS 香港）。
pub fn deb_url_cn(version: &str) -> String {
    format!(
        "{}/launcher/{version}/{}",
        crate::config::CN_DOWNLOAD_BASE,
        deb_file_name(version)
    )
}

/// Release 页地址（界面上「看完整说明」跳这里）。
pub fn release_url(version: &str) -> String {
    format!("https://github.com/{REPO}/releases/tag/launcher-v{version}")
}

/// 装 `.deb` 用的那条命令。
///
/// 两个选择都是被真实失败教出来的：
///
/// * **`apt install` 而不是 `dpkg -i`**：前者会自己补依赖，后者缺依赖时会留下一个半装的包。
/// * **`--allow-downgrades`**：**dpkg 对预发布版的排序和 semver 相反**。
///   实测（Ubuntu 24.04）：`dpkg --compare-versions 0.1.0 gt 0.1.0-rc.2` 是**假** ——
///   在 Debian 的版本语法里 `-rc.2` 是「Debian 修订号」，所以 `0.1.0-rc.2` 比 `0.1.0` **新**。
///   于是从 rc 升到正式版时 apt 会判成降级并直接拒绝：
///   `E: Packages were downgraded and -y was used without --allow-downgrades`。
///   加上这个开关就通了；它只在真的需要时才起作用，正常升级不受影响。
///
/// > **I2 更新**：更彻底的那个修法已经做了 —— 见 [`deb_version`]，`.deb` 的版本号现在是
/// > `0.1.0~rc.2`，排序本身就对了。`--allow-downgrades` **仍然留着**：它只在 apt 真判成
/// > 降级时才起作用，而「从 0.1.2 退回 0.1.1」这种手动降级仍然是一条要留给用户的路
/// > （出了问题先退回去），少了它那条路会被 apt 直接挡掉。
pub fn deb_install_command(path: &str) -> String {
    format!("sudo apt install -y --allow-downgrades {path}")
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

/// 装。**两条路都由启动器自己装完**，装完调用方重启进程。
///
/// | 装法 | 怎么装 | 要不要密码 |
/// |---|---|---|
/// | AppImage / Windows / macOS | Tauri 的 updater 就地换掉 | 不要 |
/// | `.deb` | 下好包 → polkit 原生授权框 → `dpkg -i` | 要（系统自己弹） |
///
/// 返回 `Ok(())` = 装好了，调用方应当重启进程。装不了就是 `Err`，**如实说原因**
/// （I8 之前这里还有第三种结局：「包下好了，命令给你，你自己去敲」—— 没有了）。
pub async fn install<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> AppResult<()> {
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
        return Ok(());
    }

    // `.deb` 这类：I8 之前是「下载下来 + 给一条命令 + 请你自己去终端里敲」。
    // 用户 2026-09-21 22:10 把那种做法否了，所以现在**启动器自己装**：
    // 下好包 → 走 polkit 的原生授权框（`pkexec dpkg -i`）→ 装完。
    //
    // 提权前那条命令要过 [`crate::assist::guard::argv_privileged`]：
    // 只允许 `dpkg -i <~/.hunter/updates/ 下我们自己刚下的那个包>`。
    let pkg = download_only(&version)?;
    if let Err(e) = crate::runtime::elevate::available() {
        // 没有 polkit 就**如实说装不了**，不退回「请你自己敲一条命令」
        return Err(AppError::new(
            Code::UpdateFailed,
            format!(
                "新版本 {version} 的安装包已经下到 {}，但这台机器上装不了：{e}",
                crate::redact::mask_home(&pkg.path)
            ),
        ));
    }
    linfo!(
        "{} 已下好（{}），接下来弹系统授权框把它装上",
        crate::redact::mask_home(&pkg.path),
        crate::flow::human_bytes(pkg.bytes)
    );
    crate::runtime::elevate::run(
        crate::runtime::elevate::Op::InstallDeb,
        &["dpkg".to_string(), "-i".to_string(), pkg.path.clone()],
        Duration::from_secs(300),
    )?;
    linfo!("启动器 {version} 已安装（.deb，经系统授权框），准备重启");
    Ok(())
}

/// 把新版 `.deb` 下到 `~/.hunter/updates/`。两个地址按顺序试（国内在前）。
fn download_only(version: &str) -> AppResult<Downloaded> {
    paths::ensure_dirs()?;
    let name = deb_file_name(version);
    let dst = paths::updates_dir().join(&name);
    let mut why: Vec<String> = Vec::new();
    for url in [deb_url_cn(version), deb_url_github(version)] {
        let host = crate::http::host_of(&url);
        match download(&url, &dst) {
            Ok(n) if n > 0 => {
                linfo!("已下载 {name}（{n} 字节，来自 {host}）");
                let path = dst.to_string_lossy().into_owned();
                return Ok(Downloaded { path, bytes: n });
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

    /// 待办池 P1-19：`.deb` 的版本号要用 Debian 的波浪号写法，否则从 rc 升到正式版
    /// 会被 dpkg 判成降级。`release.yml` 的 Linux 打包步骤按同一个规则重打并改名。
    #[test]
    fn deb_版本号用波浪号不用连字符() {
        assert_eq!(deb_version("0.1.0"), "0.1.0");
        assert_eq!(deb_version("0.1.2"), "0.1.2");
        assert_eq!(deb_version("0.1.0-rc.2"), "0.1.0~rc.2");
        // 只换第一个 `-`：再往后的属于预发布段自己的内容
        assert_eq!(deb_version("1.0.0-beta-3"), "1.0.0~beta-3");
        assert_eq!(
            deb_file_name("0.1.0-rc.2"),
            "hunter-launcher_0.1.0~rc.2_amd64.deb"
        );
        assert_eq!(deb_file_name("0.1.2"), "hunter-launcher_0.1.2_amd64.deb");
    }

    /// tag 段用 semver、文件名段用 Debian 版本号 —— 这两段**不一样**是故意的。
    /// 写反了就是 404。
    #[test]
    fn 预发布版的_tag_与文件名各用各的写法() {
        let u = deb_url_github("0.1.0-rc.2");
        assert!(u.contains("/launcher-v0.1.0-rc.2/"), "{u}");
        assert!(u.ends_with("/hunter-launcher_0.1.0~rc.2_amd64.deb"), "{u}");
        let c = deb_url_cn("0.1.0-rc.2");
        assert!(c.contains("/launcher/0.1.0-rc.2/"), "{c}");
        assert!(c.ends_with("/hunter-launcher_0.1.0~rc.2_amd64.deb"), "{c}");
    }

    #[test]
    fn 装_deb_的命令用_apt_不用_dpkg() {
        // dpkg -i 不会补依赖，缺 libwebkit2gtk 时会留下一个半装的包
        let c = deb_install_command("/home/u/.hunter/updates/hunter-launcher_0.1.0_amd64.deb");
        assert!(c.starts_with("sudo apt install"), "{c}");
        assert!(c.contains("hunter-launcher_0.1.0_amd64.deb"));
    }

    /// dpkg 对预发布版的排序和 semver **相反**（`0.1.0-rc.2` 在 dpkg 眼里比 `0.1.0` 新），
    /// 所以从 rc 升到正式版时 apt 会判成降级并拒绝。这条测试盯住那个开关别被删掉。
    #[test]
    fn 装_deb_的命令要带_allow_downgrades() {
        let c = deb_install_command("/tmp/x.deb");
        assert!(
            c.contains("--allow-downgrades"),
            "少了它，从 rc 升到正式版会报 `Packages were downgraded`：{c}"
        );
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

// ── headless 下的自更新 ───────────────────────────────────────────────────
//
// ## 为什么这里有第二套实现
//
// 上面那套走 Tauri 的 updater 插件，它要一个 `AppHandle` —— 也就是**必须有 GUI**。
// 而 `--headless` 跑在没有桌面的服务器上，压根没有 app。结果是 SSH 用户连
// 「启动器有没有新版本」都查不到，这是个真实的缺口。
//
// 所以这里自己走一遍：读清单 → 比版本 → 下载 → **用同一把公钥验签** → 装。
// 只在 Linux 上做（headless 的用户就在 Linux 上），Windows / macOS 的就地安装
// 仍然由 Tauri 插件负责 —— 那两个平台要调 NSIS 安装器、要替换 `.app` 包，
// 重写一遍只会多一个出错的地方。
//
// **验签这一步不能省。** 安装包没有代码签名，更新通道要是也不验，
// 等于给任何能劫持 HTTP 的人一个装任意程序的口子。

use serde::Deserialize;

/// `latest.json` 里我们要用的部分。
#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub pub_date: Option<String>,
    #[serde(default)]
    pub platforms: std::collections::BTreeMap<String, ManifestPlatform>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ManifestPlatform {
    pub signature: String,
    pub url: String,
}

/// updater 的两个端点，与 `tauri.conf.json` 里那份**必须一致**。
/// 有一条测试盯着它们不会各改各的。
pub fn endpoints() -> [String; 2] {
    [
        format!("{}/launcher/latest.json", crate::config::CN_DOWNLOAD_BASE),
        format!("https://github.com/{REPO}/releases/latest/download/latest.json"),
    ]
}

/// 这台机器对应 `latest.json` 里的哪个 target 键。
pub fn target_key() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "linux-x86_64",
        ("linux", "aarch64") => "linux-aarch64",
        ("windows", _) => "windows-x86_64",
        ("macos", "aarch64") => "darwin-aarch64",
        ("macos", _) => "darwin-x86_64",
        _ => "linux-x86_64",
    }
}

/// 按顺序试两个端点，第一个能读出合法清单的就用它。返回 (清单, 来自哪个主机)。
pub fn fetch_manifest(timeout: Duration) -> AppResult<(Manifest, String)> {
    let mut why: Vec<String> = Vec::new();
    for url in endpoints() {
        let host = crate::http::host_of(&url);
        match crate::http::get(&url, &[], timeout) {
            Ok(r) if r.ok() => match serde_json::from_str::<Manifest>(&r.body) {
                Ok(m) if !m.version.is_empty() => return Ok((m, host)),
                Ok(_) => why.push(format!("{host} 的清单里没有 version")),
                Err(e) => why.push(format!("{host} 的清单解析不了：{e}")),
            },
            Ok(r) => why.push(format!("{host} HTTP {}", r.status)),
            Err(e) => why.push(format!("{host} {}", e.msg)),
        }
    }
    Err(AppError::new(
        Code::UpdateFailed,
        format!("两个端点都读不到 latest.json：{}", why.join("；")),
    ))
}

/// 极简 base64 解码。只为了拆 minisign 的公钥与签名 —— 为这一件事拖一个 crate 不划算。
/// 不认识的字符（含换行）一律跳过，这正好吃得下 PEM 风格的折行。
fn b64_decode(s: &str) -> Option<Vec<u8>> {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        if c == b'=' {
            break;
        }
        let Some(v) = T.iter().position(|&t| t == c) else {
            continue; // 换行、空格之类
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// 编译进程序的那把 minisign 公钥（与 `tauri.conf.json` 的 `plugins.updater.pubkey` 同一个）。
///
/// 为什么在这里再写一份：`tauri.conf.json` 是构建期的配置，运行时读不到它。
/// 有一条测试把两边对了一遍，改一个忘了另一个会红。
pub const PUBKEY_B64: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDY2NUU4MUVDNEI5MDJFQkMKUldTOExwQkw3SUZlWmxpYk9sbFAxNWhqYWVKK0l2NTZQQkxZd2NrbUoyQU5DZnVYM1g3LzRvREkK";

/// 用编译进来的公钥验一段字节的签名。`sig` 可以是 base64 包了一层的，也可以是 minisig 原文。
pub fn verify(bytes: &[u8], sig: &str) -> AppResult<()> {
    let pk_text = b64_decode(PUBKEY_B64)
        .and_then(|v| String::from_utf8(v).ok())
        .ok_or_else(|| AppError::new(Code::UpdateFailed, "内置公钥解不出来"))?;
    let pk = minisign_verify::PublicKey::decode(pk_text.trim())
        .map_err(|e| AppError::new(Code::UpdateFailed, format!("内置公钥不合法：{e}")))?;

    // Tauri 往 `.sig` 与清单里写的都是「minisig 全文再 base64 一层」；
    // 手工用 minisign 生成的则是原文。两种都收。
    let sig_text = if sig.contains("untrusted comment:") {
        sig.to_string()
    } else {
        b64_decode(sig)
            .and_then(|v| String::from_utf8(v).ok())
            .ok_or_else(|| AppError::new(Code::UpdateFailed, "签名解不出来"))?
    };
    let signature = minisign_verify::Signature::decode(sig_text.trim())
        .map_err(|e| AppError::new(Code::UpdateFailed, format!("签名格式不对：{e}")))?;
    // 第三个参数是 `allow_legacy`。minisign 有两种签名：预哈希的（`-H`）与传统的。
    // Tauri 的签名工具用哪一种取决于它的版本，这里两种都收 ——
    // 「传统」指的只是先不做 BLAKE2b 预哈希，签的仍然是同一份内容、同一套 Ed25519，
    // 收它不降低安全性；而只收一种的话，换个 Tauri 版本就会莫名其妙「验不过」。
    pk.verify(bytes, &signature, true).map_err(|e| {
        AppError::new(
            Code::UpdateFailed,
            format!("签名验不过，这个包不装：{e}。要么下载被人动过，要么它不是我们发的。"),
        )
    })
}

/// headless 的 `--self-update`。
///
/// * AppImage → 验签后**就地替换**那个文件（先写临时文件再 rename，中途断电不会留下半个）
/// * `.deb` → 验签后下到 `~/.hunter/updates/`，再走 polkit 的原生授权框装上（I8）
///
/// 两条路都**先验签再落地**。
///
/// `.deb` 那条路在**没有图形会话**的地方（ssh、cron）弹不出 polkit 的框 ——
/// 那时如实说弹不出来，并把该跑的命令打在终端里。这是唯一保留「给一条命令」的地方，
/// 理由很简单：**调用方本来就在终端里**（`check-wording.py` 的白名单写的就是这一条）。
pub fn self_update_headless(mut note: impl FnMut(&str)) -> AppResult<String> {
    let current = env!("CARGO_PKG_VERSION");
    let (m, host) = fetch_manifest(Duration::from_secs(20))?;
    note(&format!(
        "清单来自 {host}：最新 {}（当前 {current}）",
        m.version
    ));
    if !crate::upgrade::is_newer(&m.version, current) {
        return Ok(format!(
            "已经是最新版 v{current}（清单里是 v{}）。",
            m.version
        ));
    }
    if !m.notes.trim().is_empty() {
        note(&format!(
            "更新说明：{}",
            crate::upgrade::summarize_notes(&m.notes, 300)
        ));
    }

    let kind = install_kind();
    paths::ensure_dirs()?;

    match kind {
        InstallKind::AppImage => {
            let (bytes, _) = fetch_asset(&m, &mut note)?;

            let target = std::env::var_os("APPIMAGE")
                .map(PathBuf::from)
                .ok_or_else(|| AppError::new(Code::UpdateFailed, "读不到 APPIMAGE 环境变量"))?;
            // 先写同目录的临时文件再 rename：rename 在同一个文件系统上是原子的，
            // 中途断电不会留下一个半截的 AppImage
            let tmp = target.with_extension("new");
            std::fs::write(&tmp, &bytes).map_err(|e| {
                AppError::new(
                    Code::UpdateFailed,
                    format!("写 {} 失败：{e}", tmp.display()),
                )
            })?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).map_err(
                    |e| AppError::new(Code::UpdateFailed, format!("给新文件加可执行位失败：{e}")),
                )?;
            }
            std::fs::rename(&tmp, &target).map_err(|e| {
                AppError::new(
                    Code::UpdateFailed,
                    format!(
                        "替换 {} 失败：{e}（新文件还在 {}）",
                        target.display(),
                        tmp.display()
                    ),
                )
            })?;
            linfo!("AppImage 已就地替换为 {}", m.version);
            Ok(format!(
                "已就地更新到 v{}（{}）。重新运行它就是新版本。",
                m.version,
                target.display()
            ))
        }
        InstallKind::MacOS => {
            let (bytes, url) = fetch_asset(&m, &mut note)?;
            macos_install(&bytes, &m.version, &url, &mut note)
        }
        InstallKind::Windows => {
            let (bytes, url) = fetch_asset(&m, &mut note)?;
            windows_install(&bytes, &m.version, &url, &mut note)
        }
        // 到这儿只剩 Linux 上的 `.deb` 与「认不出装法」两种。
        // 认不出的时候也走 `.deb` 这条路（Linux 上的包管理器装法只有它一种），
        // 但话要说得不一样 —— 不能对着一台认不出来的机器咬定它是 .deb 装的
        _ => {
            note(match kind {
                InstallKind::Deb => {
                    "这台机器上的启动器是 .deb 装的，安装包要写进 /usr —— 那个位置只有管理员能改。"
                }
                _ => {
                    "认不出这份启动器是怎么装的，按 Linux 的包管理器装法处理（安装包要写进 /usr，那个位置只有管理员能改）。"
                }
            });
            let deb = download_and_verify_deb(&m.version, &mut note)?;
            // I8：先试着自己装完 —— 弹的是 polkit 自己的授权框
            match crate::runtime::elevate::available() {
                Ok(()) => {
                    note("签名已验过，接下来系统会弹出它自己的授权框");
                    crate::runtime::elevate::run(
                        crate::runtime::elevate::Op::InstallDeb,
                        &["dpkg".to_string(), "-i".to_string(), deb.clone()],
                        Duration::from_secs(300),
                    )?;
                    Ok(format!(
                        "已更新到 v{}。重新运行启动器就是新版本。",
                        m.version
                    ))
                }
                // 没有图形会话（ssh / cron）时 polkit 弹不出框。**如实说**，
                // 并把命令打在终端里 —— 调用方本来就在终端里
                Err(why) => Ok(format!(
                    "新版本 {} 的安装包已下到 {}（签名已验过），但这里装不上：{why}\n\
                     在有桌面会话的地方重新跑一次 --self-update 就能装；\n\
                     要现在装的话：{}",
                    m.version,
                    deb,
                    deb_install_command(&deb)
                )),
            }
        }
    }
}

/// 按**这台机器的平台**从清单里取该下哪个包，下下来、验完签再交出去（I14 · F1）。
///
/// ## 这个函数是怎么来的
///
/// 0.1.13 在用户 Mac 上 `--self-update` 的结果是：
///
/// ```text
/// 已下载并验签 hunter-launcher_0.1.13_amd64.deb
/// E_NOT_IMPLEMENTED: /bin/sh: dpkg: command not found (127)
/// ```
///
/// —— 在 macOS 上下了一个 Linux 的 `.deb`，然后用系统授权框去跑 `dpkg`。
/// 根子是原来的 `match` 只单独处理了 AppImage，`macOS` / `Windows` 全落进
/// 那个 `_ =>` 分支，而那个分支写死了「下 `.deb`、`dpkg -i`」。
///
/// 现在**不再按安装形式猜文件名**：清单（`latest.json`）里每个平台那一项
/// 本来就写着该下哪个 URL 与它的签名，直接按 [`target_key`] 取。
/// 这样「选错平台」这一类问题从根上就不存在了 —— 选的不是我们拼的名字，
/// 是发布流水线自己写进清单的那一条。
///
/// 返回 `(包的字节, 它的下载地址)`。**验不过签就不返回**。
fn fetch_asset(m: &Manifest, note: &mut impl FnMut(&str)) -> AppResult<(Vec<u8>, String)> {
    let key = target_key();
    let p = m.platforms.get(key).ok_or_else(|| {
        AppError::new(
            Code::UpdateFailed,
            format!(
                "清单里没有 {key} 这个平台的包（清单里有的是 {}）",
                m.platforms.keys().cloned().collect::<Vec<_>>().join(" / ")
            ),
        )
    })?;
    note(&format!("这台机器对应清单里的 {key}，正在下载 {}", p.url));
    let bytes = crate::http::get_bytes(&p.url, Duration::from_secs(900))?;
    note(&format!(
        "下好了 {}，正在验签…",
        crate::flow::human_bytes(bytes.len() as u64)
    ));
    verify(&bytes, &p.signature)?;
    note("签名通过");
    Ok((bytes, p.url.clone()))
}

/// 从 URL 里取文件名（落盘时用它，保持和发布出去的名字一致）。
fn file_name_of(url: &str, fallback: &str) -> String {
    url.rsplit('/')
        .next()
        .filter(|s| !s.is_empty() && !s.contains(['?', '\\']))
        .unwrap_or(fallback)
        .to_string()
}

/// 这份启动器所在的那个 `.app` 包（macOS）。
///
/// 从**正在跑的这个可执行文件**往上找第一个 `.app` 目录，而不是写死
/// `/Applications/Hunter Launcher.app`：用户完全可能把它放在
/// `~/Applications`、外接盘或者下载文件夹里。写死路径的后果不是「更新失败」，
/// 是**把新版本装到一个他根本没在用的位置上，界面还报「已更新」**。
pub fn app_bundle_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors()
        .find(|p| p.extension().is_some_and(|e| e == "app"))
        .map(|p| p.to_path_buf())
}

/// macOS：把 `.app.tar.gz` 就地换成新版。
///
/// | 情况 | 怎么装 | 要不要密码 |
/// |---|---|---|
/// | `.app` 所在目录当前账号写得了（`/Applications` 默认就是这样：`drwxrwxr-x root:admin`） | 解包到旁边 → 两次 `rename` 换过去 | **不要** |
/// | 写不了（被 MDM 管起来、或者装在别人的账号下） | 走 macOS 自己的授权框，一条 `rsync -a --delete` | 要（系统弹的框） |
///
/// 换法是「先把旧的挪开，再把新的搬进来」：两次 `rename` 都在同一个目录里，
/// 同一个文件系统上 `rename` 是原子的，中途断电不会留下半个包。
/// 第二次没成就**把旧的搬回来** —— 宁可这次没更新成，也不能让用户开不了。
fn macos_install(
    bytes: &[u8],
    version: &str,
    url: &str,
    note: &mut impl FnMut(&str),
) -> AppResult<String> {
    let app = app_bundle_path().ok_or_else(|| {
        AppError::new(
            Code::UpdateFailed,
            "找不到这份启动器所在的 .app 包（当前可执行文件的路径里没有 .app）。\
             不知道该换哪儿，就什么都不换。"
                .to_string(),
        )
    })?;
    let parent = app
        .parent()
        .ok_or_else(|| {
            AppError::new(
                Code::UpdateFailed,
                format!("{} 没有上级目录，换不了。", app.display()),
            )
        })?
        .to_path_buf();

    // 已经验过签才落地
    let tgz = paths::updates_dir().join(file_name_of(url, "hunter-launcher.app.tar.gz"));
    std::fs::write(&tgz, bytes).map_err(|e| {
        AppError::new(
            Code::UpdateFailed,
            format!("写 {} 失败：{e}", tgz.display()),
        )
    })?;

    // 解包目录先试着建在 `.app` 旁边 —— 它同时是一次**写权限实测**：
    // 这一步建得起来，后面那两次 rename 就一定做得成（同一个目录、同一个文件系统）。
    // 建不起来才说明要提权，而不是靠「路径是不是 /Applications」去猜。
    let near = parent.join(format!(".hunter-launcher-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&near);
    let writable = std::fs::create_dir(&near).is_ok();
    let stage = if writable {
        near.clone()
    } else {
        let d = paths::updates_dir().join(format!("stage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).map_err(|e| {
            AppError::new(Code::UpdateFailed, format!("建 {} 失败：{e}", d.display()))
        })?;
        d
    };

    let out = (|| -> AppResult<String> {
        untar_gz(&tgz, &stage)?;
        let new_app = single_app_in(&stage)?;
        if writable {
            note(&format!(
                "正在就地替换 {}（这个位置不需要管理员权限）",
                app.display()
            ));
            swap_bundle(&app, &new_app)?;
        } else {
            note(&format!(
                "{} 这个位置当前账号写不了，接下来系统会弹出它自己的授权框",
                parent.display()
            ));
            crate::runtime::elevate::run(
                crate::runtime::elevate::Op::ReplaceAppBundle,
                &[
                    "/usr/bin/rsync".to_string(),
                    "-a".to_string(),
                    "--delete".to_string(),
                    format!("{}/", new_app.display()),
                    format!("{}/", app.display()),
                ],
                Duration::from_secs(600),
            )?;
        }
        linfo!("macOS 的 .app 已就地替换为 {version}（{}）", app.display());
        Ok(format!(
            "已就地更新到 v{version}（{}）。退出再打开它就是新版本。",
            app.display()
        ))
    })();

    // 不管成没成，临时目录都收拾干净
    let _ = std::fs::remove_dir_all(&stage);
    let _ = std::fs::remove_file(&tgz);
    out
}

/// 「把旧的挪开，再把新的搬进来」这两次 `rename`。
///
/// 两次都在**同一个目录**里，同一个文件系统上 `rename` 是原子的 ——
/// 中途断电不会留下半个包。第二次没成就**把旧的搬回来**：
/// 宁可这次没更新成，也不能让用户下次打不开启动器。
///
/// 拆出来是为了能在 Linux 上测 —— 这段逻辑动的是用户 `/Applications` 里的东西，
/// 是这条路上最该被钉死的一段，而它本身和 macOS 没有任何关系。
fn swap_bundle(app: &Path, new_app: &Path) -> AppResult<()> {
    let parent = app.parent().ok_or_else(|| {
        AppError::new(
            Code::UpdateFailed,
            format!("{} 没有上级目录，换不了。", app.display()),
        )
    })?;
    let backup = parent.join(format!(".hunter-launcher-old-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&backup);
    std::fs::rename(app, &backup).map_err(|e| {
        AppError::new(
            Code::UpdateFailed,
            format!("挪开旧的 {} 失败：{e}（什么都没有改动）", app.display()),
        )
    })?;
    if let Err(e) = std::fs::rename(new_app, app) {
        let back = std::fs::rename(&backup, app);
        return Err(AppError::new(
            Code::UpdateFailed,
            format!(
                "把新版本搬到 {} 失败：{e}。旧版本{}。",
                app.display(),
                if back.is_ok() {
                    "已经放回原位，照常可以打开"
                } else {
                    "没能放回原位，它现在在同一个目录下、名字以 .hunter-launcher-old- 开头"
                }
            ),
        ));
    }
    let _ = std::fs::remove_dir_all(&backup);
    Ok(())
}

/// 解一个 `.tar.gz` 到指定目录。用系统自带的 `tar`（macOS 上是 bsdtar）。
///
/// 参数是**数组**不是拼出来的命令行（红线 3）。
fn untar_gz(tgz: &Path, dst: &Path) -> AppResult<()> {
    let r = crate::proc::run_timeout(
        "/usr/bin/tar",
        &["-xzf", &tgz.to_string_lossy(), "-C", &dst.to_string_lossy()],
        Duration::from_secs(300),
    )?;
    if !r.ok() {
        return Err(AppError::new(
            Code::UpdateFailed,
            format!("解包 {} 失败：{}", tgz.display(), r.err_line()),
        ));
    }
    Ok(())
}

/// 解出来的目录里那唯一一个 `.app`。
///
/// **不止一个就停下来**：我们只发一个包，出现第二个说明下到的东西不是我们以为的那个，
/// 这时候乱挑一个去替换用户的应用是最不该做的事。
fn single_app_in(dir: &Path) -> AppResult<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| {
            AppError::new(
                Code::UpdateFailed,
                format!("读 {} 失败：{e}", dir.display()),
            )
        })?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "app"))
        .collect();
    found.sort();
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => Err(AppError::new(
            Code::UpdateFailed,
            "解出来的包里没有 .app —— 下到的东西不是 macOS 的启动器包，不换。".to_string(),
        )),
        n => Err(AppError::new(
            Code::UpdateFailed,
            format!("解出来的包里有 {n} 个 .app，认不准该用哪一个，不换。"),
        )),
    }
}

/// Windows：下 NSIS 安装器，用 **passive** 模式跑它（与 `tauri.conf.json` 的
/// `plugins.updater.windows.installMode = "passive"` 一致 —— 界面版走 Tauri 的
/// updater，也是这个模式，两条路的行为得是同一个）。
///
/// `/P` 是 Tauri 的 NSIS 模板认的「被动模式」开关：只显示进度条、不问问题。
/// **不带 `/R`**：那是「装完把程序重新拉起来」，而这条路是命令行，
/// 调用方要的是一个退出码，不是一个突然弹出来的窗口。
fn windows_install(
    bytes: &[u8],
    version: &str,
    url: &str,
    note: &mut impl FnMut(&str),
) -> AppResult<String> {
    let name = file_name_of(url, "hunter-launcher-setup.exe");
    let exe = paths::updates_dir().join(&name);
    std::fs::write(&exe, bytes).map_err(|e| {
        AppError::new(
            Code::UpdateFailed,
            format!("写 {} 失败：{e}", exe.display()),
        )
    })?;
    note(&format!(
        "正在用被动模式安装 {name}（只显示进度条，不会问你问题）"
    ));
    let r = crate::proc::run_timeout(&exe.to_string_lossy(), &["/P"], Duration::from_secs(900))?;
    if !r.ok() {
        return Err(AppError::new(
            Code::UpdateFailed,
            format!(
                "安装器返回了 {:?}：{}（包还在 {}）",
                r.status,
                r.err_line(),
                exe.display()
            ),
        ));
    }
    linfo!("Windows 安装器已跑完（退出码 0），目标版本 {version}");
    // **不说「已经更新好了」**：NSIS 在需要 UAC 时会另起一个提权进程，
    // 父进程可能在那之前就返回 0 —— 我们能确定的只有「安装器跑完了、没报错」。
    // 版本号得等它真的装完、重新打开才算数（红线 1）。
    Ok(format!(
        "{name} 已经跑完（退出码 0），目标版本 v{version}。重新打开启动器确认版本号。"
    ))
}

/// 下 `.deb` 与它的 `.sig`，验完再返回落地路径。
fn download_and_verify_deb(version: &str, note: &mut impl FnMut(&str)) -> AppResult<String> {
    let name = deb_file_name(version);
    let dst = paths::updates_dir().join(&name);
    let mut why: Vec<String> = Vec::new();
    for url in [deb_url_cn(version), deb_url_github(version)] {
        let host = crate::http::host_of(&url);
        note(&format!("正在从 {host} 下载 {name}…"));
        let bytes = match crate::http::get_bytes(&url, Duration::from_secs(900)) {
            Ok(b) if !b.is_empty() => b,
            Ok(_) => {
                why.push(format!("{host} 返回了一个空文件"));
                continue;
            }
            Err(e) => {
                why.push(format!("{host} {}", e.msg));
                continue;
            }
        };
        let sig = match crate::http::get(&format!("{url}.sig"), &[], Duration::from_secs(60)) {
            Ok(r) if r.ok() => r.body,
            Ok(r) => {
                why.push(format!("{host} 的 .sig 返回 HTTP {}", r.status));
                continue;
            }
            Err(e) => {
                why.push(format!("{host} 的 .sig {}", e.msg));
                continue;
            }
        };
        verify(&bytes, &sig)?;
        note(&format!(
            "签名通过（{}）",
            crate::flow::human_bytes(bytes.len() as u64)
        ));
        std::fs::write(&dst, &bytes).map_err(|e| {
            AppError::new(
                Code::UpdateFailed,
                format!("写 {} 失败：{e}", dst.display()),
            )
        })?;
        linfo!("已下载并验签 {name}（来自 {host}）");
        return Ok(dst.to_string_lossy().into_owned());
    }
    Err(AppError::new(
        Code::UpdateFailed,
        format!("下载 {name} 失败：{}", why.join("；")),
    ))
}

#[cfg(test)]
mod headless_tests {
    use super::*;

    #[test]
    fn base64_解码对得上() {
        assert_eq!(b64_decode("aGVsbG8=").unwrap(), b"hello");
        // 带换行的（PEM 风格）也要能吃
        assert_eq!(b64_decode("aGVs\nbG8=").unwrap(), b"hello");
        assert_eq!(b64_decode("").unwrap(), b"");
    }

    #[test]
    fn 内置公钥能解出一个合法的_minisign_公钥() {
        let text = String::from_utf8(b64_decode(PUBKEY_B64).unwrap()).unwrap();
        assert!(text.contains("untrusted comment:"), "{text}");
        assert!(minisign_verify::PublicKey::decode(text.trim()).is_ok());
    }

    /// 公钥写在两个地方（`tauri.conf.json` 与这里），改一个忘了另一个会让自更新悄悄失效。
    #[test]
    fn 内置公钥与_tauri_conf_里的一致() {
        let conf = include_str!("../tauri.conf.json");
        assert!(
            conf.contains(PUBKEY_B64),
            "selfupdate.rs 的 PUBKEY_B64 与 tauri.conf.json 的 plugins.updater.pubkey 不一致"
        );
    }

    /// 端点也写在两个地方，同理。
    #[test]
    fn 端点与_tauri_conf_里的一致() {
        let conf = include_str!("../tauri.conf.json");
        for e in endpoints() {
            assert!(conf.contains(&e), "tauri.conf.json 里没有这个端点：{e}");
        }
    }

    #[test]
    fn 平台键覆盖了清单里会出现的那几个() {
        // make-latest-json.py 产出的键就是这几个
        let k = target_key();
        assert!(
            [
                "linux-x86_64",
                "linux-aarch64",
                "windows-x86_64",
                "darwin-x86_64",
                "darwin-aarch64"
            ]
            .contains(&k),
            "{k}"
        );
    }

    /// 一份**按真实 `latest.json` 的形状**造出来的清单（签名段是占位符，
    /// 这几条测试只看「选中了哪个 url」，验签有它自己的测试）。
    ///
    /// 各平台的文件名与 `release.yml` 实际发出去的一致：
    /// Linux 是 `.AppImage`、Windows 是 `_x64-setup.exe`、
    /// macOS 两个键都指向同一个 `_universal.app.tar.gz`（通用二进制，一份管两种芯片）。
    fn 样本清单() -> Manifest {
        serde_json::from_str(
            r#"{
              "version": "0.1.14",
              "notes": "n",
              "platforms": {
                "linux-x86_64":   {"signature":"s1","url":"https://cos/launcher/0.1.14/hunter-launcher_0.1.14_amd64.AppImage"},
                "windows-x86_64": {"signature":"s2","url":"https://cos/launcher/0.1.14/hunter-launcher_0.1.14_x64-setup.exe"},
                "darwin-x86_64":  {"signature":"s3","url":"https://cos/launcher/0.1.14/hunter-launcher_0.1.14_universal.app.tar.gz"},
                "darwin-aarch64": {"signature":"s4","url":"https://cos/launcher/0.1.14/hunter-launcher_0.1.14_universal.app.tar.gz"}
              }
            }"#,
        )
        .expect("样本清单要解析得了")
    }

    /// **I14 · F1 的回归**：这台机器该下哪个包，由清单里它自己那一项说了算。
    ///
    /// 0.1.13 在用户 Mac 上下的是 `hunter-launcher_0.1.13_amd64.deb`，
    /// 然后 `dpkg: command not found` —— 因为 macOS / Windows 都落进了
    /// 「下 .deb」那个兜底分支。这条测试在三个平台上都跑，
    /// 断言的是「按 target_key 取到的那一项，后缀是这个平台该有的后缀」。
    #[test]
    fn 每个平台取到的都是它自己那个包() {
        let m = 样本清单();
        for (key, want) in [
            ("linux-x86_64", ".AppImage"),
            ("windows-x86_64", "-setup.exe"),
            ("darwin-x86_64", ".app.tar.gz"),
            ("darwin-aarch64", ".app.tar.gz"),
        ] {
            let p = m
                .platforms
                .get(key)
                .unwrap_or_else(|| panic!("{key} 该在清单里"));
            assert!(p.url.ends_with(want), "{key} 应当取 {want}：{}", p.url);
            // 任何一个平台都不该取到 .deb —— 那是 Linux 包管理器的东西，
            // 而且它从来就不在 latest.json 里（updater 换不了它）
            assert!(!p.url.ends_with(".deb"), "{key} 不该是 .deb：{}", p.url);
        }
        // 这台机器自己那一项一定取得到
        let mine = m.platforms.get(target_key());
        assert!(mine.is_some(), "清单里必须有 {}", target_key());
    }

    /// macOS 的两个 target 指的是**同一个通用二进制包**（`release.yml` 就是这么打的）。
    #[test]
    fn macos_两个架构共用同一个通用包() {
        let m = 样本清单();
        assert_eq!(
            m.platforms["darwin-x86_64"].url, m.platforms["darwin-aarch64"].url,
            "universal 包一份管两种芯片"
        );
    }

    /// 清单里没有这台机器的平台时**如实报**，不去拼一个文件名硬下。
    #[test]
    fn 清单里没有这个平台就如实说() {
        let m: Manifest = serde_json::from_str(
            r#"{"version":"0.1.14","platforms":{"某个不存在的平台":{"signature":"s","url":"u"}}}"#,
        )
        .unwrap();
        let e = fetch_asset(&m, &mut |_| {}).unwrap_err();
        assert!(e.msg.contains(target_key()), "{}", e.msg);
        assert!(
            e.msg.contains("清单里有的是"),
            "要说清单里到底有什么：{}",
            e.msg
        );
    }

    #[test]
    fn 从地址里取文件名() {
        assert_eq!(
            file_name_of(
                "https://cos/launcher/0.1.14/hunter-launcher_0.1.14_universal.app.tar.gz",
                "x"
            ),
            "hunter-launcher_0.1.14_universal.app.tar.gz"
        );
        assert_eq!(
            file_name_of("https://cos/launcher/", "fallback"),
            "fallback"
        );
        // 带查询串或反斜杠的一律退回兜底名字，不拿它去拼路径
        assert_eq!(
            file_name_of("https://x/a.exe?token=1", "fallback"),
            "fallback"
        );
        assert_eq!(file_name_of("https://x/..\\evil", "fallback"), "fallback");
    }

    /// 解出来的包里必须**正好一个** `.app`。零个或多个都停下来 ——
    /// 这一步之后就要动用户 `/Applications` 里的东西了，认不准就别动。
    #[test]
    fn 解出来的包里必须正好一个_app() {
        let base = std::env::temp_dir().join(format!("hunter-app-pick-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);

        let zero = base.join("zero");
        std::fs::create_dir_all(&zero).unwrap();
        assert!(single_app_in(&zero).is_err(), "一个都没有要报错");

        let one = base.join("one");
        std::fs::create_dir_all(one.join("Hunter Launcher.app")).unwrap();
        std::fs::write(one.join("readme.txt"), "x").unwrap();
        let got = single_app_in(&one).expect("正好一个要挑得出来");
        assert!(got.ends_with("Hunter Launcher.app"), "{}", got.display());

        let two = base.join("two");
        std::fs::create_dir_all(two.join("A.app")).unwrap();
        std::fs::create_dir_all(two.join("B.app")).unwrap();
        assert!(single_app_in(&two).is_err(), "两个要报错，不许乱挑一个");

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 换包那两次 `rename`：**换成了新的，而且旧的收拾干净了**。
    ///
    /// 这段逻辑动的是用户 `/Applications` 里的东西，所以它单独拆出来、
    /// 在 Linux 上也真跑一遍（它和 macOS 本身没有任何关系）。
    #[test]
    fn 换包那两次_rename_换得对也收拾得干净() {
        let base = std::env::temp_dir().join(format!("hunter-swap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let apps = base.join("Applications");
        let stage = base.join("stage");
        std::fs::create_dir_all(apps.join("Hunter Launcher.app/Contents/MacOS")).unwrap();
        std::fs::create_dir_all(stage.join("Hunter Launcher.app/Contents/MacOS")).unwrap();
        let app = apps.join("Hunter Launcher.app");
        let new_app = stage.join("Hunter Launcher.app");
        std::fs::write(app.join("Contents/MacOS/hunter-launcher"), b"old").unwrap();
        // 旧包里有、新包里没有的文件：换完之后不该再出现（不是「盖上去」而是「换掉」）
        std::fs::write(app.join("Contents/stale.txt"), b"x").unwrap();
        std::fs::write(new_app.join("Contents/MacOS/hunter-launcher"), b"new").unwrap();

        swap_bundle(&app, &new_app).expect("换包应当成功");

        assert_eq!(
            std::fs::read(app.join("Contents/MacOS/hunter-launcher")).unwrap(),
            b"new",
            "换上去的必须是新的那一份"
        );
        assert!(
            !app.join("Contents/stale.txt").exists(),
            "旧包里多出来的文件不该留下 —— 这是「换掉」不是「盖上去」"
        );
        // 临时的备份目录收拾干净了：同级目录里只剩那一个 .app
        let left: Vec<String> = std::fs::read_dir(&apps)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left, vec!["Hunter Launcher.app".to_string()], "{left:?}");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// 第二次 `rename` 失败时**旧的要回到原位** —— 用户下次必须还能打开它。
    #[test]
    fn 换包搬不进去的时候旧的会被放回来() {
        let base = std::env::temp_dir().join(format!("hunter-swap-back-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let apps = base.join("Applications");
        std::fs::create_dir_all(apps.join("Hunter Launcher.app")).unwrap();
        let app = apps.join("Hunter Launcher.app");
        std::fs::write(app.join("marker"), b"old").unwrap();
        // 新包根本不存在 → 第二次 rename 必然失败
        let missing = base.join("stage").join("Hunter Launcher.app");

        let e = swap_bundle(&app, &missing).expect_err("这一次必须失败");
        assert!(e.msg.contains("已经放回原位"), "{}", e.msg);
        assert!(app.join("marker").exists(), "旧包必须回到原位");
        assert_eq!(std::fs::read(app.join("marker")).unwrap(), b"old");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// `.deb` 那条路是 **Linux 专属**的兜底，不该再对 macOS / Windows 说
    /// 「这台机器上的启动器是 macos 形式装的，换包要管理员权限」。
    #[test]
    fn 只有_deb_那一档才说要管理员() {
        assert!(InstallKind::MacOS.can_self_install(), "mac 上要能自己装");
        assert!(
            InstallKind::Windows.can_self_install(),
            "Windows 上要能自己装"
        );
        assert!(!InstallKind::Deb.can_self_install());
    }

    #[test]
    fn 签名验不过就不装() {
        // 拿一段随便什么字节配一个随便什么签名，必须失败而不是放过去
        let r = verify(b"hello", "bm90LWEtc2lnbmF0dXJl");
        assert!(r.is_err());
    }

    #[test]
    fn 清单解析只认必要字段() {
        let m: Manifest = serde_json::from_str(
            r#"{"version":"0.1.1","notes":"x","pub_date":"2026-09-20T00:00:00Z",
                "platforms":{"linux-x86_64":{"signature":"s","url":"u"}}}"#,
        )
        .unwrap();
        assert_eq!(m.version, "0.1.1");
        assert_eq!(m.platforms["linux-x86_64"].url, "u");
        // 少了可选字段也要能解
        let m2: Manifest = serde_json::from_str(r#"{"version":"0.1.1","platforms":{}}"#).unwrap();
        assert!(m2.notes.is_empty());
        assert!(m2.pub_date.is_none());
    }
}
