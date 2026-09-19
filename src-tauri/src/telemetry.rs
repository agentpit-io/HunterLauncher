//! 匿名遥测的**本地队列**（技术方案 §12、里程碑 M3 第 6 项）。
//!
//! ## 这个模块现在做什么、不做什么
//!
//! M0 实测 `telemetry.agentpit.io` **不存在**（DNS 都解析不出来），所以本轮**不做上报**：
//! 事件只往 `~/.hunter/telemetry/queue.jsonl` 里追加，上报地址做成可配置
//! （`launcher.toml` 的 `telemetry.endpoint`）但**默认是空**。
//! 界面上如实写「暂未开启上报」，不做「已发送」的假象（总控规则红线 1）。
//!
//! 队列文件就是方案 §12.1「透明」那一条的落地：设置页的「查看将要发送的数据」
//! 读的就是这个文件的原文，一个字都不多、不少。
//!
//! ## 三条硬约束
//!
//! 1. **开关默认关**。`enabled == false` 时 [`record`] 直接返回，一个字节都不写；
//!    关闭开关的那一刻调 [`clear`] 把队列删掉（方案 §11.2「关闭时本地队列清空」）。
//! 2. **只收计数与枚举，不收自由文本**。字段值在 [`Field`] 这一层就被限死成
//!    「字符串枚举 / 整数 / 布尔」，而且每个字符串值还要过 [`sanitize_value`]：
//!    超长、含空格、含中文的一律拒收 —— 自由文本根本塞不进来（方案 §12.1「最小化」）。
//! 3. **写盘前过脱敏**。和 `log::write_line` 一样，[`append_line`] 是唯一出口，
//!    每一行都先过 `redact::redact`。key 进不了队列（红线 2）。
//!
//! ## 事件表（方案 §12.2 中启动器侧拿得到的那些）
//!
//! | 事件 | 何时 | 字段 |
//! |---|---|---|
//! | `launcher_start` | 启动器进程起来 | version, os, arch, locale |
//! | `docker_detected` | Docker 检测完成 | runtime, version, ok |
//! | `setup_step` | 向导每一步 | step, outcome, error_code, duration_ms |
//! | `pull_done` | 拉取结束 | registry, total_mb, duration_ms, retries |
//! | `hunter_started` | 六服务就绪 | hunter_tag, duration_ms, ports_remapped |
//! | `error` | 出错 | component, error_code |
//! | `update` | 升级 | from_tag, to_tag, outcome |
//!
//! 方案里的 `hunter_daily` 与 `skill_usage` **本轮没有**：它们要由上游 api
//! 本地聚合后交给启动器转发，而 M0/M3 实测 api 上没有 `/api/system/metrics/daily`
//! 这类接口（见成果文档「需上游配合」）。没有数据来源就不编一个（红线 1）。

use std::fmt::Write as _;
use std::io::Write as _;

use crate::err::{AppError, AppResult, Code};
use crate::paths;

/// 队列文件的上限。超过就把最老的一半丢掉 —— 本地队列没人消费，
/// 不设上限的话跑上一年会变成一个几十 MB 的文件。
const MAX_LINES: usize = 2000;

/// 一个字段值。**刻意只有这三种**：字符串枚举、整数、布尔。
/// 没有「任意 JSON」这一档，自由文本、股票代码、对话内容在类型层面就进不来。
#[derive(Debug, Clone)]
pub enum Field {
    /// 枚举值（`ghcr`、`ok`、`E_PULL_FAILED` 这种）。会过 [`sanitize_value`]
    Enum(String),
    Int(i64),
    Bool(bool),
}

impl Field {
    fn to_json(&self) -> String {
        match self {
            Field::Enum(s) => format!("{:?}", sanitize_value(s)),
            Field::Int(n) => n.to_string(),
            Field::Bool(b) => b.to_string(),
        }
    }
}

/// 枚举值的上限长度。`hkccr.ccs.tencentyun.com/agentpit` 是现有最长的合法值（33 字符）。
const MAX_VALUE_LEN: usize = 48;

/// 枚举值的清洗。这是「不收自由文本」的**兜底闸门**（方案 §12.1 最小化）。
///
/// 规则是**整条拒收**而不是逐字符过滤 —— 这一点是写完第一版之后改的：
/// 第一版只把空格和中文滤掉，结果「我在 600519 上试了半天，报错说 connection refused」
/// 会被压成 `600519connectionrefused`，**股票代码原样留了下来**。
/// 逐字符过滤永远拦不住这种事，因为自由文本里最敏感的部分（数字、代码）恰好都是合法字符。
///
/// 所以：真正的枚举值不可能带空白、不可能有非 ASCII、不可能超过 48 字符。
/// 只要沾上这三条里的任何一条，整个值直接变成 `invalid`，一个字符都不留。
pub fn sanitize_value(s: &str) -> String {
    let s = s.trim();
    if s.is_empty()
        || s.len() > MAX_VALUE_LEN
        || !s.is_ascii()
        || s.chars().any(|c| c.is_ascii_whitespace())
    {
        return "invalid".to_string();
    }
    // 剩下的只可能是短 ASCII 串了，再把控制字符与引号之类剔掉（JSON 转义的隐患）
    let cleaned: String = s
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':' | '/'))
        .collect();
    if cleaned.is_empty() {
        "invalid".to_string()
    } else {
        cleaned
    }
}

