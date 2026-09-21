//! 实时过程流的事件（I5 · 设计文档 §七）。
//!
//! 后端把「正在干什么」一条条推给界面，界面按 `parent` 组成一棵树：
//!
//! ```text
//! ✅ 检查 Docker            Docker 29.4.0 正在运行
//! ⚠️ 发现问题              你电脑上已经在运行另一套 Hunter，占用了 5 个端口
//!    🔎 分析中…            这 5 个端口属于 hunter-community，不能动它
//!    🔧 正在处理…          为新安装换到空闲端口：3101 / 8101 / 3922 / 5443 / 6480
//!    ✔ 已解决              新旧两套可以同时运行，互不影响
//! ✅ 启动服务              6 / 6 健康
//! ```
//!
//! ## 两条硬规矩
//!
//! 1. **数字只来自真实值**（红线 1）。事件里的耗时、端口、token 数、镜像大小
//!    全部由发出事件的那段代码从真实调用里取；讲解员只负责把它们摆进句子里，
//!    不生成任何数字。
//! 2. **一条事件发出去就不改事实**，只能改 `status`（running → ok / failed）。
//!    界面据此把转圈换成勾，而不是把整棵树重画一遍。
//!
//! ## 落盘
//!
//! 同一份事件流同时写 `~/.hunter/logs/assist-events.jsonl`（一行一条 JSON）。
//! 测试要交的「事件流 JSON」就是它，headless 模式下也靠它给出可核对的记录。

use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::Serialize;

/// Tauri 事件名。前端 `listen('assist://event', …)`。
pub const EVENT: &str = "assist://event";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    /// 安装的一个步骤（检查 Docker / 下载组件 / 启动服务…）
    Step,
    /// 发现一个问题
    Issue,
    /// 正在分析（侦察 + 诊断）
    Analyze,
    /// 正在执行一个动作
    Action,
    /// 正在复验（重跑失败的那一步）
    Verify,
    /// 问题已解决
    Resolved,
    /// 需要用户点一下
    NeedUser,
    /// 彻底失败
    Failed,
    /// 顶上那一行常驻摘要
    Summary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Running,
    Ok,
    Warn,
    Failed,
    /// 在等用户点按钮
    Waiting,
    /// 跳过（例如离线模式下不测速）
    Skipped,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub id: u64,
    pub parent: Option<u64>,
    pub kind: Kind,
    pub status: Status,
    /// 左边那句人话
    pub title: String,
    /// 右边那句（真实数字都在这里）
    pub detail: String,
    /// 「详情」折叠里的技术细节（命令、原始输出、依据）。**已脱敏**
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tech: Vec<String>,
    /// 这一条花掉的 token（只有模型调用才有）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u64>,
    /// 「需要你」卡片上的两个按钮。`kind == NeedUser` 时才有
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<Choice>,
    /// 上海时间
    pub at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Choice {
    /// 前端回传给 `assist_auto_answer` 的值
    pub value: String,
    /// 按钮文案。**是结论不是问题**：「好，帮我装 OrbStack」而不是「是否安装？」
    pub label: String,
    /// 主按钮（琥珀金实心）还是次按钮
    pub primary: bool,
}

/// 顶上那一行常驻摘要的内容。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    /// AI 自动解决掉的问题数
    pub solved: usize,
    /// 还没解决的
    pub open: usize,
    pub tokens: u64,
    pub rounds: usize,
    pub max_rounds: usize,
    /// 从授权那一刻算起
    pub elapsed_ms: u64,
    /// 当前在干什么（一句话）
    pub phase: String,
}

// ── 发射器 ────────────────────────────────────────────────────────────────

/// 事件往哪儿发。Tauri 里发到窗口，headless 里打到终端 —— 两条路共用同一份事件。
pub trait Sink: Send + Sync {
    fn event(&self, e: &Event);
    fn summary(&self, s: &Summary);
}

/// 什么都不做的 sink（单元测试用）。
pub struct Null;
impl Sink for Null {
    fn event(&self, _: &Event) {}
    fn summary(&self, _: &Summary) {}
}

/// headless：一行一条打到 stdout。
#[derive(Default)]
pub struct Stdout {
    /// 上一行总览，一样就不重复打（不然每个 tick 刷一行，日志没法看）
    last_summary: Mutex<String>,
}

