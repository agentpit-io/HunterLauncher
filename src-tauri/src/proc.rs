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
    run_timeout_env(program, args, timeout, &[])
}

/// 带超时 **且带额外环境变量** 的版本（I7 的内置运行时要给 colima / docker
/// 传 `LIMA_HOME` / `COLIMA_HOME` / `DOCKER_HOST`）。
///
/// 这些变量**只加在这一次调用上**，不写进进程环境 —— 启动器自己的环境里
/// 出现 `DOCKER_HOST` 会把「用不用内置运行时」这件事变成全局隐式状态，
/// 那正是最难查的一类 bug。
pub fn run_timeout_env(
    program: &str,
    args: &[&str],
    timeout: Duration,
    env: &[(&str, &str)],
) -> AppResult<Ran> {
    let mut cmd = base_command(program);
    cmd.args(args);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| AppError::new(Code::Unknown, format!("无法执行 {program}：{e}")))?;

    // **必须一边跑一边读**（I7 实测撞出来的）。
    //
    // 管道的内核缓冲区只有 64 KB 左右。原先的写法是「先等它退出，再
    // `wait_with_output()` 把输出读出来」—— 子进程写满 64 KB 之后就阻塞在 `write` 上
    // 永远不退出，而我们在等它退出。**双方互等，只能靠超时收场。**
    //
    // 现场：`docker compose logs --tail 200` 对着一套 6 个容器的栈
    // （postgres 的日志行很长），60 秒超时报「超过 60 秒没有返回」；
    // 同一条命令在终端里 0.24 秒就跑完了。
    //
    // 这不是 I7 才有的问题 —— `compose::logs` 走的是同一个函数，
    // 只是我们自己那一套的日志一直没满过 64 KB。
    let mut out_pipe = child.stdout.take();
    let mut err_pipe = child.stderr.take();
    let h_out = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(p) = out_pipe.as_mut() {
            let _ = std::io::Read::read_to_end(p, &mut buf);
        }
        buf
    });
    let h_err = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(p) = err_pipe.as_mut() {
            let _ = std::io::Read::read_to_end(p, &mut buf);
        }
        buf
    });

    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    // 杀掉它，两个读线程就会拿到 EOF 结束 —— 否则 join 会挂住
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = h_out.join();
                    let _ = h_err.join();
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
                let _ = child.kill();
                let _ = h_out.join();
                let _ = h_err.join();
                return Err(AppError::new(
                    Code::Unknown,
                    format!("等待 {program} 失败：{e}"),
                ));
            }
        }
    };
    let stdout = h_out.join().unwrap_or_default();
    let stderr = h_err.join().unwrap_or_default();
    Ok(Ran {
        status: status.code(),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
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

/// 建 Command。**这是全项目唯一建子进程的地方**，两件事在这里统一做掉：
///
/// 1. Windows 上加 `CREATE_NO_WINDOW`，否则每调一次 docker 都会闪一个黑框；
/// 2. 套上 [`crate::runtime::env`] 算出来的环境 —— PATH 补全 + 必要时的
///    `DOCKER_CONFIG`。**I6 的 P0 就修在这一行**：I4 解决了「启动器找得到 docker」，
///    但 docker 自己去 `$PATH` 上找 `docker-credential-osxkeychain` 时用的是
///    子进程继承的那份受限 PATH，照样找不到（用户 Mac 上 0.1.5 的现场）。
pub fn base_command(program: &str) -> Command {
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    crate::runtime::env::apply(&mut cmd);
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

    /// **输出超过管道缓冲区（64 KB）时不许挂住。**
    ///
    /// I7 实测撞出来的：原先的写法先等进程退出、再 `wait_with_output()` 读输出 ——
    /// 子进程写满 64 KB 之后阻塞在 `write` 上，而我们在等它退出，双方互等。
    /// 现场是 `docker compose logs --tail 200` 对着一套 6 个容器的栈。
    ///
    /// 这条测试造 1 MB 输出（远超缓冲区），限时 20 秒 —— 修之前必然超时。
    #[test]
    fn 输出超过管道缓冲区也不会挂住() {
        // 1 MB：`yes` 打 16 个字符一行，打 65536 行
        let r = run_timeout(
            "/usr/bin/env",
            &[
                "sh",
                "-c",
                "i=0; while [ $i -lt 65536 ]; do echo 0123456789abcde; i=$((i+1)); done",
            ],
            Duration::from_secs(20),
        );
        // 这台机器上没有 /usr/bin/env 或 sh 的话跳过（Windows）
        let Ok(r) = r else {
            if cfg!(windows) {
                return;
            }
            panic!("起不来：{:?}", r.err().map(|e| e.msg));
        };
        assert_eq!(r.status, Some(0));
        assert!(
            r.stdout.len() > 1_000_000,
            "只读到 {} 字节，管道那一头被截断了",
            r.stdout.len()
        );
        assert_eq!(r.stdout.lines().count(), 65536);
    }

    /// stderr 那一路同样要一边跑一边读。
    #[test]
    fn stderr_超过缓冲区也不会挂住() {
        let r = run_timeout(
            "/usr/bin/env",
            &[
                "sh",
                "-c",
                "i=0; while [ $i -lt 65536 ]; do echo 0123456789abcde >&2; i=$((i+1)); done",
            ],
            Duration::from_secs(20),
        );
        let Ok(r) = r else {
            if cfg!(windows) {
                return;
            }
            panic!("起不来");
        };
        assert_eq!(r.status, Some(0));
        assert!(r.stderr.len() > 1_000_000, "只读到 {} 字节", r.stderr.len());
    }
}
