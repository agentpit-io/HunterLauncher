//! `~/.hunter/logs/launcher.log` 的滚动日志（技术方案 §7：5 × 5 MB）。
//!
//! **每一行都先过 [`crate::redact::redact`]**（总控规则红线 2：key 不进日志）。
//! 这不是调用方的自觉，而是写盘路径上的硬约束 —— `write_line` 是唯一的出口，
//! 想绕过它只能不用这个模块。对应的单元测试见本文件末尾。

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use crate::redact::redact;

const MAX_BYTES: u64 = 5 * 1024 * 1024;
const MAX_FILES: usize = 5;

struct Inner {
    path: PathBuf,
    file: Option<File>,
    written: u64,
    /// 最近若干行，给界面的「启动器日志」面板用（已脱敏）
    tail: Vec<String>,
}

fn inner() -> &'static Mutex<Option<Inner>> {
    static L: OnceLock<Mutex<Option<Inner>>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(None))
}

/// 初始化日志。重复调用会切到新路径（测试里会用到）。
pub fn init(path: PathBuf) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let written = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok();
    if let Ok(mut g) = inner().lock() {
        *g = Some(Inner {
            path,
            file,
            written,
            tail: Vec::new(),
        });
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
    Error,
}

impl Level {
    fn tag(self) -> &'static str {
        match self {
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
        }
    }
}

/// 写一行日志。返回写进去的那一行（已脱敏），方便调用方顺手回显。
pub fn write_line(level: Level, msg: &str) -> String {
    let line = format!(
        "{} [{}] {}",
        crate::timefmt::now_shanghai(),
        level.tag(),
        redact(msg)
    );
    if let Ok(mut g) = inner().lock() {
        if let Some(st) = g.as_mut() {
            st.tail.push(line.clone());
            if st.tail.len() > 400 {
                st.tail.drain(..st.tail.len() - 400);
            }
            if let Some(f) = st.file.as_mut() {
                let _ = writeln!(f, "{line}");
                let _ = f.flush();
                st.written += line.len() as u64 + 1;
            }
            if st.written >= MAX_BYTES {
                rotate(st);
            }
        }
    }
    line
}

/// launcher.log → launcher.log.1 → … → launcher.log.4，最老的删掉。
fn rotate(st: &mut Inner) {
    st.file = None;
    for i in (1..MAX_FILES).rev() {
        let from = with_suffix(&st.path, i);
        let to = with_suffix(&st.path, i + 1);
        if from.exists() {
            if i + 1 >= MAX_FILES {
                let _ = std::fs::remove_file(&from);
            } else {
                let _ = std::fs::rename(&from, &to);
            }
        }
    }
    let _ = std::fs::rename(&st.path, with_suffix(&st.path, 1));
    st.file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&st.path)
        .ok();
    st.written = 0;
}

fn with_suffix(path: &std::path::Path, n: usize) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(format!(".{n}"));
    PathBuf::from(s)
}

/// 界面「启动器日志」面板用的最近 n 行（已脱敏）。
pub fn tail(n: usize) -> Vec<String> {
    inner()
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|st| st.tail.clone()))
        .map(|mut v| {
            if v.len() > n {
                v.drain(..v.len() - n);
            }
            v
        })
        .unwrap_or_default()
}

#[macro_export]
macro_rules! linfo {
    ($($arg:tt)*) => {{ let _ = $crate::log::write_line($crate::log::Level::Info, &format!($($arg)*)); }};
}
#[macro_export]
macro_rules! lwarn {
    ($($arg:tt)*) => {{ let _ = $crate::log::write_line($crate::log::Level::Warn, &format!($($arg)*)); }};
}
#[macro_export]
macro_rules! lerror {
    ($($arg:tt)*) => {{ let _ = $crate::log::write_line($crate::log::Level::Error, &format!($($arg)*)); }};
}

/// 全局 logger 是**进程级**的单例（生产里本来就该如此：一个进程一份日志）。
/// 测试默认并行跑，两条测试各自 `init()` 到不同路径会互相把对方的日志抢走，
/// 所以凡是碰全局 logger 的测试都要先拿这把锁。
#[cfg(test)]
pub fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    // 上一条测试 panic 时锁会中毒，这里不在乎，拿到里面的值继续用
    L.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 编造的假 key，不是测试机上那把真的（红线 2）。
    const FAKE: &str = "hunt_tools_q6sKaaaaaaaaaaaaaaaaaaaaaaaaQMo2";

    fn tmp(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("hunter-log-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        p.push("launcher.log");
        p
    }

    /// 这是总控规则红线 2 在 Rust 侧的看门测试：**落到盘上的日志文件里不能出现 key**。
    #[test]
    fn 落盘的日志里找不到_key() {
        let _g = test_lock();
        let path = tmp("nokey");
        init(path.clone());
        crate::redact::register_secret(FAKE);
        write_line(Level::Info, &format!("写 .env：LLM_API_KEY={FAKE}"));
        write_line(
            Level::Error,
            &format!("网关拒绝了 Authorization: Bearer {FAKE}"),
        );
        write_line(Level::Info, "docker compose --project-name hunter up -d");

        let content = std::fs::read_to_string(&path).expect("日志文件应当存在");
        assert!(!content.contains(FAKE), "日志里出现了整把 key：\n{content}");
        assert!(
            !content.contains("q6sK"),
            "日志里出现了 key 的片段：\n{content}"
        );
        assert!(
            content.contains("hunt_tools_****"),
            "应当留下可辨识的打码痕迹：\n{content}"
        );
        assert!(content.contains("up -d"), "正常内容不该被吃掉：\n{content}");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn 每行都带上海时间与级别() {
        let _g = test_lock();
        let path = tmp("fmt");
        init(path.clone());
        let line = write_line(Level::Warn, "端口 3100 被占用，改用 3101");
        assert!(line.contains("[warn]"), "{line}");
        assert!(line.contains("端口 3100 被占用"), "{line}");
        // 形如 2026-09-19 21:30:01
        assert_eq!(line.as_bytes()[4], b'-', "{line}");
        assert_eq!(line.as_bytes()[13], b':', "{line}");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn tail_返回最近的行() {
        let _g = test_lock();
        let path = tmp("tail");
        init(path.clone());
        for i in 0..10 {
            write_line(Level::Info, &format!("第 {i} 行"));
        }
        let t = tail(3);
        assert_eq!(t.len(), 3);
        assert!(t[2].contains("第 9 行"), "{:?}", t);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
