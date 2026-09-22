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
    /// 复核员（第二个模型）对含 `Sensitive` 动作的计划的判断（I7）
    Review,
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
            (Kind::Review, _) => "审",
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
        // 「需要你」卡片上的按钮也要打出来 —— 命令行下点不了，
        // 但用户至少得知道界面上此刻会问他什么（不然只看到一行标题，像卡住了）
        if !e.choices.is_empty() {
            let labels: Vec<&str> = e.choices.iter().map(|c| c.label.as_str()).collect();
            line.push_str(&format!("　[{}]", labels.join(" / ")));
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

/// 当前这一次安装的事件流文件名。**排查工具与测试都认这个名字**，不要改。
pub const CURRENT: &str = "assist-events.jsonl";
/// 连当前这一份在内，一共留几份。
pub const KEEP: usize = 5;

/// 归档上一次的事件流，并把超出 [`KEEP`] 的旧档删掉。
///
/// 归档名用的是**那份文件自己的修改时间**（上海时间，文件名安全的写法），
/// 不是「现在」—— 归档发生在下一次安装开始的时刻，拿现在当时间戳会让
/// 文件名和内容对不上。取不到修改时间就退回用当前时间并在名字里标 `unknown`。
fn archive_previous(current: &std::path::Path) {
    let Ok(md) = std::fs::metadata(current) else {
        return; // 第一次装，没有上一份
    };
    if md.len() == 0 {
        return; // 上一次开了头就退出了，空文件不值得留
    }
    let stamp = md
        .modified()
        .ok()
        .map(crate::timefmt::file_stamp)
        .unwrap_or_else(|| format!("unknown-{}", crate::timefmt::file_stamp_now()));
    let dir = current
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let archived = dir.join(format!("assist-events-{stamp}.jsonl"));
    if let Err(e) = std::fs::rename(current, &archived) {
        crate::lwarn!("归档上一次的事件流失败（这次仍然会重开一份）：{e}");
        return;
    }
    let _ = crate::paths::chmod_600(&archived);
    prune(dir);
}

/// 只留最近 [`KEEP`] - 1 份历史档（加上马上要新建的当前那一份正好 [`KEEP`] 份）。
fn prune(dir: &std::path::Path) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut olds: Vec<std::path::PathBuf> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("assist-events-") && n.ends_with(".jsonl"))
                .unwrap_or(false)
        })
        .collect();
    // 名字里的时间戳是定长且可比的，按名字排等于按时间排
    olds.sort();
    let keep_history = KEEP.saturating_sub(1);
    if olds.len() <= keep_history {
        return;
    }
    for p in &olds[..olds.len() - keep_history] {
        if let Err(e) = std::fs::remove_file(p) {
            crate::lwarn!("删旧的事件流归档 {} 失败：{e}", p.display());
        }
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
            let p = crate::paths::logs_dir().join(CURRENT);
            if let Some(d) = p.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            // 每次安装重开一份 —— 这份是「本次安装的事件流」，不是滚动日志
            // （树是按 id 组的，两次混在一起就乱了）。但**上一次的不能就这么没了**：
            // 排查「上一版装到哪一步失败的」时，被覆盖掉的正是最要紧的那一份。
            // 所以开新的之前先把旧的归档，并且只留最近几份（I7 · 待办池 P2-21）
            archive_previous(&p);
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

    /// 事件流**按次保留最近 5 份**，不再一开新的就把上一次覆盖掉（I7 · 待办池 P2-21）。
    #[test]
    fn 事件流归档只留最近五份() {
        let _g = crate::paths::test_home("events-archive");
        let dir = crate::paths::logs_dir();
        std::fs::create_dir_all(&dir).unwrap();
        // 先把上一条测试留下的归档清掉，免得两条测试互相看见
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if n.starts_with("assist-events-") && n.ends_with(".jsonl") {
                let _ = std::fs::remove_file(e.path());
            }
        }
        let cur = dir.join(CURRENT);
        // 造 8 份「上一次的事件流」，每次都走一遍开新档的流程
        for i in 0..8u32 {
            std::fs::write(&cur, format!("{{\"id\":{i}}}\n")).unwrap();
            // 归档名取的是文件修改时间（秒级），连着写会撞名 —— 手工给不同的名字，
            // 这里直接测 prune：先按序造出 8 个归档，再看 prune 留下几个
            let archived = dir.join(format!("assist-events-2026092{i}-000000.jsonl"));
            std::fs::rename(&cur, &archived).unwrap();
        }
        prune(&dir);
        let left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("assist-events-") && n.ends_with(".jsonl"))
            .collect();
        assert_eq!(
            left.len(),
            KEEP - 1,
            "连当前那一份算上一共留 {KEEP} 份，历史档就该是 {} 个：{left:?}",
            KEEP - 1
        );
        // 留下的必须是**最近的**那几个（名字里的时间戳最大）
        let mut sorted = left.clone();
        sorted.sort();
        assert!(sorted[0].contains("20260924"), "留错了：{sorted:?}");
        assert!(
            sorted[sorted.len() - 1].contains("20260927"),
            "留错了：{sorted:?}"
        );
    }

    /// 空的上一份不值得归档（上一次开了头就退出了）。
    #[test]
    fn 空文件不归档() {
        let _g = crate::paths::test_home("events-empty");
        let dir = crate::paths::logs_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let cur = dir.join("assist-events-empty-probe.jsonl");
        std::fs::write(&cur, "").unwrap();
        archive_previous(&cur);
        assert!(cur.exists(), "空文件应该原地不动");
        let _ = std::fs::remove_file(&cur);
    }

    /// 复核卡片是**单独一种** kind，不是借用别的。
    #[test]
    fn 复核是单独一种事件() {
        let j = serde_json::to_string(&Kind::Review).unwrap();
        assert_eq!(j, "\"review\"");
        assert_ne!(
            serde_json::to_string(&Kind::Analyze).unwrap(),
            j,
            "复核不能和「分析」混成一种"
        );
    }
}
