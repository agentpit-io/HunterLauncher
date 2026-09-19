//! 开机自启（技术方案 §5.8 最后一条：「开机自启（可选，**默认关**）」）。
//!
//! 三个平台三套机制，都是**用户级**的，不碰系统目录、不需要 root：
//!
//! | 平台 | 机制 | 落点 |
//! |---|---|---|
//! | Linux | XDG 自启动规范 | `~/.config/autostart/hunter-launcher.desktop` |
//! | macOS | LaunchAgent | `~/Library/LaunchAgents/io.agentpit.hunter.launcher.plist` |
//! | Windows | 注册表 Run 键 | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` |
//!
//! Linux / macOS 两条是写文件，Windows 那条调 `reg.exe`，**参数一律数组**（红线 3）。
//!
//! ## 自启起来的是什么
//!
//! 带 `--minimized` 参数启动：进程起来、托盘出现，但**不弹窗**。开机时糊用户一脸窗口
//! 是最招人烦的做法。`main.rs` 认这个参数，`lib.rs` 据此决定主窗口建出来后显不显示。
//!
//! ## 为什么不用 `tauri-plugin-autostart`
//!
//! 那个插件在 Linux 上写的是同一个 `.desktop` 文件，在 Windows 上写的是同一个注册表键，
//! 做的事一模一样；自己写这 100 行的好处是**能读回真实状态**（`status()` 去看文件/注册表
//! 到底在不在），界面上的开关显示的就是系统里真实的样子，而不是 `launcher.toml` 里
//! 记着的一个可能已经和现实脱节的布尔值（红线 1）。

use crate::err::{AppError, AppResult, Code};

/// macOS 的 LaunchAgent 标签。Linux 上用不到（那边是 .desktop 文件名）。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const APP_ID: &str = "io.agentpit.hunter.launcher";
/// Windows 注册表 Run 键里的值名。其它平台用不到。
#[cfg_attr(not(windows), allow(dead_code))]
const RUN_VALUE: &str = "HunterLauncher";

/// 当前可执行文件的绝对路径。拿不到就没法写自启项 —— 如实报错，不猜一个路径。
fn exe_path() -> AppResult<String> {
    let p = std::env::current_exe()
        .map_err(|e| AppError::new(Code::ConfigWrite, format!("读不到自身路径：{e}")))?;
    Ok(p.to_string_lossy().into_owned())
}

/// 自启项现在到底在不在（**读系统真实状态**，不读我们自己的配置文件）。
pub fn status() -> bool {
    #[cfg(target_os = "linux")]
    {
        desktop_file().exists()
    }
    #[cfg(target_os = "macos")]
    {
        plist_file().exists()
    }
    #[cfg(windows)]
    {
        crate::proc::run(
            "reg",
            &[
                "query",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                "/v",
                RUN_VALUE,
            ],
        )
        .map(|r| r.ok())
        .unwrap_or(false)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        false
    }
}

/// 打开或关闭自启。返回一句给用户看的说明（界面与 headless 共用）。
pub fn set(on: bool) -> AppResult<String> {
    if on {
        enable()
    } else {
        disable()
    }
}

fn enable() -> AppResult<String> {
    let exe = exe_path()?;

    #[cfg(target_os = "linux")]
    {
        let p = desktop_file();
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).map_err(|e| {
                AppError::new(Code::ConfigWrite, format!("建 {} 失败：{e}", d.display()))
            })?;
        }
        std::fs::write(&p, desktop_content(&exe)).map_err(|e| {
            AppError::new(Code::ConfigWrite, format!("写 {} 失败：{e}", p.display()))
        })?;
        return Ok(format!("已打开开机自启：{}", p.display()));
    }

    #[cfg(target_os = "macos")]
    {
        let p = plist_file();
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).map_err(|e| {
                AppError::new(Code::ConfigWrite, format!("建 {} 失败：{e}", d.display()))
            })?;
        }
        std::fs::write(&p, plist_content(&exe)).map_err(|e| {
            AppError::new(Code::ConfigWrite, format!("写 {} 失败：{e}", p.display()))
        })?;
        // 立刻加载，否则要等到下次登录才生效
        let _ = crate::proc::run("launchctl", &["load", "-w", &p.to_string_lossy()]);
        return Ok(format!("已打开开机自启：{}", p.display()));
    }

    #[cfg(windows)]
    {
        let value = format!("\"{exe}\" --minimized");
        let r = crate::proc::run(
            "reg",
            &[
                "add",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                "/v",
                RUN_VALUE,
                "/t",
                "REG_SZ",
                "/d",
                &value,
                "/f",
            ],
        )?;
        if !r.ok() {
            return Err(AppError::new(
                Code::ConfigWrite,
                format!("写注册表失败：{}", r.err_line()),
            ));
        }
        return Ok("已打开开机自启（注册表 HKCU\\...\\Run）".to_string());
    }

    #[allow(unreachable_code)]
    {
        let _ = exe;
        Err(AppError::new(
            Code::NotImplemented,
            "这个平台上启动器还不会设开机自启",
        ))
    }
}

