//! Homebrew 路线（I8 · 兜底链的最后一环）。
//!
//! ## 为什么会有这一环
//!
//! 用户 2026-09-21 22:10 的原则：**没有 brew 也要替他装好**。
//! 0.1.7 撞上的那张卡片（「请在终端里执行：brew install --cask orbstack」）
//! 正是这条原则要消灭的东西 —— 那句话对不懂命令行的人等于「到此为止」。
//!
//! 所以这条路线是：**没有 brew 就先替他装 brew**，再用 brew 装 OrbStack。
//!
//! ## 装 Homebrew 这件事绕不开 sudo
//!
//! Homebrew 要往 `/opt/homebrew`（Apple 芯片）或 `/usr/local/*`（Intel）里建目录，
//! 那是系统位置。而且它**拒绝以 root 身份运行**，所以也不能整个塞进
//! `do shell script … with administrator privileges`。
//!
//! 官方脚本给的口子正好是任务书里点名的那一条 —— 它自己会看 `SUDO_ASKPASS`：
//!
//! ```text
//! if [[ -n "${SUDO_ASKPASS-}" ]]; then args=("-A" "${args[@]}"); fi
//! ```
//!
//! 于是我们把 `SUDO_ASKPASS` 指到 [`super::elevate::ensure_askpass`] 生成的小脚本上：
//! 需要密码的那一刻，macOS 弹一个对话框，密码由 `sudo` 直接读走，
//! **启动器进程不在这条链路上**，不读、不存、不写日志。
//!
//! ## 这条路线没有校验和可对，如实说明
//!
//! 安装脚本跟着 `HEAD` 走（这是 Homebrew 官方唯一提供的地址），
//! 所以**没有一个固定的 sha256 可以钉**。我们能做也确实做了的是：
//!
//! * 只从官方地址取（`raw.githubusercontent.com/Homebrew/install/HEAD/install.sh`，https）；
//! * 下回来先做**形状检查**（是不是一个 bash 脚本、里面有没有 Homebrew 自己的标记），
//!   不像就拒绝执行；
//! * 落盘位置固定在 `~/.hunter/runtime/cache/`，执行时过
//!   [`crate::assist::guard::argv_install_script`] 那道窄门。
//!
//! 内置运行时那条路线（默认）是有校验和的；这一条是**兜底**，两者的保证不一样，
//! 报告与界面上都要说清楚，不能含糊。

use std::time::Duration;

use crate::err::{AppError, AppResult, Code};

/// Homebrew 官方安装脚本。**只有这一个地址**。
pub const INSTALLER_URL: &str =
    "https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh";

/// 装 Homebrew 最多等多久。它可能要先装 Xcode 命令行工具（几百兆），给足 30 分钟。
const INSTALL_TIMEOUT: Duration = Duration::from_secs(1800);
/// `brew install --cask orbstack` 最多等多久。
const CASK_TIMEOUT: Duration = Duration::from_secs(1800);

/// 这台机器上的 brew 在哪。找不到就是 `None`。
pub fn brew_bin() -> Option<String> {
    if let Some(p) = super::which::resolve("brew").resolved {
        return Some(p);
    }
    // Apple 芯片与 Intel 的默认前缀不一样，GUI 程序的受限 PATH 里两个都没有
    for p in ["/opt/homebrew/bin/brew", "/usr/local/bin/brew"] {
        if std::path::Path::new(p).is_file() {
            return Some(p.to_string());
        }
    }
    None
}

/// 下回来的东西看着像不像 Homebrew 的安装脚本。
///
/// **纯函数**，CI 上能测。这不是校验和的替代品，是「拿到的东西明显不对时
/// 别执行它」的一道下限（代理插页、404 的 HTML、被截断的文件都会被它挡住）。
pub fn looks_like_installer(text: &str) -> Result<(), String> {
    if text.len() < 2000 {
        return Err(format!("只有 {} 字节，官方安装脚本没有这么短", text.len()));
    }
    if !text.starts_with("#!/bin/bash") {
        return Err("开头不是 `#!/bin/bash`".to_string());
    }
    for marker in ["HOMEBREW_PREFIX", "Homebrew", "NONINTERACTIVE"] {
        if !text.contains(marker) {
            return Err(format!("里面找不到 `{marker}` 这个标记"));
        }
    }
    if text.contains("<html") || text.contains("<HTML") {
        return Err("拿回来的是一个网页，不是脚本".to_string());
    }
    Ok(())
}

