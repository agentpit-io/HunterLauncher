//! 子进程调用。**一律参数数组，不拼 shell 字符串**（总控规则红线 3）。
//!
//! 这个模块只暴露「给一个程序名 + 一个参数数组」的接口，
//! 连一个接受整串命令行的函数都不提供 —— 想拼字符串也没有地方拼。

use std::process::{Command, Output, Stdio};
use std::time::Duration;

use crate::err::{AppError, AppResult, Code};

#[derive(Debug)]
pub struct Ran {
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Ran {
    pub fn ok(&self) -> bool {
        self.status == Some(0)
    }
    /// 失败时给人看的一行说明（会进日志，所以由 AppError 那边统一脱敏）
    pub fn err_line(&self) -> String {
        let s = self.stderr.trim();
        let s = if s.is_empty() { self.stdout.trim() } else { s };
        s.lines().next_back().unwrap_or("（没有输出）").to_string()
    }
}

/// 跑一个程序并等它结束。程序**起不来**（没装 / 不在 PATH）与**跑了但失败**是两种不同的返回：
/// 前者 `Err`，后者 `Ok(Ran{status: Some(非 0)})`。M0 §5.4 的 Docker 判定就靠这个区分。
pub fn run(program: &str, args: &[&str]) -> AppResult<Ran> {
    run_with_env(program, args, &[])
}

pub fn run_with_env(program: &str, args: &[&str], env: &[(&str, &str)]) -> AppResult<Ran> {
    let mut cmd = base_command(program);
    cmd.args(args);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out: Output = cmd
        .stdin(Stdio::null())
        .output()
        .map_err(|e| AppError::new(Code::Unknown, format!("无法执行 {program}：{e}")))?;
    Ok(Ran {
        status: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

/// 带超时的版本。docker 在 daemon 半死不活时会一直挂着，不设超时会把界面卡住。
pub fn run_timeout(program: &str, args: &[&str], timeout: Duration) -> AppResult<Ran> {
    let mut child = base_command(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| AppError::new(Code::Unknown, format!("无法执行 {program}：{e}")))?;

    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(AppError::new(
                        Code::Unknown,
                        format!(
                            "{program} {} 超过 {} 秒没有返回",
                            args.join(" "),
                            timeout.as_secs()
                        ),
                    ));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                return Err(AppError::new(
                    Code::Unknown,
                    format!("等待 {program} 失败：{e}"),
                ))
            }
        }
    }
    let out = child
        .wait_with_output()
        .map_err(|e| AppError::new(Code::Unknown, format!("读取 {program} 输出失败：{e}")))?;
    Ok(Ran {
        status: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

/// 判断一个程序在不在 PATH 上。
pub fn exists(program: &str) -> bool {
    base_command(program)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

/// 建 Command。Windows 上要加 `CREATE_NO_WINDOW`，否则每调一次 docker 都会闪一个黑框。
pub fn base_command(program: &str) -> Command {
    let cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut cmd = cmd;
        cmd.creation_flags(CREATE_NO_WINDOW);
        return cmd;
    }
    #[cfg(not(windows))]
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 不存在的程序返回_err_而不是非零退出码() {
        let r = run("hunter-definitely-not-a-real-program", &["--version"]);
        assert!(
            r.is_err(),
            "起不来的程序必须是 Err，M0 §5.4 的判定依赖这一点"
        );
    }

    #[test]
    #[cfg(unix)]
    fn 跑得起来但失败的返回非零退出码() {
        let r = run("sh", &["-c", "exit 3"]).expect("sh 应当存在");
        assert_eq!(r.status, Some(3));
        assert!(!r.ok());
    }

    #[test]
    #[cfg(unix)]
    fn 参数是数组不经过_shell() {
        // 如果实现里偷偷拼了 shell 字符串，这里的 `;` 会被当成命令分隔符
        let r = run("echo", &["a; touch /tmp/hunter-should-not-exist"]).expect("echo 应当存在");
        assert!(
            r.stdout.contains("a; touch"),
            "参数被 shell 解释了：{}",
            r.stdout
        );
        assert!(!std::path::Path::new("/tmp/hunter-should-not-exist").exists());
    }

    #[test]
    #[cfg(unix)]
    fn 超时会把进程杀掉() {
        let r = run_timeout("sleep", &["10"], Duration::from_millis(300));
        assert!(r.is_err());
        assert!(r.unwrap_err().msg.contains("没有返回"));
    }
}