/// 事件名的白名单。不在表里的名字一律拒收 —— 免得日后谁随手加一个事件
/// 却忘了在设置页的说明里写上它（方案 §12.1「透明」）。
pub const EVENTS: [&str; 7] = [
    "launcher_start",
    "docker_detected",
    "setup_step",
    "pull_done",
    "hunter_started",
    "error",
    "update",
];

/// 记一条事件。**开关关着就什么都不做**。
///
/// 返回 `true` 表示真写进去了，`false` 表示被开关或白名单挡下 —— 调用方一般不关心，
/// 单元测试关心。
pub fn record(enabled: bool, install_id: &str, event: &str, fields: &[(&str, Field)]) -> bool {
    if !enabled {
        return false;
    }
    if !EVENTS.contains(&event) {
        crate::lwarn!("遥测事件 {event} 不在白名单里，已丢弃");
        return false;
    }
    let mut line = String::new();
    let _ = write!(
        line,
        "{{\"ts\":{:?},\"install_id\":{:?},\"event\":{:?}",
        crate::timefmt::now_shanghai(),
        sanitize_value(install_id),
        event
    );
    for (k, v) in fields {
        let _ = write!(line, ",{:?}:{}", sanitize_value(k), v.to_json());
    }
    line.push('}');
    if let Err(e) = append_line(&line) {
        crate::lwarn!("写遥测队列失败：{}", e.msg);
        return false;
    }
    true
}

/// 唯一的写盘出口。**每一行先过脱敏**（红线 2），再追加。
fn append_line(line: &str) -> AppResult<()> {
    let safe = crate::redact::redact(line);
    // 脱敏后如果多出了换行（不该发生，但万一），压成一行，jsonl 一行一条是硬约定
    let safe = safe.replace(['\n', '\r'], " ");
    std::fs::create_dir_all(paths::telemetry_dir())
        .map_err(|e| AppError::new(Code::ConfigWrite, format!("建遥测目录失败：{e}")))?;
    let p = paths::telemetry_queue();
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p)
        .map_err(|e| AppError::new(Code::ConfigWrite, format!("打开 {} 失败：{e}", p.display())))?;
    writeln!(f, "{safe}")
        .map_err(|e| AppError::new(Code::ConfigWrite, format!("写 {} 失败：{e}", p.display())))?;
    drop(f);
    trim_if_too_long();
    Ok(())
}

fn trim_if_too_long() {
    let p = paths::telemetry_queue();
    let Ok(s) = std::fs::read_to_string(&p) else {
        return;
    };
    let lines: Vec<&str> = s.lines().collect();
    if lines.len() <= MAX_LINES {
        return;
    }
    let keep = &lines[lines.len() - MAX_LINES / 2..];
    let _ = std::fs::write(&p, format!("{}\n", keep.join("\n")));
}

/// 队列原文。设置页的「查看将要发送的数据」直接显示这个结果。
pub fn queue_lines() -> Vec<String> {
    std::fs::read_to_string(paths::telemetry_queue())
        .map(|s| s.lines().map(|l| l.to_string()).collect())
        .unwrap_or_default()
}

/// 清空队列。**关闭开关时必须调**（方案 §11.2）。
pub fn clear() -> AppResult<()> {
    let p = paths::telemetry_queue();
    match std::fs::remove_file(&p) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(AppError::new(
            Code::ConfigWrite,
            format!("删除 {} 失败：{e}", p.display()),
        )),
    }
}