impl Stdout {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Sink for Stdout {
    fn event(&self, e: &Event) {
        let icon = match (e.kind, e.status) {
            (_, Status::Running) => "…",
            (_, Status::Waiting) => "?",
            (Kind::Issue, _) => "!",
            (Kind::Resolved, _) => "✔",
            (Kind::Failed, _) | (_, Status::Failed) => "×",
            (_, Status::Skipped) => "-",
            _ => "✓",
        };
        let indent = if e.parent.is_some() { "   " } else { "" };
        let mut line = format!("{indent}{icon} {}", e.title);
        if !e.detail.is_empty() {
            line.push_str(&format!("　{}", e.detail));
        }
        println!("{line}");
    }
    fn summary(&self, s: &Summary) {
        let line = format!(
            "[总览] {} · 已自动解决 {} 个问题 · 回合 {}/{} · token {}",
            s.phase, s.solved, s.rounds, s.max_rounds, s.tokens
        );
        if let Ok(mut g) = self.last_summary.lock() {
            if *g == line {
                return;
            }
            *g = line.clone();
        }
        println!("{line}");
    }
}

/// 事件总线：发号、落盘、转发给 sink，并留一份快照给「页面刚挂载上来」的前端。
pub struct Bus {
    next: AtomicU64,
    log: Mutex<Vec<Event>>,
    summary: Mutex<Summary>,
    sink: Box<dyn Sink>,
    /// 落盘路径。`None` 表示不落盘（单元测试）
    file: Option<std::path::PathBuf>,
}

impl Bus {
    pub fn new(sink: Box<dyn Sink>, to_file: bool) -> Self {
        let file = if to_file {
            let p = crate::paths::logs_dir().join("assist-events.jsonl");
            if let Some(d) = p.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            // 每次安装重开一份 —— 这份是「本次安装的事件流」，不是滚动日志
            let _ = std::fs::write(&p, "");
            let _ = crate::paths::chmod_600(&p);
            Some(p)
        } else {
            None
        };
        Self {
            next: AtomicU64::new(1),
            log: Mutex::new(Vec::new()),
            summary: Mutex::new(Summary::default()),
            sink,
            file,
        }
    }

    /// 发一条新事件，返回它的 id（后续用 [`Bus::finish`] 改它的状态）。
    pub fn emit(&self, e: EventDraft) -> u64 {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let ev = Event {
            id,
            parent: e.parent,
            kind: e.kind,
            status: e.status,
            title: clean(&e.title),
            detail: clean(&e.detail),
            tech: e.tech.iter().map(|s| clean(s)).collect(),
            tokens: e.tokens,
            elapsed_ms: e.elapsed_ms,
            choices: e.choices,
            at: crate::timefmt::now_shanghai(),
        };
        self.push(ev);
        id
    }

    /// 把一条已经发出去的事件收尾：改状态、补 detail / 耗时 / token。
    ///
    /// **只补不改事实**：`title` 不动，`detail` 由调用方给的真实值覆盖。
    pub fn finish(&self, id: u64, status: Status, detail: &str, elapsed_ms: Option<u64>) {
        let mut ev = {
            let Ok(g) = self.log.lock() else { return };
            match g.iter().find(|e| e.id == id) {
                Some(e) => e.clone(),
                None => return,
            }
        };
        ev.status = status;
        if !detail.is_empty() {
            ev.detail = clean(detail);
        }
        if elapsed_ms.is_some() {
            ev.elapsed_ms = elapsed_ms;
        }
        ev.at = crate::timefmt::now_shanghai();
        if let Ok(mut g) = self.log.lock() {
            if let Some(slot) = g.iter_mut().find(|e| e.id == id) {
                *slot = ev.clone();
            }
        }
        self.write_line(&ev);
        self.sink.event(&ev);
    }

    fn push(&self, ev: Event) {
        if let Ok(mut g) = self.log.lock() {
            g.push(ev.clone());
        }
        self.write_line(&ev);
        self.sink.event(&ev);
    }

    fn write_line(&self, ev: &Event) {
        let Some(p) = &self.file else { return };
        let Ok(line) = serde_json::to_string(ev) else {
            return;
        };
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)
        {
            let _ = writeln!(f, "{line}");
        }
    }

    pub fn set_summary(&self, s: Summary) {
        if let Ok(mut g) = self.summary.lock() {
            *g = s.clone();
        }
        self.sink.summary(&s);
    }

    pub fn summary_now(&self) -> Summary {
        self.summary.lock().map(|g| g.clone()).unwrap_or_default()
    }

    /// 整棵树的快照。前端页面挂载时先拉一次，之后靠事件增量更新。
    pub fn snapshot(&self) -> Vec<Event> {
        self.log.lock().map(|g| g.clone()).unwrap_or_default()
    }
}