/// 替用户装 Homebrew。**这一步可能会弹系统密码框**，调用方要先说一句人话。
pub fn install_homebrew(say: &mut dyn FnMut(&str), cancel: &dyn Fn() -> bool) -> AppResult<String> {
    if !cfg!(target_os = "macos") {
        return Err(AppError::new(
            Code::NotImplemented,
            "Homebrew 这条路线只做了 macOS。".to_string(),
        ));
    }
    if let Some(b) = brew_bin() {
        return Ok(format!(
            "这台 Mac 上已经有 Homebrew 了（{}），不用再装",
            crate::redact::mask_home(&b)
        ));
    }
    std::fs::create_dir_all(crate::paths::runtime_cache()).ok();
    let script = crate::paths::runtime_cache().join("homebrew-install.sh");
    crate::assist::guard::writable_path(&script)?;

    say("正在从 Homebrew 官方地址下载安装脚本");
    let mut noop = |_: u64, _: Option<u64>| {};
    crate::http::download_to_file(
        INSTALLER_URL,
        &script,
        Duration::from_secs(120),
        2 * 1024 * 1024,
        cancel,
        &mut noop,
    )?;
    let text = std::fs::read_to_string(&script)
        .map_err(|e| AppError::new(Code::Unknown, format!("读下回来的安装脚本失败：{e}")))?;
    if let Err(why) = looks_like_installer(&text) {
        let _ = std::fs::remove_file(&script);
        return Err(AppError::new(
            Code::Unknown,
            format!("从官方地址下回来的东西不像 Homebrew 的安装脚本（{why}），已删掉，不会执行。"),
        ));
    }
    say(&format!(
        "安装脚本已下载（{} 字节，来自 {}）",
        text.len(),
        crate::http::host_of(INSTALLER_URL)
    ));

    // 需要密码的那一步：系统弹框，密码直接给 sudo
    let askpass = super::elevate::ensure_askpass(
        "需要管理员密码",
        "Hunter 启动器正在替你安装 Homebrew（装 Docker 要用它）。\
         Homebrew 要往系统目录里建文件夹，所以要你的开机密码。\
         密码只交给系统的 sudo，启动器看不到，也不会保存。",
    )?;

    let argv = vec![
        "/bin/bash".to_string(),
        script.to_string_lossy().into_owned(),
    ];
    // **窄门**：只允许「/bin/bash + 一个落在 ~/.hunter/runtime 里的脚本」
    crate::assist::guard::argv_install_script(&argv)?;
    crate::assist::guard::audit(
        "install_homebrew",
        &std::collections::BTreeMap::from([("script".to_string(), argv[1].clone())]),
        crate::assist::guard::Proposer::Orchestrator,
        Some(crate::assist::guard::Level::Sensitive),
        "开始执行官方安装脚本（NONINTERACTIVE=1）",
    );

    say("正在安装 Homebrew（官方脚本，通常几分钟；需要时会弹出系统密码框）");
    let askpass_s = askpass.to_string_lossy().into_owned();
    let mut env: Vec<(String, String)> = vec![
        // 非交互：不等回车、不开分页器
        ("NONINTERACTIVE".to_string(), "1".to_string()),
        ("SUDO_ASKPASS".to_string(), askpass_s),
        ("HOMEBREW_NO_ANALYTICS".to_string(), "1".to_string()),
        ("HOMEBREW_NO_AUTO_UPDATE".to_string(), "1".to_string()),
    ];
    // 沿用用户的代理（Homebrew 自己也要从 GitHub 拉东西）
    env.extend(crate::netproxy::current().env_pairs());
    let pairs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let r = crate::proc::run_timeout_env("/bin/bash", &[&argv[1]], INSTALL_TIMEOUT, &pairs)?;
    let _ = std::fs::remove_file(&script);
    if !r.ok() {
        // **原话交给诊断员**，不改写
        return Err(AppError::new(
            Code::Unknown,
            format!(
                "装 Homebrew 没成功（退出码 {:?}）：{}",
                r.status,
                tail(&format!("{}\n{}", r.stdout, r.stderr), 600)
            ),
        ));
    }
    super::which::invalidate();
    let Some(b) = brew_bin() else {
        return Err(AppError::new(
            Code::Unknown,
            "安装脚本跑完了，但这台机器上还是找不到 brew —— 不当成成功。".to_string(),
        ));
    };
    Ok(format!(
        "Homebrew 已装好（{}）",
        crate::redact::mask_home(&b)
    ))
}