/// 生成一个 install_id（随机 UUID v4 的形状，本地生成、与 key 无关，方案 §12.1）。
pub fn new_install_id() -> String {
    let mut b = [0u8; 16];
    if getrandom::fill(&mut b).is_err() {
        // 拿不到系统随机源时不要退回一个可预测的值，宁可不生成
        return String::new();
    }
    b[6] = (b[6] & 0x0f) | 0x40; // version 4
    b[8] = (b[8] & 0x3f) | 0x80; // variant 1
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 这些测试要往临时的 `HUNTER_HOME` 里写文件，进程级环境变量得串行改。
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        use std::sync::{Mutex, OnceLock};
        static L: OnceLock<Mutex<()>> = OnceLock::new();
        L.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    struct TempHome {
        old: Option<String>,
        dir: std::path::PathBuf,
    }

    impl TempHome {
        fn new(tag: &str) -> Self {
            let old = std::env::var("HUNTER_HOME").ok();
            let dir = std::env::temp_dir().join(format!(
                "hunter-telemetry-{tag}-{}-{}",
                std::process::id(),
                crate::timefmt::now_unix()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::env::set_var("HUNTER_HOME", &dir);
            Self { old, dir }
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
            match self.old.take() {
                Some(v) => std::env::set_var("HUNTER_HOME", v),
                None => std::env::remove_var("HUNTER_HOME"),
            }
        }
    }

    #[test]
    fn 开关关着一个字节都不写() {
        let _g = env_lock();
        let _h = TempHome::new("off");
        assert!(!record(false, "abc", "launcher_start", &[]));
        assert!(
            !paths::telemetry_queue().exists(),
            "开关关着的时候连文件都不该建出来"
        );
        assert!(queue_lines().is_empty());
    }

    #[test]
    fn 开着才写并且是合法的_jsonl() {
        let _g = env_lock();
        let _h = TempHome::new("on");
        assert!(record(
            true,
            "11111111-2222-4333-8444-555555555555",
            "pull_done",
            &[
                ("registry", Field::Enum("tencent".into())),
                ("total_mb", Field::Int(849)),
                ("retries", Field::Int(0)),
                ("ports_remapped", Field::Bool(true)),
            ]
        ));
        let lines = queue_lines();
        assert_eq!(lines.len(), 1);
        let v: serde_json::Value = serde_json::from_str(&lines[0]).expect("必须是合法 JSON");
        assert_eq!(v["event"], "pull_done");
        assert_eq!(v["registry"], "tencent");
        assert_eq!(v["total_mb"], 849);
        assert_eq!(v["ports_remapped"], true);
        assert_eq!(v["install_id"], "11111111-2222-4333-8444-555555555555");
        assert!(v["ts"].as_str().unwrap().starts_with("20"));
    }

    #[test]
    fn 白名单外的事件名被拒() {
        let _g = env_lock();
        let _h = TempHome::new("wl");
        assert!(!record(true, "x", "偷偷加的事件", &[]));
        assert!(queue_lines().is_empty());
    }

    #[test]
    fn 自由文本塞不进来() {
        let _g = env_lock();
        let _h = TempHome::new("free");
        // 假装有人把一整句用户描述当成枚举值传进来
        record(
            true,
            "x",
            "error",
            &[(
                "component",
                Field::Enum("我在 600519 上试了半天，报错说 connection refused".into()),
            )],
        );
        let v: serde_json::Value = serde_json::from_str(&queue_lines()[0]).unwrap();
        let c = v["component"].as_str().unwrap();
        // 整条拒收，不是逐字符过滤 —— 逐字符过滤会留下 `600519connectionrefused`
        assert_eq!(c, "invalid", "自由文本必须整条变成 invalid，实际是 {c}");
    }

    #[test]
    fn 合法的枚举值不会被误伤() {
        for v in [
            "ghcr",
            "hkccr.ccs.tencentyun.com/agentpit",
            "DockerEngine",
            "29.8.1",
            "zh-CN",
            "E_PULL_FAILED",
            "1.2.0",
            "x86_64",
        ] {
            assert_eq!(sanitize_value(v), v, "合法枚举值被改坏了：{v}");
        }
    }

    #[test]
    fn 三种脏值都整条拒收() {
        assert_eq!(sanitize_value("有中文"), "invalid");
        assert_eq!(sanitize_value("带 空格"), "invalid");
        assert_eq!(sanitize_value(&"a".repeat(49)), "invalid");
        assert_eq!(sanitize_value(""), "invalid");
        // 恰好 48 个字符仍然放行
        assert_eq!(sanitize_value(&"a".repeat(48)), "a".repeat(48));
    }

    #[test]
    fn 队列里找不到_key() {
        let _g = env_lock();
        let _h = TempHome::new("key");
        let key = "hunt_tools_AbCdEfGhIjKlMnOpQrStUvWxYz012345";
        crate::redact::register_secret(key);
        // 就算调用方犯傻把 key 当成枚举值传进来，写到盘上也必须是打码的
        record(
            true,
            key,
            "error",
            &[("component", Field::Enum(key.into()))],
        );
        let raw = std::fs::read_to_string(paths::telemetry_queue()).unwrap();
        assert!(!raw.contains(key), "队列里出现了整把 key");
        assert!(
            !raw.contains("AbCdEfGhIjKl"),
            "队列里出现了 key 的片段：{raw}"
        );
    }

    #[test]
    fn 关闭开关会把队列删干净() {
        let _g = env_lock();
        let _h = TempHome::new("clear");
        record(true, "x", "launcher_start", &[]);
        assert_eq!(queue_lines().len(), 1);
        clear().expect("清空应当成功");
        assert!(!paths::telemetry_queue().exists());
        assert!(queue_lines().is_empty());
        // 再清一次不能报错（文件本来就不在）
        clear().expect("重复清空也应当成功");
    }

    #[test]
    fn install_id_是_uuid_v4_的形状() {
        let id = new_install_id();
        assert_eq!(id.len(), 36, "{id}");
        assert_eq!(id.as_bytes()[14], b'4', "版本位应当是 4：{id}");
        assert!(
            matches!(id.as_bytes()[19], b'8' | b'9' | b'a' | b'b'),
            "{id}"
        );
        assert_ne!(id, new_install_id(), "两次生成不该一样");
    }
}