/// 发事件时填的那一份（`id` 与 `at` 由总线补）。
pub struct EventDraft {
    pub parent: Option<u64>,
    pub kind: Kind,
    pub status: Status,
    pub title: String,
    pub detail: String,
    pub tech: Vec<String>,
    pub tokens: Option<u64>,
    pub elapsed_ms: Option<u64>,
    pub choices: Vec<Choice>,
}

impl EventDraft {
    pub fn new(kind: Kind, title: impl Into<String>) -> Self {
        Self {
            parent: None,
            kind,
            status: Status::Running,
            title: title.into(),
            detail: String::new(),
            tech: Vec::new(),
            tokens: None,
            elapsed_ms: None,
            choices: Vec::new(),
        }
    }
    pub fn under(mut self, parent: u64) -> Self {
        self.parent = Some(parent);
        self
    }
    pub fn status(mut self, s: Status) -> Self {
        self.status = s;
        self
    }
    pub fn detail(mut self, d: impl Into<String>) -> Self {
        self.detail = d.into();
        self
    }
    pub fn tech(mut self, t: impl Into<String>) -> Self {
        self.tech.push(t.into());
        self
    }
    pub fn tokens(mut self, n: u64) -> Self {
        self.tokens = Some(n);
        self
    }
    pub fn elapsed(mut self, ms: u64) -> Self {
        self.elapsed_ms = Some(ms);
        self
    }
    pub fn choices(mut self, c: Vec<Choice>) -> Self {
        self.choices = c;
        self
    }
}

/// 事件里的每一个字符串都要过这里：脱敏 + 把家目录换成 `<用户目录>`（红线 2）。
fn clean(s: &str) -> String {
    crate::redact::mask_home(&crate::redact::redact(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 事件编号从_1_开始且递增() {
        let b = Bus::new(Box::new(Null), false);
        let a = b.emit(EventDraft::new(Kind::Step, "检查 Docker"));
        let c = b.emit(EventDraft::new(Kind::Step, "下载组件").under(a));
        assert_eq!(a, 1);
        assert_eq!(c, 2);
        let snap = b.snapshot();
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[1].parent, Some(1));
    }

    #[test]
    fn finish_只改状态与_detail_不改标题() {
        let b = Bus::new(Box::new(Null), false);
        let id = b.emit(EventDraft::new(Kind::Step, "启动服务").detail("正在起容器"));
        b.finish(id, Status::Ok, "6 / 6 健康", Some(42_000));
        let e = b.snapshot().into_iter().find(|e| e.id == id).unwrap();
        assert_eq!(e.title, "启动服务");
        assert_eq!(e.detail, "6 / 6 健康");
        assert_eq!(e.status, Status::Ok);
        assert_eq!(e.elapsed_ms, Some(42_000));
    }

    #[test]
    fn 事件里的_key_一律脱敏() {
        let b = Bus::new(Box::new(Null), false);
        b.emit(
            EventDraft::new(Kind::Action, "写 .env")
                .detail("写入 hunt_tools_abcdefghijklmnopqrstuvwxyz012345"),
        );
        let e = &b.snapshot()[0];
        assert!(!e.detail.contains("abcdefghij"), "{}", e.detail);
        assert!(e.detail.contains("hunt_tools_****"), "{}", e.detail);
    }

    #[test]
    fn 摘要拿得回来() {
        let b = Bus::new(Box::new(Null), false);
        b.set_summary(Summary {
            solved: 1,
            open: 0,
            tokens: 7000,
            rounds: 2,
            max_rounds: 10,
            elapsed_ms: 120_000,
            phase: "启动服务".into(),
        });
        let s = b.summary_now();
        assert_eq!(s.solved, 1);
        assert_eq!(s.tokens, 7000);
    }

    #[test]
    fn 需要你的卡片带两个结论式按钮() {
        let b = Bus::new(Box::new(Null), false);
        let id = b.emit(
            EventDraft::new(Kind::NeedUser, "要装 OrbStack 吗")
                .status(Status::Waiting)
                .choices(vec![
                    Choice {
                        value: "yes".into(),
                        label: "好，帮我安装 OrbStack".into(),
                        primary: true,
                    },
                    Choice {
                        value: "no".into(),
                        label: "不用，我自己装".into(),
                        primary: false,
                    },
                ]),
        );
        let e = b.snapshot().into_iter().find(|e| e.id == id).unwrap();
        assert_eq!(e.choices.len(), 2);
        assert_eq!(e.status, Status::Waiting);
        // 按钮文案是结论，不是问句
        assert!(!e.choices[0].label.contains('?'));
        assert!(!e.choices[0].label.contains('？'));
    }
}