/// `brew install --cask orbstack`。
///
/// 参数是**写死的**（不是模型给的 formula 名），照样过一遍 [`crate::assist::guard::argv`]。
pub fn install_orbstack_cask(say: &mut dyn FnMut(&str)) -> AppResult<String> {
    let Some(brew) = brew_bin() else {
        return Err(AppError::new(
            Code::NotImplemented,
            "这台机器上没有 brew。".to_string(),
        ));
    };
    let argv = vec![
        brew.clone(),
        "install".to_string(),
        "--cask".to_string(),
        "orbstack".to_string(),
    ];
    crate::assist::guard::argv(&argv)?;
    say("正在用 Homebrew 安装 OrbStack（下载量约 200 MB；需要时会弹出系统密码框）");
    let mut env: Vec<(String, String)> = vec![
        ("NONINTERACTIVE".to_string(), "1".to_string()),
        ("HOMEBREW_NO_ANALYTICS".to_string(), "1".to_string()),
        ("HOMEBREW_NO_AUTO_UPDATE".to_string(), "1".to_string()),
    ];
    // cask 安装偶尔要管理员权限（往 /Applications 里写），同样走系统密码框
    if let Ok(p) = super::elevate::ensure_askpass(
        "需要管理员密码",
        "Hunter 启动器正在替你安装 OrbStack。装到「应用程序」文件夹里要管理员权限。\
         密码只交给系统的 sudo，启动器看不到，也不会保存。",
    ) {
        env.push(("SUDO_ASKPASS".to_string(), p.to_string_lossy().into_owned()));
    }
    env.extend(crate::netproxy::current().env_pairs());
    let pairs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let args: Vec<&str> = argv[1..].iter().map(String::as_str).collect();
    let r = crate::proc::run_timeout_env(&brew, &args, CASK_TIMEOUT, &pairs)?;
    if !r.ok() {
        return Err(AppError::new(
            Code::Unknown,
            format!(
                "brew install --cask orbstack 没成功（退出码 {:?}）：{}",
                r.status,
                tail(&format!("{}\n{}", r.stdout, r.stderr), 600)
            ),
        ));
    }
    Ok("OrbStack 已经用 Homebrew 装好了".to_string())
}

/// 取一段输出的末尾若干字符（报错时给现场，但不把几千行日志塞进界面）。
fn tail(s: &str, n: usize) -> String {
    let t = s.trim();
    let count = t.chars().count();
    if count <= n {
        return t.to_string();
    }
    format!("…{}", t.chars().skip(count - n).collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 官方地址是_https_而且只有一个() {
        assert!(INSTALLER_URL.starts_with("https://raw.githubusercontent.com/Homebrew/install/"));
    }

    /// 形状检查要能挡住「拿回来的不是脚本」的几种真实情况。
    #[test]
    fn 不像安装脚本的东西一律不执行() {
        let good = format!(
            "#!/bin/bash\n{}\nHOMEBREW_PREFIX=/opt/homebrew\nNONINTERACTIVE\nHomebrew\n",
            "# ".repeat(1200)
        );
        assert!(looks_like_installer(&good).is_ok());

        // 404 页面 / 代理插页
        let html = format!("<html>{}</html>", "x".repeat(3000));
        assert!(looks_like_installer(&html).is_err());
        // 被截断
        assert!(looks_like_installer("#!/bin/bash\necho hi\n").is_err());
        // 是个脚本但不是 Homebrew 的
        let other = format!("#!/bin/bash\n{}\n", "echo x\n".repeat(400));
        assert!(looks_like_installer(&other).is_err());
    }

    #[test]
    fn 末尾截断保留的是末尾() {
        let s = "abcdefghij";
        assert_eq!(tail(s, 100), s);
        assert_eq!(tail(s, 3), "…hij");
    }

    /// 非 macOS 上这条路线**如实拒绝**，不半途而废。
    #[test]
    fn 非_mac_上不装_homebrew() {
        if !cfg!(target_os = "macos") {
            let mut say = |_: &str| {};
            let no = || false;
            let e = install_homebrew(&mut say, &no).expect_err("非 mac 上要拒绝");
            assert!(e.msg.contains("macOS"), "{}", e.msg);
        }
    }
}
