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
    pk.verify(bytes, &signature, false).map_err(|e| {
        AppError::new(
            Code::UpdateFailed,
            format!("签名验不过，这个包不装：{e}。要么下载被人动过，要么它不是我们发的。"),
        )
    })
}

/// headless 的 `--self-update`。
///
/// * AppImage → 验签后**就地替换**那个文件（先写临时文件再 rename，中途断电不会留下半个）
/// * `.deb` / 认不出来的 → 下到 `~/.hunter/updates/` 并返回一条要用户自己敲的命令
///
/// 两条路都**先验签再落地**。
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
            let p = m.platforms.get(target_key()).ok_or_else(|| {
                AppError::new(
                    Code::UpdateFailed,
                    format!("清单里没有 {} 这个平台的包", target_key()),
                )
            })?;
            note(&format!("正在下载 {}", p.url));
            let bytes = crate::http::get_bytes(&p.url, Duration::from_secs(900))?;
            note(&format!(
                "下好了 {}，正在验签…",
                crate::flow::human_bytes(bytes.len() as u64)
            ));
            verify(&bytes, &p.signature)?;
            note("签名通过");

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
        _ => {
            note(&format!(
                "这台机器上的启动器是 {} 形式装的，换包要 root —— 只下载，不替你装。",
                kind.as_str()
            ));
            let deb = download_and_verify_deb(&m.version, &mut note)?;
            Ok(format!(
                "新版本 {} 的安装包已下到 {}（签名已验过）。装它：\n  {}",
                m.version,
                deb,
                deb_install_command(&deb)
            ))
        }
    }
}

/// 下 `.deb` 与它的 `.sig`，验完再返回落地路径。
fn download_and_verify_deb(version: &str, note: &mut impl FnMut(&str)) -> AppResult<String> {
    let name = format!("hunter-launcher_{version}_amd64.deb");
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