fn disable() -> AppResult<String> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        #[cfg(target_os = "linux")]
        let p = desktop_file();
        #[cfg(target_os = "macos")]
        let p = plist_file();

        #[cfg(target_os = "macos")]
        if p.exists() {
            let _ = crate::proc::run("launchctl", &["unload", "-w", &p.to_string_lossy()]);
        }
        match std::fs::remove_file(&p) {
            Ok(()) => return Ok(format!("已关闭开机自启（删掉了 {}）", p.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok("开机自启本来就是关着的".to_string())
            }
            Err(e) => {
                return Err(AppError::new(
                    Code::ConfigWrite,
                    format!("删 {} 失败：{e}", p.display()),
                ))
            }
        }
    }

    #[cfg(windows)]
    {
        if !status() {
            return Ok("开机自启本来就是关着的".to_string());
        }
        let r = crate::proc::run(
            "reg",
            &[
                "delete",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                "/v",
                RUN_VALUE,
                "/f",
            ],
        )?;
        if !r.ok() {
            return Err(AppError::new(
                Code::ConfigWrite,
                format!("删注册表项失败：{}", r.err_line()),
            ));
        }
        return Ok("已关闭开机自启".to_string());
    }

    #[allow(unreachable_code)]
    Err(AppError::new(
        Code::NotImplemented,
        "这个平台上启动器还不会设开机自启",
    ))
}

// ── Linux ─────────────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
pub fn desktop_file() -> std::path::PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| crate::paths::home().join(".config"));
    base.join("autostart").join("hunter-launcher.desktop")
}

/// XDG 自启动条目。`X-GNOME-Autostart-enabled` 是给 GNOME 的开关，
/// `Terminal=false` 免得某些桌面给它开一个终端窗口。
#[cfg(target_os = "linux")]
pub fn desktop_content(exe: &str) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Hunter Launcher\n\
         Comment=Hunter 启动器（开机自启 · 只出托盘不弹窗）\n\
         Exec={exe} --minimized\n\
         Icon=hunter-launcher\n\
         Terminal=false\n\
         X-GNOME-Autostart-enabled=true\n"
    )
}

// ── macOS ─────────────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
pub fn plist_file() -> std::path::PathBuf {
    crate::paths::home()
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{APP_ID}.plist"))
}

#[cfg(target_os = "macos")]
pub fn plist_content(exe: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{APP_ID}</string>
  <key>ProgramArguments</key>
  <array><string>{exe}</string><string>--minimized</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><false/>
</dict>
</plist>
"#
    )
}

#[cfg(test)]
mod tests {
    #[allow(unused_imports)]
    use super::*;

    #[test]
    #[cfg(target_os = "linux")]
    fn desktop_文件内容带_minimized_且不经过_shell() {
        let c = desktop_content("/usr/bin/hunter-launcher");
        assert!(c.starts_with("[Desktop Entry]"));
        assert!(
            c.contains("Exec=/usr/bin/hunter-launcher --minimized"),
            "{c}"
        );
        assert!(c.contains("Terminal=false"));
        // 自启项里绝不该出现 shell 元字符拼接的痕迹
        assert!(!c.contains("sh -c"), "{c}");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn 自启文件走_xdg_config_home() {
        let old = std::env::var("XDG_CONFIG_HOME").ok();
        std::env::set_var("XDG_CONFIG_HOME", "/tmp/hunter-xdg-test");
        assert_eq!(
            desktop_file(),
            std::path::PathBuf::from("/tmp/hunter-xdg-test/autostart/hunter-launcher.desktop")
        );
        match old {
            Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn plist_是合法_xml_形状() {
        let c = plist_content("/Applications/Hunter Launcher.app/Contents/MacOS/hunter-launcher");
        assert!(c.contains("<key>RunAtLoad</key><true/>"));
        assert!(c.contains("--minimized"));
        assert!(c.contains(APP_ID));
    }

    /// 默认必须是关的（方案 §5.8）。这条测试在三个平台上都跑：
    /// 只要没人调过 `set(true)`，`status()` 就该是 false。
    #[test]
    fn 默认没有自启项() {
        // 注：CI 与开发机上都不会有人给这个程序装自启项；
        // 真有的话这条测试失败是对的 —— 说明环境脏了。
        assert!(!status(), "默认状态下不该存在自启项");
    }
}
