//! AI 自动驾驶安装 · 总指挥与各个角色（I5 · 设计文档 §四）。
//!
//! ```text
//!                  ┌──────────────────────────────────────────────┐
//!                  │  总指挥 Orchestrator（确定性状态机，非模型）    │
//!                  │  安装步骤推进 · 回合上限 · token 预算 · 超时    │
//!                  └──┬────────────┬───────────┬──────────┬────────┘
//!                     │失败         │           │          │
//!           ┌─────────▼───┐  ┌──────▼─────┐ ┌───▼────┐ ┌──▼──────────┐
//!           │ 侦察员 Scout │→ │ 诊断员      │→│ 守卫    │→│ 执行员       │
//!           │ 只读采证据   │  │ 规则 → 模型 │ │ 代码    │ │ 动作表       │
//!           │ 零 token    │  └────────────┘ └────────┘ └──┬──────────┘
//!           └─────────────┘                               │
//!                                                  ┌──────▼──────┐
//!                                                  │ 验证员       │
//!                                                  │ 重跑失败那步  │
//!                                                  └─────────────┘
//! ```
//!
//! ## 只有一个角色会调模型
//!
//! 侦察、守卫、执行、验证、讲解全是确定性代码。理由不是「省 token」，
//! 是**这些事交给模型只会更差**：用户 Mac 上那次失败的 5 条根因里有 3 条
//! （端口误判、错误归类、证据缺口）本来就该由代码判死，交给模型去猜的结果
//! 就是它把「用户自己另一套 Hunter」猜成了「残留容器」。
//!
//! 诊断员也是**先规则后模型**：规则认得出来的（端口冲突、装了没起、拉取失败）
//! 一个 token 都不花，而且同一个现象每次给的都是同一句话。
//!
//! ## 验证不是问模型「好了吗」
//!
//! [`Orchestrator::run_step`] 执行完修复动作之后**重跑失败的那一步**。
//! 跑过了才算解决，跑不过就带着新证据进下一回合。模型说「应该可以了」不算数。
//!
//! ## 预算（设计文档 §八）
//!
//! | 项 | 取值 |
//! |---|---|
//! | 单问题回合上限 | 4 |
//! | 整次安装回合上限 | 10 |
//! | 整次安装 token 上限 | 60,000 |
//! | 单次模型超时 | 45 秒（[`super::ai::TIMEOUT`]） |

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

use super::actions::{self, Call};
use super::events::{Bus, Choice, EventDraft, Kind, Status, Summary};
use super::guard::{self, Mode, Proposer};
use super::probe;
use crate::compose;
use crate::config::LauncherConfig;
use crate::err::{AppError, AppResult, Code};
use crate::flow::{self, AppState, InstallOptions};

/// 「需要你」卡片的答案从哪儿来：`(问题, 按钮) -> 用户选的 value`。
///
/// 界面版不用它（那边走事件 + `assist_auto_answer`）；命令行版在 stdin 是终端时
/// 给一个「打印问题 + 读一行」的实现（I7 · 待办池 P1-25）。
pub type AnswerReader = Box<dyn Fn(&str, &[Choice]) -> Option<String> + Send + Sync>;

/// 单个问题最多来回几次。I4 实测 3 轮常常刚摸到答案，给 4。
pub const MAX_ROUNDS_PER_ISSUE: usize = 4;
/// 整次安装最多几个回合。防止在多个问题之间来回兜圈。
pub const MAX_ROUNDS_TOTAL: usize = 10;
/// 整次安装的 token 上限。
///
/// 原来按「日额度 30 万的 20%」定为 6 万；2026-09-22 内置额度调到每天 1000 万后
/// 放宽到 20 万（仍只占日额度 2%），让 `MAX_ROUNDS_TOTAL` 的回合数先于 token 预算起作用。
pub const MAX_TOKENS: u64 = 200_000;

/// 安装的几个步骤。**失败重跑的就是这一个枚举里的一项**。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Step {
    /// 检查 Docker 装没装、起没起
    Docker,
    /// 选源 + 写配置 + 算端口
    Prepare,
    /// 拉镜像
    Pull,
    /// 起容器 + 等健康
    Start,
}

impl Step {
    pub fn title(self) -> &'static str {
        match self {
            Step::Docker => "检查 Docker",
            Step::Prepare => "挑下载源、算端口、写配置",
            Step::Pull => "下载组件",
            Step::Start => "启动服务",
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Step::Docker => "docker",
            Step::Prepare => "prepare",
            Step::Pull => "pull",
            Step::Start => "start",
        }
    }
}

/// 一次自动安装的结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub ok: bool,
    /// 失败时的错误码
    pub code: Option<String>,
    /// 失败时的一句话（已脱敏）
    pub message: Option<String>,
    pub solved: usize,
    pub rounds: usize,
    pub tokens: u64,
    pub elapsed_ms: u64,
    /// 装好之后打开哪个地址（真实端口）
    pub url: Option<String>,
    /// 用户中途选了「直接用你已经有的那一套」时，这里是那个 compose 项目名（I7）。
    /// 有值时 `ok` 也是 true —— 事情办成了，只是办法不是「再装一套」
    #[serde(skip_serializing_if = "Option::is_none")]
    pub takeover: Option<String>,
    /// **这一次没有装任何东西**：开工前的复查发现启动器自己上一次装的那一套
    /// 已经在正常跑了（或者只需要把某个服务重新起一下）。
    ///
    /// 0.1.9 在用户 Mac 上的那一下「重试」之所以要重新拉 849 MB，
    /// 就是因为当时根本没有这一问（I11 · U2）。
    #[serde(default)]
    pub reused: bool,
}

/// 开工前复查的三种去向（I11 · U2）。
enum Preflight {
    /// 已经在正常跑了 —— 什么都不做
    Reuse(crate::selfcheck::Review),
    /// 六个容器都在，只有一部分不正常 —— 只修那一部分
    FixOnly(crate::selfcheck::Review),
    /// 这台机器上没有一套能用的 —— 走完整安装
    FullInstall,
}

// ── 需要用户点一下 ────────────────────────────────────────────────────────

/// 「需要你」的三种情况（设计文档 §三）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AskWhy {
    /// 要装新软件（I7 起真的会装，见 `install_runtime`）
    InstallSoftware,
    /// 要动这台机器上已有的 Hunter
    TouchExisting,
    /// 这次的额度上限要花超了
    OverBudget,
}

/// 等用户回答的一次提问。
struct Ask {
    answer: Mutex<Option<String>>,
    cv: Condvar,
}

/// 给别的线程用的句柄。
#[derive(Clone)]
pub struct Handle {
    pub bus: Arc<Bus>,
    ask: Arc<Mutex<Option<Arc<Ask>>>>,
    cancel: Arc<AtomicBool>,
    takeover: Arc<Mutex<Option<String>>>,
    /// 「你电脑上已经有一套」那张**不阻塞**卡片的事件 id。
    /// 用户点了任意一个按钮就把它收尾，免得它一直挂着「等待中」
    offer: Arc<Mutex<Option<u64>>>,
}

/// 前端点「直接用它，不再装一套」时回传的值前缀（I7）。
pub const TAKEOVER_PREFIX: &str = "takeover:";

impl Handle {
    /// 用户点了「需要你」卡片上的按钮。没有正在等的提问时返回 false。
    ///
    /// **「直接用它」这一个是例外**：那张卡片不阻塞（见 [`Orchestrator::report_other_installs`]），
    /// 安装本来就在往下跑，所以它走的是另一条通路 —— 记下项目名，
    /// 总指挥在下一个步骤开始前读到它就改道。
    pub fn answer(&self, value: &str) -> bool {
        if let Some(project) = value.strip_prefix(TAKEOVER_PREFIX) {
            let project = project.trim().to_string();
            if project.is_empty() {
                return false;
            }
            if let Ok(mut g) = self.takeover.lock() {
                *g = Some(project.clone());
            }
            self.close_offer(&format!("记下了：改用「{project}」"));
            return true;
        }
        if value == "coexist" {
            // 「和它并存」本来就是默认 —— 点它只是把话说死，安装照原样往下跑
            self.close_offer("你选了「和它并存」，安装照原样继续（本来也是这么跑的）");
            return true;
        }
        let g = self.ask.lock().ok().and_then(|g| g.clone());
        let Some(ask) = g else { return false };
        if let Ok(mut a) = ask.answer.lock() {
            *a = Some(value.to_string());
        }
        ask.cv.notify_all();
        true
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// 把那张不阻塞的选择卡片收尾（只收一次）。
    fn close_offer(&self, detail: &str) {
        let id = self.offer.lock().ok().and_then(|mut g| g.take());
        if let Some(id) = id {
            self.bus.finish(id, Status::Ok, detail, None);
        }
    }
}

// ── 总指挥 ────────────────────────────────────────────────────────────────

pub struct Orchestrator {
    pub bus: Arc<Bus>,
    mode: Mode,
    key: Option<String>,
    /// 用过的回合数（一个回合 = 一次诊断→执行→验证）
    rounds: usize,
    tokens: u64,
    solved: usize,
    open: usize,
    t0: Instant,
    phase: String,
    cancel: Arc<AtomicBool>,
    ask: Arc<Mutex<Option<Arc<Ask>>>>,
    /// 用户点了「直接用它，不再装一套」时，这里会有那个项目名（I7）。
    /// 总指挥在每个步骤开始前看一眼 —— 有值就改道去接管，不再往下装
    takeover: Arc<Mutex<Option<String>>>,
    /// 那张不阻塞的选择卡片的事件 id（见 [`Handle::close_offer`]）
    offer: Arc<Mutex<Option<u64>>>,
    /// `Step::Prepare` 算出来的那一份。拉取那一步直接用它，
    /// **不要再 prepare 一遍** —— 那会白跑一次镜像源测速与 compose 下载
    prep: Option<flow::PrepareResult>,
    /// 侦察员采到的、这一次安装里已经报告过的「本机其他 Hunter」，只说一次
    reported_others: Mutex<bool>,
    /// 已经报告过的「靠系统代理才通的站点」（I8）。同一个站点只说一次、只计一次
    reported_proxy: std::collections::HashSet<String>,
    /// 这一次安装里**已经试过而且失败了**的动作 id。
    ///
    /// 场景 2 首轮实测：`systemctl start docker` 没有权限，退出码 1，
    /// 规则层每一回合都原样再提一次，三个回合白花 4 分 39 秒。
    /// 同一个动作失败过就不再提第二次 —— 该换路子，或者该让用户出手。
    failed_actions: std::collections::HashSet<String>,
    /// **已经为「这条原话」换过一次源**的失败指纹（I6）。
    ///
    /// 0.1.5 在用户 Mac 上的现场：凭据助手找不到 → 归成「拉不动」→ 规则层换源 →
    /// 还是同一句原话 → 再换回去……三个回合在 ghcr 与腾讯云之间来回兜圈，
    /// 268 秒、0 token、一个问题都没解决。
    ///
    /// 规则：**同一条原话只换一次源**；换完原话一个字都没变，就判定「与源无关」，
    /// 规则层让路，把现场连同原话交给诊断员（模型）。
    switched_registry: std::collections::HashSet<String>,
    /// 命令行下「需要你」卡片的回答从哪儿来（I7 · 待办池 P1-25 的结论）。
    ///
    /// `None` = 没人能答（CI、管道、后台任务），按 `interactive` 那条走。
    /// `Some(f)` = 调用方给了一条读答案的路 —— headless 在 **stdin 是终端**时
    /// 会给一个「打印问题 + 读一行」的闭包。
    ///
    /// 为什么不直接在这里读 stdin：总指挥跑在界面进程里时，stdin 可能根本不存在；
    /// 「从哪儿读」是调用方的事，这里只负责「问」。
    answer_reader: Option<AnswerReader>,
    /// 这一次**为了把端口收回本机而重建过**的服务（I11 · U5）。
    ///
    /// 收尾那句话要跟着它改：重建过容器还说「没有动正在跑的容器」就是在骗人。
    rebound: Vec<String>,
    /// 有没有人能回答「需要你」卡片。
    ///
    /// 界面里是 true（用户看得见那两个按钮）；`--auto` 命令行里是 false ——
    /// 那边没有人能点，一直等下去就是把进程挂死。为 false 时卡片照发（要留痕），
    /// 但当场按「不」处理并说清原因。
    interactive: bool,
}

impl Orchestrator {
    pub fn new(bus: Arc<Bus>, mode: Mode, key: Option<String>, cancel: Arc<AtomicBool>) -> Self {
        Self {
            bus,
            mode,
            key,
            rounds: 0,
            tokens: 0,
            solved: 0,
            open: 0,
            t0: Instant::now(),
            phase: "准备开始".into(),
            cancel,
            ask: Arc::new(Mutex::new(None)),
            takeover: Arc::new(Mutex::new(None)),
            offer: Arc::new(Mutex::new(None)),
            prep: None,
            reported_others: Mutex::new(false),
            reported_proxy: std::collections::HashSet::new(),
            failed_actions: std::collections::HashSet::new(),
            switched_registry: std::collections::HashSet::new(),
            answer_reader: None,
            rebound: Vec::new(),
            interactive: true,
        }
    }

    /// 命令行下没有人能点按钮，调用方要如实说一声。
    pub fn set_interactive(&mut self, v: bool) {
        self.interactive = v;
    }

    /// 给一条「读答案」的路（I7）。设了它就等于 `interactive = true`。
    pub fn set_answer_reader(&mut self, f: AnswerReader) {
        self.answer_reader = Some(f);
        self.interactive = true;
    }

    /// 跨线程的句柄：界面线程要能看事件树、回答提问、喊停，
    /// 而 [`Orchestrator::run`] 本身拿的是 `&mut self`（它是一台状态机，
    /// 不该让别的线程改它的回合数与预算）。两件事分开。
    pub fn handle(&self) -> Handle {
        Handle {
            bus: self.bus.clone(),
            ask: self.ask.clone(),
            cancel: self.cancel.clone(),
            takeover: self.takeover.clone(),
            offer: self.offer.clone(),
        }
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    fn tick(&mut self, phase: &str) {
        self.phase = phase.to_string();
        let s = Summary {
            solved: self.solved,
            open: self.open,
            tokens: self.tokens,
            rounds: self.rounds,
            max_rounds: MAX_ROUNDS_TOTAL,
            elapsed_ms: self.t0.elapsed().as_millis() as u64,
            phase: phase.to_string(),
        };
        self.bus.set_summary(s);
    }

    // ── 主流程 ────────────────────────────────────────────────────────────

    /// 跑完整个安装。**授权之后到这里为止不需要任何点击**，
    /// 除非撞上「需要你」的三种情况之一。
    pub fn run(&mut self, state: &AppState, opts: &InstallOptions) -> Outcome {
        let mut opts = opts.clone();
        self.t0 = Instant::now();
        // 这一次安装的「代理救场」计数从零开始（上一次的不该算进这一次）
        crate::netproxy::reset_rescued();
        crate::netproxy::invalidate();
        self.tick("开始安装");
        crate::linfo!(
            "AI 自动安装开始（授权档位 {}，预算：单问题 {} 回合 / 整次 {} 回合 / {} token）",
            self.mode.as_str(),
            MAX_ROUNDS_PER_ISSUE,
            MAX_ROUNDS_TOTAL,
            MAX_TOKENS
        );
        guard::audit(
            "autopilot_start",
            &std::collections::BTreeMap::new(),
            Proposer::User,
            None,
            &format!("授权档位 {}", self.mode.as_str()),
        );

        // **开工第一件事：先看看现在到底是什么样**（I11 · U2）。
        //
        // 0.1.9 在用户 Mac 上，用户点「重试」的那一刻六个服务已经全绿了 41 分钟，
        // 而启动器二话不说重新取 compose、重写 .env、开始重拉 849 MB。
        // 这一段就是那句该先问的话 —— 它只读，不改任何东西。
        let steps: &[Step] = match self.preflight(state) {
            Preflight::Reuse(r) => return self.reuse_outcome(state, &r, None),
            Preflight::FixOnly(r) => match self.fix_only(state, r) {
                Ok(Some(o)) => return o,
                // 只修不正常的那部分没修好 —— 接着走「启动服务」那一步的修复回合
                // （AI、规则层都在那条路上）。**照样不重新取 compose、不重拉镜像。**
                Ok(None) => &[Step::Start],
                Err(o) => return *o,
            },
            Preflight::FullInstall => {
                // **写配置之前先看一眼有没有上一次的数据**（I12 · R5）。
                // 这一步只读；查到「不能装」的那一档就在这里停住，
                // 绝不让安装流程走到会把用户数据弄坏的地方
                if let Some(o) = self.data_precheck() {
                    return o;
                }
                &[Step::Docker, Step::Prepare, Step::Pull, Step::Start]
            }
        };

        for step in steps.iter().copied() {
            if self.cancelled() {
                return self.fail(Code::Unknown, "安装被取消了。");
            }
            // 用户在「你电脑上已经有一套」那张卡片上点了「直接用它」——
            // 那就别再装了（I7）。这一步是**改道**不是失败
            if let Some(o) = self.maybe_takeover() {
                return o;
            }
            if let Err(e) = self.run_step_with_repair(state, &mut opts, step) {
                // 最终页上要给一条能照着做的出路，而不是只丢一个错误码
                // 最终页上要说清「卡在哪、启动器为什么做不下去」，
                // **不给一条让用户自己去敲的命令**（I8 第〇节）
                let msg = match cannot_do(&e) {
                    Some((what, why)) => format!("{}\n{what}：{why}", e.msg),
                    None => e.msg.clone(),
                };
                return self.fail(e.code, &msg);
            }
        }

        // 装完了才点「直接用它」—— 那就**如实说没有改道**，不要让那张卡片
        // 留着一句「这就改道」却什么都没发生
        if let Some(late) = self.takeover.lock().ok().and_then(|mut g| g.take()) {
            self.bus.emit(
                EventDraft::new(Kind::Action, format!("「直接用 {late}」这一下点晚了"))
                    .status(Status::Skipped)
                    .detail("这次安装已经跑完了，所以没有改道 —— 两套现在都在，互不影响")
                    .tech("想改成管理原有的那一套：设置页 →「容器运行时」下面那一节，或者命令行 `--takeover use <项目名> -y`"),
            );
        }

        // **从磁盘重读**：修复动作（remap_ports / switch_registry / raise_timeouts）
        // 改的是 launcher.toml，AppState 里那一份是它们动手之前的快照。
        // 首轮实测撞到过：端口被改到 3102，最后一行却还说「打开 3101」（红线 1）
        let cfg = LauncherConfig::load();
        state.set_config(cfg.clone());
        let url = format!("http://localhost:{}", cfg.hunter.ports.web);
        let id = self.bus.emit(
            EventDraft::new(Kind::Step, "完成")
                .status(Status::Ok)
                .detail(format!("打开 {url} 就能用了")),
        );
        let _ = id;
        self.tick("完成");
        crate::linfo!(
            "AI 自动安装完成：自动解决 {} 个问题 · {} 回合 · {} token · {} 秒",
            self.solved,
            self.rounds,
            self.tokens,
            self.t0.elapsed().as_secs()
        );
        Outcome {
            ok: true,
            code: None,
            message: None,
            solved: self.solved,
            rounds: self.rounds,
            tokens: self.tokens,
            elapsed_ms: self.t0.elapsed().as_millis() as u64,
            url: Some(url),
            takeover: None,
            reused: false,
        }
    }

    // ── 开工前的现状复查（I11 · U2）──────────────────────────────────────

    /// 复查的三种去向。
    fn preflight_plan(&mut self, _state: &AppState) -> Preflight {
        let r = crate::selfcheck::review(true);
        crate::linfo!("开工前复查：{}（{} 毫秒）", r.headline, r.elapsed_ms);
        match r.posture {
            crate::selfcheck::Posture::Healthy => Preflight::Reuse(r),
            crate::selfcheck::Posture::Partial => Preflight::FixOnly(r),
            // 六个容器都建齐了、只是停着（I16 · P0-4）：**起一下就行**。
            // 走完整安装会重新取 compose、重写配置、重拉镜像 —— 一件都不需要
            crate::selfcheck::Posture::Stopped => Preflight::FixOnly(r),
            _ => Preflight::FullInstall,
        }
    }

    /// 复查 + 把结论报给界面。**这一整段只读**。
    fn preflight(&mut self, state: &AppState) -> Preflight {
        let t = Instant::now();
        let ev = self
            .bus
            .emit(EventDraft::new(Kind::Step, "先看看现在是什么情况"));
        self.tick("复查现状");
        let plan = self.preflight_plan(state);
        let (status, detail) = match &plan {
            Preflight::Reuse(r) => (Status::Ok, r.headline.clone()),
            Preflight::FixOnly(r) => (Status::Warn, r.headline.clone()),
            Preflight::FullInstall => (
                Status::Ok,
                "这台机器上还没有一套能用的 Hunter，按完整流程装".to_string(),
            ),
        };
        let mut d = EventDraft::new(Kind::Analyze, "查到的现状")
            .under(ev)
            .status(Status::Ok)
            .detail(detail.clone());
        if let Preflight::Reuse(r) | Preflight::FixOnly(r) = &plan {
            for l in &r.lines {
                d = d.tech(l.clone());
            }
            // 容器被谁重建成 0.0.0.0 了（I11 · U5）——现在就说，并在下面收回来
            for x in &r.drift {
                d = d.tech(x.human());
            }
        }
        self.bus.emit(d);
        self.bus
            .finish(ev, status, &detail, Some(t.elapsed().as_millis() as u64));
        plan
    }

    /// **重装之前先看看有没有旧数据**（I12 · R5）。
    ///
    /// 只在「完整安装」那条路上做，而且**只读**（浅查：`docker volume ls` +
    /// 一个 `:ro` 挂载的一次性容器读 `PG_VERSION` + 读 `.env`，一个字节都不写）。
    ///
    /// 四档结果对应三种做法：
    ///
    /// | 结果 | 做法 | 为什么 |
    /// |---|---|---|
    /// | 全新 / 只有备份 | 照常装 | 没有要保护的东西 |
    /// | 都在（`Reuse`） | 照常装，并在过程流里说清「会直接沿用」 | 数据卷不删、`JWT_SECRET` 沿用，compose 起来就接上了 |
    /// | 缺密钥卷 | **照常装**，但把后果说清楚 | 见下 |
    /// | 降级 | **停在这里**，返回失败 | 硬装会把数据目录弄坏，这是不可逆的 |
    ///
    /// ## 「缺密钥卷」为什么不拦（与方案 R5 的偏离，写进报告）
    ///
    /// 方案 R5 写的是「停下来提示风险，给两个选项：从备份恢复密钥卷 /
    /// 放弃这部分加密配置」。I12 只做到一半：**「从备份恢复密钥卷」这个能力
    /// 要等 I13 的 R6**（现在的 `backup.rs` 只备份数据库与配置，不备份卷）。
    /// 摆一个点不动的选项比不摆更糟。而另一个选项「放弃这部分加密配置」
    /// 恰恰就是「照常装」——它不删任何东西，只是那几项加密配置解不开。
    /// 所以这一档如实说清后果、照常往下走，等 I13 把恢复那条路补上再改成两选一。
    fn data_precheck(&mut self) -> Option<Outcome> {
        let t = Instant::now();
        let ev = self
            .bus
            .emit(EventDraft::new(Kind::Step, "看看有没有上一次的数据"));
        self.tick("检测已有数据");
        let c = crate::datacheck::probe();
        let mut d = EventDraft::new(Kind::Analyze, "查到的数据")
            .under(ev)
            .status(Status::Ok)
            .detail(c.headline.clone());
        for l in &c.lines {
            d = d.tech(l.clone());
        }
        self.bus.emit(d);

        use crate::datacheck::Decision;
        match c.decision {
            Decision::Downgrade => {
                self.bus.finish(
                    ev,
                    Status::Failed,
                    &c.headline,
                    Some(t.elapsed().as_millis() as u64),
                );
                crate::lwarn!("R5 拦下了这次安装：{}", c.headline);
                Some(self.fail(
                    Code::DataDowngrade,
                    &format!(
                        "{}\n这次安装到此为止 —— 硬装下去会把你现在的数据弄坏，那是没法撤销的。\n                         可行的两条路：把 Hunter 升到当时那一版再装；或者从备份恢复。",
                        c.headline
                    ),
                ))
            }
            Decision::MissingSecrets => {
                self.bus.finish(
                    ev,
                    Status::Warn,
                    &c.headline,
                    Some(t.elapsed().as_millis() as u64),
                );
                self.bus.emit(
                    EventDraft::new(Kind::Analyze, "这次安装会怎么处理")
                        .under(ev)
                        .status(Status::Warn)
                        .detail(
                            "数据库、会话、自建技能全都保留，登录也不会失效。                             只有用那把密钥加密过的几项配置解不开，需要你在 Hunter 里重新填一次。",
                        )
                        .tech("密钥卷会重新建一个空的，启动器不会去动数据库里的任何一行"),
                );
                None
            }
            Decision::Reuse => {
                self.bus.finish(
                    ev,
                    Status::Ok,
                    &c.headline,
                    Some(t.elapsed().as_millis() as u64),
                );
                None
            }
            Decision::Fresh | Decision::BackupOnly => {
                self.bus.finish(
                    ev,
                    Status::Ok,
                    &c.headline,
                    Some(t.elapsed().as_millis() as u64),
                );
                None
            }
        }
    }

    /// 「这一套已经在跑了」的收尾：**一个安装动作都不做**。
    ///
    /// `fixed` 有值时表示刚才只把某几个服务弄起来过 —— 那句结论要跟着改，
    /// 不能对着一台刚被动过的机器说「本来就好好的」（红线 1 的同一条道理）。
    fn reuse_outcome(
        &mut self,
        state: &AppState,
        r: &crate::selfcheck::Review,
        fixed: Option<&crate::selfcheck::FixLog>,
    ) -> Outcome {
        self.recover_bindings(r);
        // 它确实装好了 —— 让下一次打开启动器直接进运行面板，不再重走向导
        let mut cfg = LauncherConfig::load();
        // I12 · R1：`done` 之外还要记全「谁装的、装在哪、上一次好好的是什么时候」。
        // `adopted` 为真 = 这条记录是「检测到它已经在跑」补的，不是安装流程写的 ——
        // 这条路正是 2026-09-22 那次现场缺的那一笔
        let adopted = !cfg.install.done;
        cfg.mark_installed(adopted);
        cfg.touch_healthy();
        let _ = cfg.save();
        state.set_config(cfg.clone());
        let url = r
            .web_url
            .clone()
            .unwrap_or_else(|| format!("http://localhost:{}", cfg.hunter.ports.web));
        // 这句话**逐条按刚才真做过的事拼**（红线 1）：
        // 收回过端口就得承认重建过那几个容器，不能一律说「什么都没动」。
        let title = match fixed {
            None if self.rebound.is_empty() => "不用重装：Hunter 已经在运行",
            None => "不用重装：只把端口收回了本机",
            Some(_) => "不用重装：只把不正常的那部分弄好了",
        };
        let mut did: Vec<String> = Vec::new();
        if let Some(log) = fixed {
            did.push(log.human());
        }
        if !self.rebound.is_empty() {
            did.push(format!(
                "把 {} 的端口收回本机（这几个容器重建过）",
                self.rebound.join("、")
            ));
        }
        let detail = format!(
            "{} / {} 个服务健康，网页打得开。这一次没有重新取配置、没有重新下载镜像。{}",
            r.ready,
            r.total,
            if did.is_empty() {
                "正在跑的容器一个都没动过。".to_string()
            } else {
                format!("做过的只有：{}。别的容器一个都没动。", did.join("；"))
            }
        );
        self.bus.emit(
            EventDraft::new(Kind::Resolved, title)
                .status(Status::Ok)
                .detail(detail)
                .tech(format!("复查用时 {} 毫秒", r.elapsed_ms)),
        );
        self.bus.emit(
            EventDraft::new(Kind::Step, "完成")
                .status(Status::Ok)
                .detail(format!("打开 {url} 就能用了")),
        );
        self.tick("完成");
        crate::linfo!("开工前复查发现这一套已经在正常运行，本次不做任何安装动作");
        Outcome {
            ok: true,
            code: None,
            message: None,
            solved: self.solved,
            rounds: self.rounds,
            tokens: self.tokens,
            elapsed_ms: self.t0.elapsed().as_millis() as u64,
            url: Some(url),
            takeover: None,
            reused: true,
        }
    }

    /// 只修不正常的那几个服务。
    ///
    /// * `Ok(Some(o))` —— 修好了，直接收工（这一次同样没有下载任何东西）
    /// * `Ok(None)` —— 没修好，交给「启动服务」那一步的修复回合接着办
    /// * `Err(o)` —— 被取消了（`Outcome` 有点大，装箱一下，免得这条 `Result`
    ///   把每一次正常返回都变成一次大结构体拷贝）
    fn fix_only(
        &mut self,
        state: &AppState,
        r: crate::selfcheck::Review,
    ) -> std::result::Result<Option<Outcome>, Box<Outcome>> {
        if self.cancelled() {
            return Err(Box::new(self.fail(Code::Unknown, "安装被取消了。")));
        }
        self.recover_bindings(&r);

        // Docker / 内置虚拟机那一层先过一遍（虚拟机没有 DNS 就是在这里修的）。
        // 它不碰 compose、不碰 .env、不拉任何镜像。
        if let Err(e) =
            self.run_step_with_repair(state, &mut InstallOptions::default(), Step::Docker)
        {
            crate::lwarn!("复查后修 Docker 这一层没过：{}", e.msg);
            return Ok(None);
        }

        let t = Instant::now();
        let ev = self.bus.emit(
            EventDraft::new(Kind::Action, "只把不正常的那几个服务弄起来").detail(format!(
                "{}（其余 {} 个健康的一个都不动，也不重新下载任何东西）",
                r.unready.join("、"),
                r.total.saturating_sub(r.unready.len())
            )),
        );
        self.tick("修不正常的服务");
        let fixed = match crate::selfcheck::fix_unready(&r) {
            Ok(log) => {
                self.bus.finish(
                    ev,
                    Status::Ok,
                    &log.human(),
                    Some(t.elapsed().as_millis() as u64),
                );
                log
            }
            Err(e) => {
                self.bus.finish(
                    ev,
                    Status::Failed,
                    &e.msg,
                    Some(t.elapsed().as_millis() as u64),
                );
                return Ok(None);
            }
        };

        // 等它们就绪，再复查一次。**验证员的规矩：重跑一遍才算数**
        let bus = self.bus.clone();
        let wait = self
            .bus
            .emit(EventDraft::new(Kind::Verify, "等它们就绪").status(Status::Running));
        let waited = compose::wait_healthy(state.config().hunter.start_timeout(), move |v| {
            let ready = v
                .iter()
                .filter(|s| matches!(s.health, compose::Health::Healthy))
                .count();
            bus.finish(wait, Status::Running, &format!("{ready} / 6 健康"), None);
        });
        match waited {
            Ok(v) => self
                .bus
                .finish(wait, Status::Ok, &format!("{} / 6 健康", v.len()), None),
            Err(e) => {
                self.bus.finish(wait, Status::Failed, &e.msg, None);
                return Ok(None);
            }
        }

        let again = crate::selfcheck::review(true);
        if !again.all_good() {
            crate::lwarn!("只修不正常的那部分之后还是不行：{}", again.headline);
            return Ok(None);
        }
        self.solved += 1;
        Ok(Some(self.reuse_outcome(state, &again, Some(&fixed))))
    }

    /// 容器被谁重建成 0.0.0.0 了就按覆盖文件收回来（I11 · U5）。
    ///
    /// 这不是「改用户的配置」——磁盘上那份覆盖文件说的就是 127.0.0.1，
    /// 是**跑着的容器和配置不一致**。让现实回到配置，不是做一个新决定。
    fn recover_bindings(&mut self, r: &crate::selfcheck::Review) {
        if r.drift.is_empty() {
            return;
        }
        let t = Instant::now();
        let names: Vec<String> = r.drift.iter().map(|d| d.service.clone()).collect();
        let ev = self.bus.emit(
            EventDraft::new(Kind::Issue, "有容器的端口开到了本机之外")
                .status(Status::Warn)
                .detail(format!(
                    "{} 现在绑在本机以外的地址上，同一个局域网里的其他机器能连到它",
                    names.join("、")
                )),
        );
        for d in &r.drift {
            self.bus.emit(
                EventDraft::new(Kind::Analyze, format!("{} 的现状", d.service))
                    .under(ev)
                    .status(Status::Ok)
                    .detail(d.human())
                    .tech("多半是有人在 ~/.hunter/app 里直接跑了 docker compose 而没带上启动器的覆盖文件"),
            );
        }
        match compose::fix_bind_drift(&r.drift) {
            Ok(()) => {
                self.solved += 1;
                self.bus.emit(
                    EventDraft::new(Kind::Resolved, "已经按启动器的配置收回本机")
                        .under(ev)
                        .status(Status::Ok)
                        .detail(format!(
                            "重建了 {}，现在只有这台电脑能打开它们",
                            names.join("、")
                        ))
                        .tech(format!("用时 {} 毫秒", t.elapsed().as_millis())),
                );
                self.rebound = names.clone();
                crate::linfo!("端口绑定漂移已按覆盖文件收回：{}", names.join("、"));
            }
            Err(e) => {
                self.bus.emit(
                    EventDraft::new(Kind::Failed, "没能把它们收回本机")
                        .under(ev)
                        .status(Status::Failed)
                        .detail(e.msg.clone()),
                );
                crate::lwarn!("端口绑定漂移没收回来：{}", e.msg);
            }
        }
    }

    fn fail(&mut self, code: Code, msg: &str) -> Outcome {
        self.bus.emit(
            EventDraft::new(Kind::Failed, "没能自动装好")
                .status(Status::Failed)
                .detail(msg)
                .tech(format!("错误码 {}", code.as_str())),
        );
        self.tick("失败");
        crate::lerror!("AI 自动安装失败（{}）：{msg}", code.as_str());
        Outcome {
            ok: false,
            code: Some(code.as_str().to_string()),
            message: Some(crate::redact::mask_home(&crate::redact::redact(msg))),
            solved: self.solved,
            rounds: self.rounds,
            tokens: self.tokens,
            elapsed_ms: self.t0.elapsed().as_millis() as u64,
            url: None,
            takeover: None,
            reused: false,
        }
    }

    /// 跑一步；失败就进修复回合，修好了**重跑这一步**（这就是验证员）。
    fn run_step_with_repair(
        &mut self,
        state: &AppState,
        opts: &mut InstallOptions,
        step: Step,
    ) -> AppResult<()> {
        let mut issue_rounds = 0usize;
        let mut last_err: Option<AppError> = None;
        loop {
            if self.cancelled() {
                return Err(AppError::new(Code::Unknown, "安装被取消了。"));
            }
            let t = Instant::now();
            let ev = self.bus.emit(EventDraft::new(Kind::Step, step.title()));
            self.tick(step.title());
            match self.run_step(state, opts, step, ev) {
                Ok(detail) => {
                    self.bus.finish(
                        ev,
                        Status::Ok,
                        &detail,
                        Some(t.elapsed().as_millis() as u64),
                    );
                    // 这一步里如果靠代理救了场，现在把它如实报出来
                    self.note_proxy_rescues(ev);
                    if let Some(e) = last_err.take() {
                        // 上一圈失败的那一步这回过了 —— 验证员判定「修好了」
                        self.solved += 1;
                        self.open = self.open.saturating_sub(1);
                        crate::linfo!("修好了：{}（原错误 {}）", step.title(), e.code.as_str());
                    }
                    return Ok(());
                }
                Err(e) => {
                    self.bus.finish(
                        ev,
                        Status::Failed,
                        &e.msg,
                        Some(t.elapsed().as_millis() as u64),
                    );
                    issue_rounds += 1;
                    self.open += if last_err.is_none() { 1 } else { 0 };
                    last_err = Some(e.clone());

                    if let Some(stop) = self.budget_stop(issue_rounds) {
                        self.bus.emit(
                            EventDraft::new(Kind::Failed, "不再继续试了")
                                .status(Status::Failed)
                                .detail(stop),
                        );
                        return Err(e);
                    }
                    // 修复回合：侦察 → 诊断 → 守卫 → 执行
                    match self.repair(state, opts, step, &e, issue_rounds) {
                        Ok(true) => continue, // 下一圈的 run_step 就是验证
                        Ok(false) => return Err(e),
                        Err(re) => {
                            crate::lwarn!("修复回合本身出错了：{}", re.msg);
                            return Err(e);
                        }
                    }
                }
            }
        }
    }

    /// 预算到头了吗。到了就**如实说清是哪一条到头了**，不含糊其辞。
    fn budget_stop(&self, issue_rounds: usize) -> Option<String> {
        if issue_rounds >= MAX_ROUNDS_PER_ISSUE {
            return Some(format!(
                "同一个问题已经试了 {MAX_ROUNDS_PER_ISSUE} 回合还没解决，不再继续猜了。"
            ));
        }
        if self.rounds >= MAX_ROUNDS_TOTAL {
            return Some(format!(
                "这次安装一共用掉了 {MAX_ROUNDS_TOTAL} 个修复回合，到上限了。"
            ));
        }
        if self.tokens >= MAX_TOKENS {
            return Some(format!(
                "这次安装已经用掉 {} token（上限 {MAX_TOKENS}），不再继续问模型了。",
                self.tokens
            ));
        }
        None
    }

    /// 真正跑一步。返回给用户看的那句「结果」（真实数字）。
    fn run_step(
        &mut self,
        state: &AppState,
        opts: &InstallOptions,
        step: Step,
        parent: u64,
    ) -> AppResult<String> {
        match step {
            Step::Docker => {
                crate::runtime::which::invalidate();
                crate::runtime::env::invalidate();
                // **先问「现在生效的运行时是谁」**（I9 的 P0-1）。
                //
                // 0.1.8 这里第一句就是 `docker::detect()`，而它找到的那个 docker
                // 是我们自己下到 `~/.hunter/runtime/bin` 里的那一份（`which` 有意
                // 把它排在最前面）。于是「我们自己那台虚拟机没起来」被报成了
                // 「用户的 Docker 装了没运行」，内置主线一步都没走到。
                let eff = crate::runtime::effective::current();
                crate::linfo!("{}", eff.one_line());
                if eff.builtin_down() {
                    return Err(AppError::new(
                        Code::BuiltinRuntimeDown,
                        format!(
                            "Hunter 自己那套运行时{}。这不是你装的 Docker —— \
                             它是上一次安装下到 {} 里的，该走内置那条路把它起起来。",
                            eff.builtin.cn(),
                            crate::redact::mask_home(
                                &crate::paths::runtime_dir().to_string_lossy()
                            )
                        ),
                    ));
                }
                let d = crate::runtime::docker::detect();
                if !d.installed {
                    return Err(AppError::new(
                        Code::DockerMissing,
                        "这台机器上找不到 docker 可执行文件。".to_string(),
                    ));
                }
                if !d.daemon_running {
                    return Err(AppError::new(
                        Code::DaemonDown,
                        "docker 命令在，但连不上后台服务（daemon 没起）。".to_string(),
                    ));
                }
                let label = d
                    .runtime_label
                    .clone()
                    .unwrap_or_else(|| "Docker".to_string());
                // **就绪判定里多一步：它里面解析得动域名吗**（I10 的 P0-2）。
                //
                // 只对 Hunter 自己那台虚拟机做 —— 用户自己的 Docker 的 DNS 是他
                // 这台电脑的事，启动器不碰（红线）。
                let dns = self.ensure_vm_dns(parent)?;
                Ok(match dns {
                    Some(line) => format!("{label} 正在运行 · {line}"),
                    None => format!("{label} 正在运行"),
                })
            }
            Step::Prepare => {
                // 侦察员顺手报一句「你这台机器上已经有一套 Hunter」
                self.report_other_installs(parent);
                let bus = self.bus.clone();
                let prep = flow::prepare(state, opts, |line| {
                    bus.emit(
                        EventDraft::new(Kind::Action, line)
                            .under(parent)
                            .status(Status::Ok),
                    );
                })?;
                let changed = state
                    .port_changes
                    .lock()
                    .map(|g| g.clone())
                    .unwrap_or_default();
                if !changed.is_empty() {
                    // 预检阶段自己把端口换掉了 —— **这就是解决掉一个问题**（I7）。
                    //
                    // 0.1.6 在用户 Mac 上的现场：5 个端口全被 hunter-fresh 占着，
                    // 启动器换了一组新的、装成功了，总览那一行却写「已自动解决 0 个问题」。
                    // 从代码角度那是因为 `solved` 只在「某一步失败又重跑成功」时才加；
                    // 从用户角度，他明明看见启动器发现问题、处理掉了。
                    // 用户是对的：**没等到报错就解决掉，不该因此不算数。**
                    self.solved += 1;
                    self.tick(&self.phase.clone());
                    // 端口换过就明说换到哪、原来被谁占着（真实值，不是「某个程序」）
                    let e = self.bus.emit(
                        EventDraft::new(Kind::Resolved, "为这次安装换了一组空闲端口")
                            .under(parent)
                            .status(Status::Ok)
                            .detail(
                                changed
                                    .iter()
                                    .map(|c| format!("{} {} → {}", c.service, c.wanted, c.actual))
                                    .collect::<Vec<_>>()
                                    .join(" · "),
                            ),
                    );
                    for c in &changed {
                        if c.occupied_by.is_empty() {
                            continue;
                        }
                        // 人话那一行只说「谁占着」；绑定被系统拒绝的原文属于技术细节
                        let parts: Vec<&str> = c.occupied_by.split('；').collect();
                        let human = parts
                            .iter()
                            .find(|p| p.starts_with("Docker 容器") || p.starts_with("进程"))
                            .copied()
                            .unwrap_or(parts[0]);
                        let mut d =
                            EventDraft::new(Kind::Analyze, format!("{} 原来被占着", c.wanted))
                                .under(e)
                                .status(Status::Ok)
                                .detail(human);
                        for p in &parts {
                            d = d.tech((*p).to_string());
                        }
                        self.bus.emit(d);
                    }
                }
                // 有源连不上、启动器自动换到另一个 —— 也是解决掉一个问题（I8 一.4）
                if !prep.registry_skipped.is_empty() {
                    self.solved += 1;
                    self.tick(&self.phase.clone());
                    let mut d = EventDraft::new(
                        Kind::Resolved,
                        format!(
                            "有 {} 个下载源连不上，已自动改用「{}」",
                            prep.registry_skipped.len(),
                            prep.registry_label
                        ),
                    )
                    .under(parent)
                    .status(Status::Ok)
                    .detail("你不用做任何事，启动器自己挑了一个测得通的源");
                    for sk in &prep.registry_skipped {
                        d = d.tech(sk.clone());
                    }
                    self.bus.emit(d);
                }
                self.note_proxy_rescues(parent);
                let line = format!(
                    "下载源 {} · 端口 web {} · api {} · opencode {} · postgres {} · redis {}",
                    prep.registry_label,
                    prep.ports.web,
                    prep.ports.api,
                    prep.ports.opencode,
                    prep.ports.postgres,
                    prep.ports.redis
                );
                self.prep = Some(prep);
                Ok(line)
            }
            Step::Pull => {
                // 上一步算好的那一份；没有（例如从错误页重试直接跳到这里）才重算
                let prep = match self.prep.take() {
                    Some(p) => p,
                    None => flow::prepare(state, opts, |_| {})?,
                };
                let t = Instant::now();
                let bus = self.bus.clone();
                // 不另开一条子事件：这一步本身就是「下载组件」，
                // 再挂一条一模一样的子卡片只会让界面上出现两行同样的话（GUI 实测撞到）
                let ev = parent;
                self.bus
                    .finish(ev, Status::Running, "六个镜像，正在拉取…", None);
                let mut last_pct = 0u32;
                flow::pull(state, &prep, opts, move |p| {
                    // 每 10% 更新一次文字，不要一秒刷十行
                    if p.percent >= last_pct + 10 || p.percent == 100 {
                        last_pct = p.percent;
                        bus.finish(
                            ev,
                            Status::Running,
                            // 「已完成」不是「已下载」：本机已有的层也算进百分比，
                            // 但它们一个字节都没过网（收尾那一行会把两者分清楚）
                            &format!(
                                "{}% · 已完成 {} / {}",
                                p.percent,
                                flow::human_bytes(p.downloaded_bytes),
                                flow::human_bytes(p.total_bytes)
                            ),
                            None,
                        );
                    }
                })?;
                // **实际过了多少网**从进度快照里取。`downloaded_bytes` 把本机已有的层
                // 也算在内（它是进度条的分子），拿它去说「下载了 849 MB、用时 1 秒」
                // 就是在骗人 —— 那 849 MB 一个字节都没走网络（红线 1）。
                let snap = state.pull.lock().map(|g| g.clone()).ok();
                let net = snap.as_ref().map(|p| p.net_bytes).unwrap_or(0);
                let total: u64 = prep.sizes.values().sum();
                let secs = fmt_secs(t.elapsed().as_secs());
                let detail = match (net, total) {
                    (0, t) if t > 0 => format!(
                        "六个镜像本机已有（合计 {}），这次一个字节都没下载 · 用时 {secs}",
                        flow::human_bytes(t)
                    ),
                    (0, _) => format!("镜像本机已有，没有重新下载 · 用时 {secs}"),
                    (n, t) if t > 0 && n < t => format!(
                        "下载 {} / 共 {}（其余的层本机已有）· 用时 {secs}",
                        flow::human_bytes(n),
                        flow::human_bytes(t)
                    ),
                    (n, _) => format!("下载 {} · 用时 {secs}", flow::human_bytes(n)),
                };
                self.prep = Some(prep);
                // **镜像到手了，先问一句「容器连得上模型网关吗」**（I10 的 P0-2）。
                //
                // 放在这里而不是放在「启动服务」之后，是因为 0.1.9 那次的教训：
                // 等到健康检查超时才发现，白白多等 13 分钟，而且拿到的错误码
                // （「启动超时」）根本指不到根上。现在用的是刚拉下来的镜像，
                // **一个字节都不用多下**。
                self.gate_container_network(parent)?;
                Ok(detail)
            }
            Step::Start => {
                let bus = self.bus.clone();
                let ev = parent;
                self.bus.finish(ev, Status::Running, "正在起容器…", None);
                let r = flow::start(state, move |v| {
                    let ready = v
                        .iter()
                        .filter(|s| matches!(s.health, compose::Health::Healthy))
                        .count();
                    bus.finish(ev, Status::Running, &format!("{ready} / 6 健康"), None);
                });
                match r {
                    Ok(v) => Ok(format!("{} / 6 健康", v.len())),
                    Err(e) => Err(e),
                }
            }
        }
    }

    // ── 侦察员 ────────────────────────────────────────────────────────────

    /// 内置运行时就绪判定里的那一步：**它里面解析得动域名吗**（I10 的 P0-2）。
    ///
    /// 三种结局，都如实说：
    ///
    /// | 现场 | 做什么 |
    /// |---|---|
    /// | 不是内置运行时 / 虚拟机没跑 | 什么都不做，返回 `None`（这一项对用户自己的 Docker 没有意义） |
    /// | 解析得动 | 什么都不做，返回一句「DNS 正常」 |
    /// | 解析不动 | **自动修**（[`crate::runtime::vmdns::ensure`]），修不好就 `E_RUNTIME_NO_DNS` |
    fn ensure_vm_dns(&mut self, parent: u64) -> AppResult<Option<String>> {
        if crate::runtime::vmdns::applicable().is_err() {
            return Ok(None);
        }
        let before = match crate::runtime::vmdns::probe() {
            Ok(d) => d,
            Err(e) => {
                // 查不成不等于坏了。**不拿「没查成」当「不通」**
                self.say_cannot(parent, "查一下那台虚拟机的 DNS", &e.msg);
                return Ok(None);
            }
        };
        if before.healthy() {
            return Ok(Some("虚拟机里域名解析正常".to_string()));
        }
        // 到这儿就是真的没有 DNS。**先把这件事当成一个问题报出来**，再修
        let issue = self.bus.emit(
            EventDraft::new(
                Kind::Issue,
                "Hunter 自己那台虚拟机没有 DNS，容器会连不上网",
            )
            .under(parent)
            .status(Status::Warn)
            .detail(
                "这台虚拟机自带的系统镜像里没有 systemd-resolved，/etc/resolv.conf 出厂就是一条断链 —— \
                 不修的话，健康检查过不去，而且就算过去了也连不上 Hunter 的模型网关。",
            )
            .tech(before.one_line()),
        );
        let t = Instant::now();
        let bus = self.bus.clone();
        let act = self
            .bus
            .emit(EventDraft::new(Kind::Action, "正在给这台虚拟机写 DNS…").under(issue));
        let mut say = |line: &str| {
            bus.finish(act, Status::Running, line, None);
            crate::linfo!("{line}");
        };
        let mut nb = |_: u64, _: u64| {};
        let cancel = self.cancel.clone();
        let no_cancel = move || cancel.load(Ordering::SeqCst);
        let mut pr = crate::runtime::builtin::Progress {
            say: &mut say,
            bytes: &mut nb,
            cancel: &no_cancel,
        };
        let r = crate::runtime::vmdns::ensure(&mut pr);
        let after = match r {
            Ok(d) => d,
            Err(e) => {
                self.bus.finish(
                    act,
                    Status::Failed,
                    &e.msg,
                    Some(t.elapsed().as_millis() as u64),
                );
                return Err(AppError::new(
                    Code::RuntimeNoDns,
                    format!("给那台虚拟机写 DNS 没成：{}", e.msg),
                ));
            }
        };
        self.bus.finish(
            act,
            if after.healthy() {
                Status::Ok
            } else {
                Status::Failed
            },
            &after.one_line(),
            Some(t.elapsed().as_millis() as u64),
        );
        if !after.healthy() {
            return Err(AppError::new(
                Code::RuntimeNoDns,
                format!("修完之后那台虚拟机还是解析不了域名：{}", after.one_line()),
            ));
        }
        self.solved += 1;
        self.tick(&self.phase.clone());
        self.bus.emit(
            EventDraft::new(Kind::Resolved, "虚拟机的 DNS 已经修好")
                .under(issue)
                .status(Status::Ok)
                .detail(format!(
                    "写的是 {} —— 只动 Hunter 自己那台虚拟机，你电脑的网络设置一个字节都没改",
                    crate::runtime::vmdns::NAMESERVERS
                        .iter()
                        .map(|(ns, _)| *ns)
                        .collect::<Vec<_>>()
                        .join(" · ")
                ))
                .tech(after.one_line()),
        );
        Ok(Some("虚拟机的 DNS 刚刚修好了".to_string()))
    }

    /// 「容器到底能不能连上模型网关」这道闸门（I10 的 P0-2）。
    ///
    /// **六个服务全绿也不等于能对话** —— 0.1.9 那次就是：健康检查是外网依赖，
    /// 容器连网关也不通，用户拿到的会是一个起得来、问不出话的 Hunter。
    ///
    /// 探不成（本机还没有镜像、docker 跑不起来）**不算不通**，如实说一句就过。
    fn gate_container_network(&mut self, parent: u64) -> AppResult<()> {
        let t = Instant::now();
        let ev = self.bus.emit(
            EventDraft::new(Kind::Action, "检查容器能不能连上 Hunter 的模型网关").under(parent),
        );
        let mut o = crate::runtime::netcheck::probe();
        crate::linfo!("{}", o.one_line());
        // **不通就再试一次。** 网络抖一下就把整次安装判失败太糙了 ——
        // 而这一项的代价只是再起一个一次性容器（几秒），比误判一次便宜得多。
        // 「没探成」那一档不重试：重试一百次本机也不会凭空多出一个镜像。
        if !o.ok && o.fail != crate::runtime::netcheck::Fail::NotRun {
            self.bus
                .finish(ev, Status::Running, "第一次没通，3 秒后再试一次…", None);
            std::thread::sleep(Duration::from_secs(3));
            let again = crate::runtime::netcheck::probe();
            crate::linfo!("再试一次：{}", again.one_line());
            o = again;
        }
        let ms = t.elapsed().as_millis() as u64;
        if o.ok {
            self.bus.finish(ev, Status::Ok, &o.one_line(), Some(ms));
            return Ok(());
        }
        if o.fail == crate::runtime::netcheck::Fail::NotRun {
            // 没探成。**说清楚是「没测」不是「不通」**（红线 1）
            self.bus.finish(
                ev,
                Status::Warn,
                &format!("这一项没能测（{}），继续往下装", o.detail),
                Some(ms),
            );
            return Ok(());
        }
        self.bus.finish(ev, Status::Failed, &o.one_line(), Some(ms));
        Err(AppError::new(
            Code::ContainerOffline,
            format!("{}。命令：{}", o.one_line(), o.command),
        ))
    }

    /// 「直连不通、改走你自己配的系统代理之后成功了」——**这也是一次自动修复**，
    /// 如实报一张卡片并计进「已自动解决 N 个问题」（I8 一.4）。
    ///
    /// 只说主机名与「已沿用」，不打代理地址里可能有的用户名密码。
    fn note_proxy_rescues(&mut self, parent: u64) {
        let hosts = crate::netproxy::rescued_hosts();
        let fresh: Vec<String> = hosts
            .into_iter()
            .filter(|h| !self.reported_proxy.contains(h))
            .collect();
        if fresh.is_empty() {
            return;
        }
        for h in &fresh {
            self.reported_proxy.insert(h.clone());
        }
        self.solved += 1;
        self.tick(&self.phase.clone());
        self.bus.emit(
            EventDraft::new(
                Kind::Resolved,
                format!("直连 {} 不通，已沿用你设置的网络代理", fresh.join("、")),
            )
            .under(parent)
            .status(Status::Ok)
            .detail(crate::netproxy::current().one_line())
            .tech("只读取你系统里已有的代理设置，不会修改它，也不会改 DNS / hosts / 防火墙。"),
        );
    }

    /// 本机上别的 Hunter —— 一次安装里只报一次。
    ///
    /// ## I8：这里不再问用户，自己拿主意
    ///
    /// I7 的做法是一张**不阻塞**的卡片，上面两个按钮：「和它并存」与「直接用它」。
    /// 用户 2026-09-21 22:35 把「让用户参与决策」整类做法否了，于是这里改成：
    ///
    /// > **自动选「并存、换端口」** —— 它是风险最小的那个方案：
    /// > 不停用户已有的那套、不删它的数据、不改它的配置，只给新装的这一套
    /// > 挑一组空闲端口。
    ///
    /// 卡片还在，但它现在是一句**告知**：做了什么决定、为什么这么定、
    /// 想改的话去哪儿改（设置页）。没有按钮，也不等任何人。
    ///
    /// 「直接用已有那套」这条路并没有消失 —— [`Handle::answer`] 仍然认
    /// [`TAKEOVER_PREFIX`]，设置页与命令行 `--takeover` 走的就是它。
    fn report_other_installs(&self, parent: u64) {
        if self.reported_others.lock().map(|g| *g).unwrap_or(true) {
            return;
        }
        let cands = crate::takeover::candidates();
        if let Ok(mut g) = self.reported_others.lock() {
            *g = true;
        }
        if cands.is_empty() {
            return;
        }
        let who: Vec<String> = cands.iter().map(|o| o.one_line()).collect();
        let mut d = EventDraft::new(Kind::Resolved, "你电脑上已经在运行另一套 Hunter")
            .under(parent)
            .status(Status::Ok)
            .detail(format!(
                "{}。已自动选择「和它并存」：新装的这一套换一组空闲端口，\
                 你原来那套一点都不动 —— 这是风险最小的做法。\
                 想改成直接使用已有那套，到「设置 → 已有的 Hunter」里切换。",
                who.join("；")
            ))
            .tech(
                "并存的做法：新装的这一套换一组空闲端口。\
                 启动器不会停它、不会删它、不会改它的配置。",
            );
        for c in &cands {
            if c.manageable() {
                d = d.tech(format!(
                    "「{}」的 compose 文件在 {}，所以在设置里切过去之后能看状态、看日志，\
                     也能停 / 重启（每次都要再确认一遍）",
                    c.project,
                    crate::redact::mask_home(&c.working_dir)
                ));
            } else {
                d = d.tech(format!(
                    "「{}」的容器上没有 working_dir 标签，读不到它的 compose 文件 —— \
                     切过去之后只能看状态与日志，停 / 重启做不了",
                    c.project
                ));
            }
        }
        self.bus.emit(d);
    }

    fn maybe_takeover(&mut self) -> Option<Outcome> {
        let want = self.takeover.lock().ok().and_then(|mut g| g.take())?;
        let ev = self.bus.emit(
            EventDraft::new(Kind::Action, format!("改为管理你已有的「{want}」"))
                .status(Status::Running),
        );
        // 走正规通路：动作表 → 守卫 → 审计。`confirmed = true` 的依据是
        // 用户**亲手点了那个按钮**，不是我们替他决定的
        let call = Call::with("reuse_existing_hunter", "project", &want);
        match actions::execute_as(&call, self.mode, true, Proposer::User) {
            Ok(o) => {
                self.bus.finish(ev, Status::Ok, &first_line(&o.text), None);
                let cfg = LauncherConfig::load();
                let url = if cfg.takeover.web_port > 0 {
                    Some(format!("http://localhost:{}", cfg.takeover.web_port))
                } else {
                    None
                };
                self.bus.emit(
                    EventDraft::new(Kind::Step, "完成")
                        .status(Status::Ok)
                        .detail(match &url {
                            Some(u) => format!("启动器现在管理「{want}」，打开 {u} 就能用"),
                            None => format!("启动器现在管理「{want}」（读不到它的 web 端口）"),
                        }),
                );
                self.tick("完成");
                crate::linfo!("用户选择接管本机已有的 {want}，本次不再安装");
                Some(Outcome {
                    ok: true,
                    code: None,
                    message: None,
                    solved: self.solved,
                    rounds: self.rounds,
                    tokens: self.tokens,
                    elapsed_ms: self.t0.elapsed().as_millis() as u64,
                    url,
                    takeover: Some(want),
                    reused: true,
                })
            }
            Err(e) => {
                // 接不管就如实说，并**继续照原样装** —— 不把用户卡在这里
                self.bus.finish(
                    ev,
                    Status::Failed,
                    &format!("{}（这次仍然按「和它并存」继续装）", e.msg),
                    None,
                );
                crate::lwarn!("接管 {want} 没成功：{}", e.msg);
                None
            }
        }
    }

    /// 采一份证据（零 token）。**这是 0.1.4 最缺的东西** ——
    /// 当时送给模型的诊断里没有「端口被谁占着」，模型只好自己编一个原因。
    fn scout(&self, step: Step, e: &AppError) -> Evidence {
        let report = probe::collect(Some(e.code.as_str()), Some(&e.msg), Some(step.as_str()));
        let survey = crate::ports::Survey::collect();
        let cfg = LauncherConfig::load();
        let mut port_lines = Vec::new();
        for (name, port) in cfg.hunter.ports.as_pairs() {
            let v = survey.verdict(port, &[crate::config::PROJECT]);
            port_lines.push(format!("{name} {}", v.human()));
        }
        // **容器能不能上网**这件事只在该问的时候问（I10）。
        //
        // 0.1.9 那次失败里诊断助手读了 4 次容器日志都没看出根因 —— 因为
        // 「opencode 的健康检查返回 500」这句话里，看不出「它解析不了域名」。
        // 那条事实只有**真的去解析一次**才会出现，所以这里由侦察员去做，
        // 而不是指望模型从日志里猜。
        let container_net = if matches!(
            e.code,
            Code::StartTimeout | Code::ContainerOffline | Code::RuntimeNoDns | Code::ProxyBlock
        ) && matches!(step, Step::Pull | Step::Start)
        {
            let o = crate::runtime::netcheck::probe();
            crate::linfo!("侦察员：{}", o.one_line());
            Some(o)
        } else {
            None
        };
        // 「容器手里那份 DNS 过时了吗」只在虚拟机 DNS 现在是好的、
        // 而服务又起不来的时候才值得查（那正是 P0-3 那个现场）
        let stale_dns = if matches!(step, Step::Start)
            && matches!(e.code, Code::StartTimeout | Code::ContainerOffline)
            && report
                .vm_dns
                .as_ref()
                .is_some_and(|d| d.resolves == Some(true))
        {
            crate::runtime::netcheck::stale_dns_containers(
                &crate::runtime::netcheck::want_nameservers(),
            )
        } else {
            Vec::new()
        };
        Evidence {
            report,
            others: crate::takeover::candidates(),
            stale: compose::stale_own_containers(),
            port_lines,
            cred_helpers: crate::dockercfg::helper_status(),
            sub_env: crate::runtime::env::current().one_line(),
            container_net,
            stale_dns,
            notes: Vec::new(),
        }
    }

    // ── 修复回合 ──────────────────────────────────────────────────────────

    /// 一个修复回合：侦察 → 诊断 → 守卫 → 执行。
    /// 返回 `Ok(true)` 表示做了点什么，值得重跑这一步；`Ok(false)` 表示无计可施。
    fn repair(
        &mut self,
        state: &AppState,
        opts: &mut InstallOptions,
        step: Step,
        e: &AppError,
        issue_rounds: usize,
    ) -> AppResult<bool> {
        self.rounds += 1;
        self.tick(&format!("正在解决：{}", e.code.title()));

        let issue = self.bus.emit(
            EventDraft::new(Kind::Issue, narrate_issue(e))
                .status(Status::Warn)
                .detail(narrate_detail(e))
                .tech(format!("错误码 {}｜{}", e.code.as_str(), e.msg)),
        );

        let t_scout = Instant::now();
        let an = self
            .bus
            .emit(EventDraft::new(Kind::Analyze, "分析中…").under(issue));
        let mut ev = self.scout(step, e);
        self.bus.finish(
            an,
            Status::Ok,
            &ev.one_line(),
            Some(t_scout.elapsed().as_millis() as u64),
        );

        // 换过一次源、原话一个字都没变 —— 那就不是源的问题（I6）。
        // 说出来，并且把这句话一起交给诊断员，免得模型又提「再换个源」
        let fp = error_fingerprint(&e.msg);
        if self.switched_registry.contains(&fp) {
            let note = "上一回合已经换过下载源了，失败的原话一个字都没变，说明这件事与下载源无关。";
            ev.notes.push(note.to_string());
            self.bus.emit(
                EventDraft::new(Kind::Analyze, "这不是下载源的问题")
                    .under(issue)
                    .status(Status::Ok)
                    .detail(note)
                    .tech(format!("原话：{}", e.msg)),
            );
        }

        // 第一层规则 → 第二层模型 → **复核员**（只有含 Sensitive 的计划才走）。
        //
        // 复核被否决时退回诊断员，**并且把否决理由一起交给它**——
        // 不带理由地重问一遍，模型多半会原样再提一次同一个计划。
        // 整个循环最多两趟：第二趟还被否决就不再猜了。
        let mut skip_rules = false;
        let (calls, why, by) = 'plan: loop {
            let first = if skip_rules {
                None
            } else {
                self.rule_plan(step, e, &ev)
            };
            let got = match first {
                Some((c, w)) => Some((c, w, Proposer::Rule)),
                None => self
                    .ask_model(issue, step, e, &ev)
                    .map(|(c, w)| (c, w, Proposer::Model)),
            };
            let Some((calls, why, by)) = got else {
                // 规则与模型都没辙了。要不要请用户出手在函数末尾统一判，
                // 免得同一个问题问两遍
                self.bus.emit(
                    EventDraft::new(Kind::Failed, "这个问题我解决不了")
                        .under(issue)
                        .status(Status::Failed)
                        .detail("没有可以安全执行的办法"),
                );
                if let Some((what, why)) = cannot_do(e) {
                    self.say_cannot(issue, &what, &why);
                }
                return Ok(false);
            };
            if calls.is_empty() {
                return Ok(false);
            }
            if !super::reviewer::needs_review(&calls) {
                break 'plan (calls, why, by);
            }
            let v = self.review(issue, &calls, &why, &ev);
            if v.approve {
                break 'plan (calls, why, by);
            }
            // 否决。理由进证据，下一趟诊断员看得到
            let note = format!(
                "复核员否决了上一个计划（{}）：{}",
                calls
                    .iter()
                    .map(|c| c.id.as_str())
                    .collect::<Vec<_>>()
                    .join("、"),
                v.reasons.join("；")
            );
            crate::lwarn!("{note}");
            ev.notes.push(format!(
                "{note}。换一个不会碰到这些东西的办法，或者如实说做不到。"
            ));
            if skip_rules {
                // 已经重来过一趟了，第二趟还被否决 —— 不再猜
                self.bus.emit(
                    EventDraft::new(Kind::Failed, "这个办法没能过复核")
                        .under(issue)
                        .status(Status::Failed)
                        .detail("换了一个办法还是没过，不再继续试了"),
                );
                if let Some((what, why)) = cannot_do(e) {
                    self.say_cannot(issue, &what, &why);
                }
                return Ok(false);
            }
            skip_rules = true;
            // 退回诊断员**要计一个回合**（否则模型可以靠不停被否决把预算绕过去）
            self.rounds += 1;
            if let Some(stop) = self.budget_stop(issue_rounds) {
                self.bus.emit(
                    EventDraft::new(Kind::Failed, "不再继续试了")
                        .under(issue)
                        .status(Status::Failed)
                        .detail(stop),
                );
                return Ok(false);
            }
        };

        self.bus.emit(
            EventDraft::new(Kind::Analyze, "找到原因")
                .under(issue)
                .status(Status::Ok)
                .detail(why),
        );

        let mut did = false;
        for call in calls {
            if self.cancelled() {
                return Ok(did);
            }
            let Some(spec) = actions::spec(&call.id) else {
                continue;
            };
            // **方向该由用户定的动作，总指挥一律不替他定**（I8）。
            //
            // 取消「Sensitive 要用户点一下」这道门之后，这一类动作会裸奔：
            // 模型提一句「你已经有一套 Hunter 了，改成用它吧」，启动器就真的不装了。
            // 而用户 2026-09-21 22:35 定的是**自动选「并存、换端口」**，正好相反。
            //
            // 所以这里连 `confirmed` 都不给它 —— [`actions::execute_as`] 那一道
            // 会拒掉，事件流里如实写一行「这件事得你自己开口」。
            if spec.user_only {
                let msg = match actions::execute_as(&call, self.mode, false, by) {
                    Err(e) => e.msg,
                    // execute_as 对 user_only 且未确认的一定返回 Err；走到这里说明
                    // 那一道被谁改坏了，如实报出来而不是当没事发生
                    Ok(_) => "动作表那一道没拦住 user_only 动作，请查 actions::execute_as".into(),
                };
                self.bus.emit(
                    EventDraft::new(Kind::Action, "这件事不由启动器替你定")
                        .under(issue)
                        .status(Status::Skipped)
                        .detail(format!(
                            "模型提议「{}」—— 没有照做。默认做法是两套并存、\
                             给新装的这一套换一组空闲端口，你原来那套一点都不动。\
                             真要改成用已有那套，到「设置 → 已有的 Hunter」里切换。",
                            spec.title
                        ))
                        .tech(msg),
                );
                continue;
            }
            // 「需要你」的第一种情况：Sensitive 动作在任何档位下都要先问 ——
            // **除非用户在一次授权页上已经对这一件事明确说过「可以」**（I7）。
            //
            // 那一项默认勾着，文案是「如果电脑上没有 Docker，允许 AI 为你安装
            // （装在 ~/.hunter/runtime 里，不改系统，可一键卸载）」。
            // 已经授权过的事再问一遍不叫谨慎，叫啰嗦。但**要留痕**：
            // 事件流里照样出一张卡片说明「为什么这次没问你」。
            if self.mode.needs_confirm(spec.level) {
                if self.pre_authorized(&call) {
                    self.bus.emit(
                        EventDraft::new(Kind::Action, "这一步不再问你")
                            .under(issue)
                            .status(Status::Ok)
                            .detail("你在授权页上勾了「电脑上没有 Docker 时允许 AI 为你安装」")
                            .tech(format!(
                                "动作 {}（{}）。想改回「每次都问」：设置页把那一项取消勾选，\
                                 或者改 launcher.toml 的 [assist] allow_install_runtime = false",
                                call.id,
                                spec.level.cn()
                            )),
                    );
                    guard::audit(
                        &call.id,
                        &call.args,
                        Proposer::User,
                        Some(spec.level),
                        "授权页已勾选，免二次确认",
                    );
                } else {
                    let ok = self.ask_user(issue, spec, &call);
                    if !ok {
                        continue;
                    }
                }
            }
            let t = Instant::now();
            let title = actions::plan(&call)
                .map(|p| p.title)
                .unwrap_or_else(|_| spec.title.to_string());
            let aev = self
                .bus
                .emit(EventDraft::new(Kind::Action, format!("正在处理：{title}")).under(issue));
            // `install_runtime` 要下将近 100 MB 再起一台虚拟机，几分钟里界面上
            // 只有一行「正在处理」是不行的 —— 换成带真实字节数的那一版。
            // **三道门一道没少**：这之前刚过了 plan（动作表 + 守卫）与档位判定
            let r = if call.id == "install_runtime" {
                self.run_install_runtime(issue)
                    .map(|text| actions::Outcome {
                        id: call.id.clone(),
                        ok: true,
                        text,
                    })
            } else if call.id == "start_builtin_runtime" || call.id == "repair_builtin_runtime" {
                // 起虚拟机要一分钟起步、重建还可能重下几百 MB ——
                // 和 `install_runtime` 一样走带真实进度的那条执行体（I9）
                self.run_builtin_action(issue, &call)
                    .map(|text| actions::Outcome {
                        id: call.id.clone(),
                        ok: true,
                        text,
                    })
            } else {
                actions::execute_as(&call, self.mode, true, by)
            };
            match r {
                Ok(o) => {
                    self.bus.finish(
                        aev,
                        Status::Ok,
                        &first_line(&o.text),
                        Some(t.elapsed().as_millis() as u64),
                    );
                    if call.id == "switch_registry" {
                        // 这条原话的「换源」名额用掉了。下一回合还是同一条原话的话，
                        // 规则层不会再提换源（I6 · 防来回兜圈）
                        self.switched_registry.insert(fp.clone());
                    }
                    // **有些动作跑成了也不该重跑那一步**（I10）：
                    // 生成一个诊断包不会让容器多出一条 DNS。
                    // 不挡这一下的话就是一个真的空转 —— 实测转了 4 回合、
                    // 22,400 token，每一回合做的都是同一件不解决问题的事。
                    if actions::REPAIRS_NOTHING.contains(&call.id.as_str()) {
                        crate::linfo!(
                            "动作 {} 改变不了失败的原因，不因为它重跑「{}」",
                            call.id,
                            step.title()
                        );
                    } else {
                        did = true;
                    }
                }
                Err(err) => {
                    self.bus.finish(
                        aev,
                        Status::Failed,
                        &err.msg,
                        Some(t.elapsed().as_millis() as u64),
                    );
                    self.failed_actions.insert(call.id.clone());
                    // 后面那些动作多半是接着这一个来的（启动运行时 → 等它就绪），
                    // 前一个没成，后一个只会白等 —— 这一轮到此为止
                    break;
                }
            }
        }
        if did {
            // 动作多半改过 launcher.toml（换端口 / 换源 / 调超时），
            // 把内存里那一份同步过来，否则后面几步还在用旧值
            let fresh = LauncherConfig::load();
            // **换过源就要跟着换**：`opts.registry` 是命令行/界面一开始定死的那个，
            // `switch_registry` 改的是 launcher.toml —— 不同步过来的话，
            // 下一次 prepare 又会把那个拉不动的源钉回去，白换一场
            // （场景 4 首轮实测：换到腾讯云之后还是去连 ghcr）
            if opts.registry.as_deref() != Some(fresh.hunter.registry_id.as_str()) {
                opts.registry = Some(fresh.hunter.registry_id.clone());
            }
            state.set_config(fresh);
            self.bus.emit(
                EventDraft::new(Kind::Verify, "重新试一次刚才失败的那一步")
                    .under(issue)
                    .status(Status::Running)
                    .detail(step.title()),
            );
            return Ok(true);
        }

        // 一个都没成。**但「这个办法没成」不等于「没有别的办法」**（I9）。
        //
        // 刚才失败的那个动作已经进了 `failed_actions`，规则层下一回合看到它
        // 就会换一条路 —— 内置运行时那条链正是这样设计的：
        // `start_builtin_runtime` 起不来 → 下一回合 `repair_builtin_runtime`
        // （清掉残骸重建）。原来这里无条件 `Ok(false)`，于是第二条路
        // **一次都没有机会跑**：测试机上复现内置运行时残骸时，
        // 启动器起了一次虚拟机没成，当场就说「没能自动装好」收场了。
        //
        // 所以先问一句：下一回合还有没有**不一样的**办法？有就接着走
        // （回合上限、token 预算那几道闸在 `run_step_with_repair` 里照常管着）。
        if self.rule_plan(step, e, &ev).is_some() {
            self.bus.emit(
                EventDraft::new(Kind::Verify, "换个办法再试一次")
                    .under(issue)
                    .status(Status::Running)
                    .detail("刚才那个办法没成，还有别的路可以走"),
            );
            return Ok(true);
        }

        // 真的没别的办法了。**不把活儿丢回给用户**（I8 第〇节）：能做的都做过了，
        // 剩下的只有如实说清楚卡在哪，以及诊断包那条路
        if let Some((what, why)) = cannot_do(e) {
            self.say_cannot(issue, &what, &why);
        }
        Ok(false)
    }

    // ── 诊断员 · 第一层（确定性规则） ────────────────────────────────────

    /// 规则层。认得出来就给一串动作与一句「为什么」，**一个 token 都不花**。
    fn rule_plan(&self, step: Step, e: &AppError, ev: &Evidence) -> Option<(Vec<Call>, String)> {
        let (calls, why) = self.rule_plan_raw(step, e, ev)?;
        // 这一次安装里**已经失败过**的动作不再提第二次。
        // 场景 2 首轮实测：`systemctl start docker` 没权限、退出码 1，
        // 规则层每一回合都原样再提一次，三个回合白花 4 分 39 秒。
        //
        // **第一条被滤掉就整条作废**（I9 实测补的）：一个计划里第一条是「做事」的，
        // 后面跟着的多半是「等它生效」。头一条没了，剩下的 `wait_daemon` 单独跑
        // 只会干等到超时 —— 测试机上复现内置运行时残骸时，第 3 回合就是这样
        // **白等了 121 秒**，而且事件流里「找到原因」那张卡片说的还是
        // 「清掉残骸重建」，跟实际做的事对不上（讲解员说了谎，哪怕不是故意的）。
        if calls
            .first()
            .is_some_and(|c| self.failed_actions.contains(&c.id))
        {
            return None;
        }
        let calls: Vec<Call> = calls
            .into_iter()
            .filter(|c| !self.failed_actions.contains(&c.id))
            .collect();
        if calls.is_empty() {
            return None;
        }
        Some((calls, why))
    }

    /// 按内置清单**硬找一遍** docker（待办池 P1-23）。零 token。
    ///
    /// 一个细节：用户如果显式把 `use_builtin_paths` 关掉了，这里就**不越过他的设置**
    /// 去用内置清单 —— 那条设置的语义就是「别用你那份清单」。这时返回 `None`，
    /// 后面该问模型问模型。
    fn hard_find_docker(&self) -> Option<String> {
        crate::runtime::which::invalidate();
        let pr = crate::runtime::which::docker_probe();
        if let Some(p) = pr.resolved.clone() {
            return Some(p);
        }
        let policy = crate::runtime::which::Policy::current();
        if !policy.use_builtin {
            crate::linfo!("硬找 docker 这一步跳过：用户把内置位置清单关掉了");
            return None;
        }
        // `docker_probe` 用的就是这份清单，走到这里说明一个都没命中。
        // 再把内置运行时那一份单独看一眼（它可能刚装好、缓存还没失效）
        crate::runtime::builtin::docker_bin().map(|p| p.to_string_lossy().into_owned())
    }

    fn rule_plan_raw(
        &self,
        step: Step,
        e: &AppError,
        ev: &Evidence,
    ) -> Option<(Vec<Call>, String)> {
        match e.code {
            // ① 端口冲突：这是本轮的主角
            Code::PortConflict | Code::PortInUse => {
                let mut calls = Vec::new();
                let mut why = String::new();
                if !ev.others.is_empty() {
                    why.push_str(&format!(
                        "这些端口属于你之前装的「{}」，不能动它；",
                        ev.others
                            .iter()
                            .map(|o| o.project.clone())
                            .collect::<Vec<_>>()
                            .join("、")
                    ));
                }
                if !ev.stale.is_empty() {
                    why.push_str(&format!(
                        "本次安装还留着 {} 个没起来的容器，它们带着旧端口；",
                        ev.stale.len()
                    ));
                    calls.push(Call::new("remove_own_stale_containers"));
                }
                // 先把自己这一套拆干净，再重算端口 —— 顺序反了的话旧容器会占着新端口
                if compose::is_up() || !ev.stale.is_empty() {
                    calls.push(Call::new("compose_down_own"));
                }
                calls.push(Call::new("remap_ports"));
                why.push_str("为这次安装换一组空闲端口，两套并存、互不影响。");
                Some((calls, why))
            }
            // ② docker 找不到。**两步，顺序不能反**（待办池 P1-23）：
            //    先按内置清单硬找一遍 —— 找得到就直接写进设置，零 token；
            //    真的一个都没有，才谈「装一套」。
            //
            //    I5 场景 1 实测：这一步交给模型花了 8,632 token，而且它第一轮
            //    先要了一次 `probe_docker_path`（等于把我们刚做过的事再做一遍）。
            Code::DockerMissing => {
                if let Some(path) = self.hard_find_docker() {
                    return Some((
                        vec![Call::with("set_docker_path", "path", &path)],
                        format!(
                            "docker 其实装着，只是不在这个程序能看到的 PATH 里。按已知位置探到了 {}，写进设置。",
                            crate::redact::mask_home(&path)
                        ),
                    ));
                }
                // 这台机器上真的没有 docker。看看该装还是该点亮已有的
                match crate::runtime::builtin::decide() {
                    crate::runtime::builtin::Decision::AlreadyRunning(_) => None,
                    crate::runtime::builtin::Decision::StartExisting(app) => {
                        actions::plan(&Call::with("start_runtime", "app", &app)).ok()?;
                        Some((
                            vec![
                                Call::with("start_runtime", "app", &app),
                                Call::with("wait_daemon", "seconds", "90"),
                            ],
                            format!(
                                "{} 已经装在这台机器上了，只是没启动 —— 把它点起来就行，不用再装一套。",
                                actions::runtime_label(&app)
                            ),
                        ))
                    }
                    crate::runtime::builtin::Decision::StartBuiltin => Some((
                        vec![Call::new("start_builtin_runtime")],
                        "上次装好的内置运行时还在，把它的虚拟机起起来就行。".to_string(),
                    )),
                    crate::runtime::builtin::Decision::Install => {
                        let items = crate::runtime::manifest::for_host();
                        Some((
                            vec![Call::new("install_runtime")],
                            format!(
                                "这台机器上一个 Docker 都没有（内置清单里 {} 个位置全找过了）。                                 装一套完全放在 ~/.hunter/runtime 里的运行时：{} 个组件、合计 {}，                                 不要管理员密码、不改系统任何地方、设置里可一键卸载。",
                                crate::runtime::which::builtin_dirs().len(),
                                items.len(),
                                crate::assist::probe::human_bytes(
                                    crate::runtime::manifest::total_bytes(&items)
                                )
                            ),
                        ))
                    }
                    crate::runtime::builtin::Decision::Unsupported(_) => None,
                }
            }
            // ②′ **内置运行时装了但虚拟机没起来**（I9 的 P0-1）。
            //
            //    这一条是 0.1.8 在用户 Mac 上没走到的那条主线。三种情形，
            //    三条不同的路，全都零 token：
            //
            //    | 现场 | 怎么办 |
            //    |---|---|
            //    | 文件缺了几个（0.1.7 装过一半） | `install_runtime` —— 只下缺的那几个，下完接着起 |
            //    | 文件齐了、虚拟机没起来 | `start_builtin_runtime` |
            //    | 起过一次没起来（这一回合已经试过了） | `repair_builtin_runtime` —— 清掉 profile 重建 |
            Code::BuiltinRuntimeDown => {
                use crate::runtime::effective::BuiltinState;
                let st = crate::runtime::effective::builtin_state();
                // 这一次安装里已经起过一次没成 —— 别再原样试第二遍，直接清残骸重建
                if self.failed_actions.contains("start_builtin_runtime")
                    || self.failed_actions.contains("install_runtime")
                {
                    return Some((
                        vec![
                            Call::new("repair_builtin_runtime"),
                            Call::with("wait_daemon", "seconds", "120"),
                        ],
                        "刚才起那台虚拟机没成。上次装到一半很可能留下了残骸 —— \
                         把 Hunter 自己的这台虚拟机清掉重建一次（只动 ~/.hunter/runtime，\
                         你的 ~/.colima、别的容器和数据卷一个字节都不碰）。"
                            .to_string(),
                    ));
                }
                match st {
                    BuiltinState::Partial(miss) => Some((
                        vec![Call::new("install_runtime")],
                        format!(
                            "上一次安装没装完，Hunter 自己那套运行时还缺 {}。把缺的补齐再起虚拟机 —— \
                             已经下好并校验过的文件一个字节都不会重下。",
                            miss.join("、")
                        ),
                    )),
                    BuiltinState::Stopped => Some((
                        vec![
                            Call::new("start_builtin_runtime"),
                            Call::with("wait_daemon", "seconds", "120"),
                        ],
                        format!(
                            "Hunter 自己那套运行时上次已经装好了，只是虚拟机没起来 —— \
                             用本机那份已经校验过的系统镜像把 profile {} 起起来就行，不用下东西。",
                            crate::runtime::builtin::PROFILE
                        ),
                    )),
                    // 走到这里说明状态在这两次判定之间变了（虚拟机自己起来了 /
                    // 目录被删了）。**不硬猜**，让下一圈重新判
                    BuiltinState::Running | BuiltinState::Absent => None,
                }
            }
            // ②″ **虚拟机没有 DNS**（I10 的 P0-1）。零 token，判据是侦察员
            //     在那台虚拟机里真的解析过一次域名。
            Code::RuntimeNoDns => {
                // 修过一次还是这个码 —— 别再原样试第二遍（`rule_plan` 那层也会滤，
                // 这里写出来是为了让「为什么不再提」这句话说得清）
                if self.failed_actions.contains("fix_vm_dns") {
                    return None;
                }
                Some((
                    vec![Call::new("fix_vm_dns")],
                    format!(
                        "Hunter 自己那台虚拟机解析不了任何域名 —— 它自带的这份 Ubuntu 镜像里\
                         没有 systemd-resolved，/etc/resolv.conf 出厂就是一条断链。\
                         在**这台虚拟机**里把它写成普通文件（{}），你电脑的网络设置一个字节都不动。",
                        crate::runtime::vmdns::NAMESERVERS
                            .iter()
                            .map(|(ns, _)| *ns)
                            .collect::<Vec<_>>()
                            .join(" / ")
                    ),
                ))
            }
            // ②‴ **容器连不上模型网关**（I10 的 P0-2）。
            //     内置运行时上有的修；用户自己的 Docker 上**没得修** ——
            //     改他这台电脑的网络设置是禁止项，这时规则层让路，如实交给诊断员。
            Code::ContainerOffline => {
                // **只有「虚拟机自己就解析不动」这一种有确定的修法。**
                // 虚拟机好好的、容器却连不上（代理只放行了宿主机、公司网络挡了 443）
                // 那一类我们没得修 —— 规则层让路，绝不给一个看着像能修其实没用的动作。
                let vm_broken = ev
                    .report
                    .vm_dns
                    .as_ref()
                    .is_some_and(|d| d.resolves == Some(false));
                if !vm_broken || self.failed_actions.contains("fix_vm_dns") {
                    return None;
                }
                Some((
                    vec![Call::new("fix_vm_dns")],
                    "容器连不上 Hunter 的模型网关，而 Hunter 自己那台虚拟机本身就解析不了域名 —— \
                     先把虚拟机的 /etc/resolv.conf 写好，容器才拿得到 DNS。"
                        .to_string(),
                ))
            }
            // ③ 装了没起：把它点起来再等
            Code::DaemonDown => {
                let app = ev
                    .report
                    .apps
                    .iter()
                    // **`builtin` 不在这条路上**（I9）：它不是「用户装的运行时」，
                    // 起它要带 COLIMA_HOME 与 profile，走的是上面 ②′ 那一条
                    .filter(|a| a.id != "builtin")
                    .find(|a| a.installed == Some(true) && a.running == Some(false))?;
                // 能不能真的启动（这个平台上有没有办法）现在就判，判不了就不提这个方案
                actions::plan(&Call::with("start_runtime", "app", &app.id)).ok()?;
                Some((
                    vec![
                        Call::with("start_runtime", "app", &app.id),
                        Call::with("wait_daemon", "seconds", "90"),
                    ],
                    format!("{} 装着但没在运行，把它启动起来再等它就绪。", app.label),
                ))
            }
            // ④′ 本机凭据助手缺失（I6 的 P0）。**这一条必须排在「拉不动」前面** ——
            //     0.1.5 就是把它当成「源不通」，来回换了三次源，一个问题都没解决。
            Code::CredHelper => {
                let missing: Vec<String> = ev
                    .cred_helpers
                    .iter()
                    .filter(|(_, found)| found.is_none())
                    .map(|(n, _)| n.clone())
                    .collect();
                if missing.is_empty() {
                    // 补全之后已经找得到了 —— 那就只是重试一次的事
                    // （第一次失败发生在补全生效之前，或者用户刚把它装上）
                    return Some((
                        vec![Call::with("retry_pull", "delay_seconds", "0")],
                        format!(
                            "凭据助手其实在这台机器上，只是原来那份 PATH 里看不到它。{}，用补全后的 PATH 重来一次。",
                            ev.sub_env
                        ),
                    ));
                }
                // 补全之后还是找不到：**不碰用户的 ~/.docker/config.json**，
                // 另起一份不带 credsStore 的最小配置给我们自己用。公开镜像不需要凭据。
                Some((
                    vec![
                        Call::new("use_isolated_docker_config"),
                        Call::with("retry_pull", "delay_seconds", "0"),
                    ],
                    format!(
                        "你的 docker 配置要用 {} 去取登录信息，但这台机器上哪儿都找不到它（{}）。\
                         Hunter 的六个镜像都是公开的，本来就不需要登录 —— \
                         启动器给自己另起一份不带凭据助手的配置，你的 ~/.docker/config.json 一个字节都不动。",
                        missing.join("、"),
                        ev.sub_env
                    ),
                ))
            }
            // ④ 拉不动：换一个测得通的源再重试
            Code::PullFailed | Code::ComposeFetch => {
                // 换过一次源、原话还是同一条 → 与源无关，规则层让路给诊断员（I6）
                if self.switched_registry.contains(&error_fingerprint(&e.msg)) {
                    return None;
                }
                let cfg = LauncherConfig::load();
                let alt = crate::registry::CANDIDATES.iter().find(|c| {
                    c.prefix != cfg.hunter.registry_prefix
                        && crate::registry::probe(c, &cfg.hunter.tag, Duration::from_secs(8))
                            .available
                })?;
                let mut calls = vec![Call::with("switch_registry", "registry", alt.id.as_ref())];
                // compose 文件还没落地时（失败发生在 prepare 那一步）`docker compose pull`
                // 根本无从谈起，提了也只会得到「找不到 docker-compose.yml」
                if crate::paths::compose_file().exists() {
                    calls.push(Call::with("retry_pull", "delay_seconds", "3"));
                }
                Some((
                    calls,
                    format!(
                        "当前这个下载源拉不动，刚测过「{}」是通的，换过去重来。",
                        alt.label
                    ),
                ))
            }
            // ⑤ 起不来但不是端口的事。**先问一句「是不是根本没有 DNS」**（I10）——
            //    0.1.9 在用户 Mac 上就是这一档：opencode 的健康检查要外网，
            //    虚拟机没有 DNS，于是等满 180 秒报「启动超时」。
            //    那一次规则层在这里 `None` 了，整件事掉到模型那边、最后 unknown。
            Code::StartTimeout if step == Step::Start => {
                let vm_broken = ev
                    .report
                    .vm_dns
                    .as_ref()
                    .is_some_and(|d| d.resolves == Some(false));
                let container_no_dns = ev
                    .container_net
                    .as_ref()
                    .is_some_and(|c| c.fail == crate::runtime::netcheck::Fail::NoDns);
                if (vm_broken || container_no_dns) && !self.failed_actions.contains("fix_vm_dns") {
                    let mut calls = vec![Call::new("fix_vm_dns")];
                    if compose::is_up() {
                        calls.push(Call::new("restart_stack"));
                    }
                    return Some((
                        calls,
                        format!(
                            "服务没就绪不是因为慢，是因为**解析不了域名**：{}。\
                             健康检查要连外网、容器也要连 Hunter 的模型网关，没有 DNS 两件事都做不成。\
                             先把 Hunter 自己那台虚拟机的 /etc/resolv.conf 写好，再让容器重新起一次。",
                            ev.container_net
                                .as_ref()
                                .map(|c| c.one_line())
                                .or_else(|| ev.report.vm_dns.as_ref().map(|d| d.one_line()))
                                .unwrap_or_else(|| "虚拟机里解析不了 hunter.agentpit.io".into())
                        ),
                    ));
                }
                if !ev.stale.is_empty() {
                    return Some((
                        vec![
                            Call::new("remove_own_stale_containers"),
                            Call::new("restart_stack"),
                        ],
                        format!(
                            "上一次起到一半留下了 {} 个没启动的容器，先清掉再重新起。",
                            ev.stale.len()
                        ),
                    ));
                }
                // **虚拟机好好的，容器手里那份 DNS 却是旧的**（I10 的 P0-3）。
                //
                // 容器里那份 `/etc/resolv.conf` 是它**创建那一刻**由 dockerd 写死的，
                // 之后不会自己变。所以「DNS 是上一次装到一半时修好的、容器是在那之前起的」
                // 这个现场是真实存在的 —— 用户 Mac 上 0.1.9 结束时就是这个样子。
                // 重建一次（`down` 不带 -v + `up -d`，数据卷一个都不动）就好。
                if !ev.stale_dns.is_empty() && !self.failed_actions.contains("compose_down_own") {
                    return Some((
                        vec![Call::new("compose_down_own"), Call::new("restart_stack")],
                        format!(
                            "{} 这几个容器手里那份 /etc/resolv.conf 还是修好 DNS 之前的 —— \
                             那份文件是容器创建时写死的，不会自己更新。\
                             把它们重建一次（数据卷一个都不动）就能拿到新的 DNS。",
                            ev.stale_dns.join("、")
                        ),
                    ));
                }
                None
            }
            _ => None,
        }
    }

    // ── 复核员（第二个模型角色，I7） ─────────────────────────────────────

    /// 计划里含 `Sensitive` 动作时走这一趟。事件流里单独一张「复核」卡片。
    ///
    /// 复核这件事本身要花 token，所以也吃同一份预算；预算到头 / 没 key /
    /// AI 关着的时候**不是默认放行**，而是「复核做不成」—— 那一条随后会落到
    /// 「需要你」卡片上，由用户自己拍板。
    fn review(
        &mut self,
        parent: u64,
        calls: &[Call],
        why: &str,
        ev: &Evidence,
    ) -> super::reviewer::Verdict {
        let v = if self.mode == Mode::Off {
            super::reviewer::Verdict::unavailable("AI 那一层在设置里关着")
        } else if self.tokens >= MAX_TOKENS {
            super::reviewer::Verdict::unavailable(&format!(
                "这次安装已经用掉 {} token，到上限了",
                self.tokens
            ))
        } else {
            match self.key.clone() {
                Some(k) => super::reviewer::review(calls, why, &ev.to_prompt(), &k),
                None => super::reviewer::Verdict::unavailable("还没有可用的 hunter key"),
            }
        };
        self.tokens += v.tokens;
        super::reviewer::emit_card(&self.bus, parent, &v);
        self.tick(&self.phase.clone());
        v
    }

    // ── 诊断员 · 第二层（模型兜底） ──────────────────────────────────────

    /// 规则认不出来时才问模型。**模型只能从动作表里挑**，挑表外的一律被
    /// [`actions::plan`] 拒掉并记审计。
    fn ask_model(
        &mut self,
        parent: u64,
        step: Step,
        e: &AppError,
        ev: &Evidence,
    ) -> Option<(Vec<Call>, String)> {
        if self.mode == Mode::Off {
            self.degrade(parent, "AI 那一层在设置里关着，只用确定性规则。");
            return None;
        }
        let Some(key) = self.key.clone() else {
            self.degrade(parent, "还没有可用的 hunter key，问不了模型。");
            return None;
        };
        if self.tokens >= MAX_TOKENS {
            self.degrade(
                parent,
                &format!("这次安装已经用掉 {} token，到上限了。", self.tokens),
            );
            return None;
        }

        let t = Instant::now();
        let mev = self
            .bus
            .emit(EventDraft::new(Kind::Analyze, "问一下模型").under(parent));
        let messages = vec![
            serde_json::json!({"role": "system", "content": AUTO_SYSTEM_PROMPT}),
            serde_json::json!({"role": "user", "content": format!(
                "安装走到「{}」这一步失败了。\n错误码：{}\n原话：{}\n\n{}\n\n\
                 请只做一件事：从工具表里挑出**最可能解决它**的动作（可以挑多个，按执行顺序），\
                 并用一句话说清原因。原因必须基于上面给出的证据，证据里没有的事实不要写。",
                step.title(), e.code.as_str(), crate::redact::redact(&e.msg), ev.to_prompt()
            )}),
        ];
        let resp = match super::ai::call_gateway(&messages, &key) {
            Ok(r) => r,
            Err(d) => {
                self.bus.finish(
                    mev,
                    Status::Warn,
                    d.message(),
                    Some(t.elapsed().as_millis() as u64),
                );
                return None;
            }
        };
        let used = resp
            .get("usage")
            .and_then(|u| u.get("total_tokens"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        self.tokens += used;

        let msg = resp
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|a| a.first())
            .and_then(|c| c.get("message"))
            .cloned()
            .unwrap_or(serde_json::json!({}));
        let text = msg
            .get("content")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .trim()
            .to_string();

        let mut calls = Vec::new();
        let mut rejected = Vec::new();
        for tc in msg
            .get("tool_calls")
            .and_then(|t| t.as_array())
            .cloned()
            .unwrap_or_default()
        {
            let name = tc
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string();
            let args_raw = tc
                .get("function")
                .and_then(|f| f.get("arguments"))
                .and_then(|a| a.as_str())
                .unwrap_or("{}");
            let args: std::collections::BTreeMap<String, String> =
                serde_json::from_str::<serde_json::Value>(args_raw)
                    .ok()
                    .and_then(|v| v.as_object().cloned())
                    .map(|o| {
                        o.into_iter()
                            .map(|(k, v)| {
                                (
                                    k,
                                    match v {
                                        serde_json::Value::String(s) => s,
                                        other => other.to_string(),
                                    },
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
            let call = Call {
                id: name.clone(),
                args,
            };
            // **白名单那道闸**：表外的、参数不合法的一律拒绝并写审计
            match actions::plan(&call) {
                Ok(_) => calls.push(call),
                Err(err) => {
                    guard::audit(
                        &call.id,
                        &call.args,
                        Proposer::Model,
                        None,
                        &format!("拒绝：{}", err.msg),
                    );
                    rejected.push(format!("{}：{}", short(&name), err.msg));
                }
            }
        }

        let detail = if text.is_empty() {
            format!("模型挑了 {} 个动作", calls.len())
        } else {
            first_line(&crate::redact::mask_home(&crate::redact::redact(&text)))
        };
        let mut done = EventDraft::new(Kind::Analyze, "模型的判断")
            .under(parent)
            .status(Status::Ok)
            .detail(detail.clone())
            .tokens(used)
            .elapsed(t.elapsed().as_millis() as u64);
        for r in &rejected {
            done = done.tech(format!("被守卫拒绝：{r}"));
        }
        self.bus.finish(
            mev,
            Status::Ok,
            &detail,
            Some(t.elapsed().as_millis() as u64),
        );
        self.bus.emit(done);
        // 被拒的也要**显示出来**：用户有权知道模型提了什么、为什么没执行
        for r in &rejected {
            self.bus.emit(
                EventDraft::new(Kind::Action, "模型提的这个动作被拦下了")
                    .under(parent)
                    .status(Status::Warn)
                    .detail(r.clone()),
            );
        }
        self.tick(&self.phase.clone());
        if calls.is_empty() {
            return None;
        }
        Some((calls, if text.is_empty() { detail } else { text }))
    }

    fn degrade(&self, parent: u64, why: &str) {
        self.bus.emit(
            EventDraft::new(Kind::Analyze, "这一步没有问模型")
                .under(parent)
                .status(Status::Skipped)
                .detail(why),
        );
    }

    // ── 需要你 ────────────────────────────────────────────────────────────

    /// 「这一步我确实做不了」——发一张**如实说明**的卡片。
    ///
    /// I8 之前这里是另一张卡片：把要敲的命令原样给出来 + 一个「我执行完了，继续」
    /// 按钮。用户 2026-09-21 22:10 把那种做法否了：
    ///
    /// > 不要让用户拷贝命令到命令行执行，因为很多用户连这个都不明白。
    ///
    /// 所以现在只有两种结局：**要么启动器自己做完**（要管理员权限就弹系统密码框，
    /// 见 [`crate::runtime::elevate`]），**要么如实说做不了**并给出诊断包那条路。
    /// 中间那种「把活儿丢回给用户」的卡片不再存在。
    fn say_cannot(&self, parent: u64, what: &str, why: &str) {
        self.bus.emit(
            EventDraft::new(Kind::Failed, what)
                .under(parent)
                .status(Status::Failed)
                .detail(why)
                .tech("这一步启动器自己做不了，也不会把它变成一条让你去敲的命令。"),
        );
        guard::audit(
            "cannot_do",
            &std::collections::BTreeMap::from([("what".to_string(), what.to_string())]),
            Proposer::Orchestrator,
            None,
            why,
        );
    }

    /// 挂一个提问并阻塞等答案。**不设超时** —— 悄悄超时然后自己决定，
    /// 正好是这一轮要修掉的那种行为。
    fn wait_answer(&self, question: &str, choices: &[Choice]) -> bool {
        if let Some(r) = &self.answer_reader {
            return r(question, choices).as_deref() == Some("yes");
        }
        if !self.interactive {
            return false;
        }
        let ask = Arc::new(Ask {
            answer: Mutex::new(None),
            cv: Condvar::new(),
        });
        if let Ok(mut g) = self.ask.lock() {
            *g = Some(ask.clone());
        }
        let mut answer = None;
        if let Ok(g) = ask.answer.lock() {
            let mut g = g;
            while g.is_none() && !self.cancelled() {
                let (ng, _) = ask
                    .cv
                    .wait_timeout(g, Duration::from_millis(500))
                    .unwrap_or_else(|e| e.into_inner());
                g = ng;
            }
            answer = g.clone();
        }
        if let Ok(mut g) = self.ask.lock() {
            *g = None;
        }
        answer.as_deref() == Some("yes")
    }

    /// 这个 `Sensitive` 动作用户在授权页上提前批过吗（I7）。
    ///
    /// **只有 `install_runtime` 这一个**。`reuse_existing_hunter` 动的是用户
    /// 自己装的那一套，不在这个口子里 —— 那种事没有「一次授权、以后不问」的道理。
    fn pre_authorized(&self, call: &Call) -> bool {
        if call.id != "install_runtime" {
            return false;
        }
        LauncherConfig::load().assist.allow_install_runtime
    }

    /// `install_runtime` 的带事件版本：下载进度、校验、起虚拟机，全是**真实值**。
    ///
    /// 走的是同一份 [`crate::runtime::builtin`]；不同的只是把进度喂给事件总线
    /// 而不是日志。执行前仍然过 [`actions::plan`] 与守卫（下面那句 `execute_as`
    /// 走完整通路），这里只负责「一边装一边说」。
    fn run_install_runtime(&mut self, parent: u64) -> AppResult<String> {
        // **三道门一道都不能少。** 这一条走的是自己的执行体（要实时进度），
        // 所以动作表校验与审计在这里补上 —— 不能因为「换了个执行入口」
        // 就绕过 actions::execute_as 里那一套
        let call = Call::new("install_runtime");
        let plan = match actions::plan(&call) {
            Ok(p) => p,
            Err(e) => {
                guard::audit(
                    &call.id,
                    &call.args,
                    Proposer::Orchestrator,
                    None,
                    &format!("拒绝：{}", e.msg),
                );
                return Err(e);
            }
        };
        guard::audit(
            &call.id,
            &call.args,
            Proposer::Orchestrator,
            Some(plan.level),
            "开始执行（已过动作表与授权判定）",
        );
        let finish = |r: &AppResult<String>| {
            guard::audit(
                "install_runtime",
                &std::collections::BTreeMap::new(),
                Proposer::Orchestrator,
                Some(plan.level),
                &match r {
                    Ok(t) => format!("成功：{}", first_line(t)),
                    Err(e) => format!("失败：{}", e.msg),
                },
            );
        };
        let r = self.run_install_runtime_inner(parent);
        finish(&r);
        r
    }

    fn run_install_runtime_inner(&mut self, parent: u64) -> AppResult<String> {
        // 整条兜底链在 [`crate::runtime::chain`] 里（内置运行时 → OrbStack 官方包 →
        // Homebrew）。这里只负责把它吐出来的每一句话变成事件流里的一张卡片，
        // 以及把下载进度做成**真实字节数**的那一行。
        let bus = self.bus.clone();
        let head = bus.emit(
            EventDraft::new(Kind::Action, "正在替你把 Docker 装好")
                .under(parent)
                .status(Status::Running),
        );
        let t0 = Instant::now();
        let b2 = bus.clone();
        let mut say = |line: &str| {
            b2.emit(
                EventDraft::new(Kind::Action, line)
                    .under(head)
                    .status(Status::Ok),
            );
        };
        let b3 = bus.clone();
        let mut last = 0u64;
        let mut bytes = move |got: u64, total: u64| {
            // 每 4 MB 更新一行，不要一秒刷几十条
            if got < last + 4 * 1024 * 1024 && got < total {
                return;
            }
            last = got;
            let pct = got
                .checked_mul(100)
                .and_then(|x| x.checked_div(total))
                .unwrap_or(0);
            b3.finish(
                head,
                Status::Running,
                &format!(
                    "{pct}% · 已下载 {} / {}",
                    crate::assist::probe::human_bytes(got),
                    crate::assist::probe::human_bytes(total)
                ),
                None,
            );
        };
        let c = self.cancel.clone();
        let cancel = move || c.load(Ordering::Relaxed);
        let mut pr = crate::runtime::builtin::Progress {
            say: &mut say,
            bytes: &mut bytes,
            cancel: &cancel,
        };
        let r = crate::runtime::chain::install_docker(&mut pr);
        match &r {
            Ok(out) => {
                // 换过路线也是一次「自动解决的问题」——**如实计数**（I8 一.4）
                for (route, why) in &out.failed {
                    // 换一条路线走通了，也是**自动解决掉一个问题**（I8 一.4）。
                    // 与「预检时换端口」那一条同一个计数器，不另立一个
                    self.solved += 1;
                    crate::linfo!("兜底链换过一次路线（{} 没走通）", route.label());
                    bus.emit(
                        EventDraft::new(
                            Kind::Resolved,
                            format!("「{}」这条路没走通，已自动改走下一条", route.label()),
                        )
                        .under(head)
                        .status(Status::Ok)
                        .detail("你不用做任何事，启动器自己换了一条路")
                        .tech(why.clone()),
                    );
                }
                self.tick(&self.phase.clone());
                bus.finish(
                    head,
                    Status::Ok,
                    &format!("{}（走的是「{}」）", out.message, out.route.label()),
                    Some(t0.elapsed().as_millis() as u64),
                );
            }
            Err(e) => bus.finish(head, Status::Failed, &e.msg, None),
        }
        Ok(r?.message)
    }

    /// 起 / 重建内置运行时的**带事件版本**（I9）。
    ///
    /// 为什么不走 [`actions::execute_as`] 那条通路：那一条给 `builtin` 的
    /// `Progress` 是三个空闭包，进度只进日志。起一台虚拟机要一分钟起步，
    /// 重建还可能顺带重下几百 MB —— 界面上只挂一行「正在处理」是不行的
    /// （`install_runtime` 早就是这么处理的，这里只是把同样的待遇给另外两条）。
    ///
    /// **三道门一道没少**：动作表校验 + 守卫 + 审计，与 `run_install_runtime`
    /// 完全一致，下面那段就是从它那儿照搬的。
    fn run_builtin_action(&mut self, parent: u64, call: &Call) -> AppResult<String> {
        let plan = match actions::plan(call) {
            Ok(p) => p,
            Err(e) => {
                guard::audit(
                    &call.id,
                    &call.args,
                    Proposer::Orchestrator,
                    None,
                    &format!("拒绝：{}", e.msg),
                );
                return Err(e);
            }
        };
        guard::audit(
            &call.id,
            &call.args,
            Proposer::Orchestrator,
            Some(plan.level),
            "开始执行（已过动作表与授权判定）",
        );
        let bus = self.bus.clone();
        let head = bus.emit(
            EventDraft::new(Kind::Action, plan.title.clone())
                .under(parent)
                .status(Status::Running),
        );
        let t0 = Instant::now();
        let b2 = bus.clone();
        let mut say = |line: &str| {
            if line.trim().is_empty() {
                return;
            }
            b2.emit(
                EventDraft::new(Kind::Action, line)
                    .under(head)
                    .status(Status::Ok),
            );
        };
        let b3 = bus.clone();
        let mut last = 0u64;
        let mut bytes = move |got: u64, total: u64| {
            if got < last + 4 * 1024 * 1024 && got < total {
                return;
            }
            last = got;
            let pct = got
                .checked_mul(100)
                .and_then(|x| x.checked_div(total))
                .unwrap_or(0);
            b3.finish(
                head,
                Status::Running,
                &format!(
                    "{pct}% · 已下载 {} / {}",
                    crate::assist::probe::human_bytes(got),
                    crate::assist::probe::human_bytes(total)
                ),
                None,
            );
        };
        let c = self.cancel.clone();
        let cancel = move || c.load(Ordering::Relaxed);
        let mut pr = crate::runtime::builtin::Progress {
            say: &mut say,
            bytes: &mut bytes,
            cancel: &cancel,
        };
        let r: AppResult<String> = (|| {
            if call.id == "repair_builtin_runtime" {
                let cleaned = crate::runtime::builtin::repair(&mut pr)?;
                if crate::runtime::builtin::needs_download() {
                    crate::runtime::builtin::install(&mut pr)?;
                }
                let started = crate::runtime::builtin::start(&mut pr)?;
                return Ok(format!("{cleaned}；{started}"));
            }
            crate::runtime::builtin::start(&mut pr)
        })();
        match &r {
            Ok(t) => bus.finish(
                head,
                Status::Ok,
                &first_line(t),
                Some(t0.elapsed().as_millis() as u64),
            ),
            Err(e) => bus.finish(head, Status::Failed, &e.msg, None),
        }
        guard::audit(
            &call.id,
            &call.args,
            Proposer::Orchestrator,
            Some(plan.level),
            &match &r {
                Ok(t) => format!("成功：{}", first_line(t)),
                Err(e) => format!("失败：{}", e.msg),
            },
        );
        r
    }

    /// 发一张「需要你」卡片并**阻塞等待**用户点。按钮文案是结论不是问句。
    fn ask_user(&self, parent: u64, spec: &actions::Spec, call: &Call) -> bool {
        let label_yes = format!("好，{}", spec.title);
        let ev = self.bus.emit(
            EventDraft::new(Kind::NeedUser, spec.title)
                .under(parent)
                .status(Status::Waiting)
                .detail(spec.why)
                .tech(format!("级别：{}", spec.level.cn()))
                .choices(vec![
                    Choice {
                        value: "yes".into(),
                        label: label_yes.clone(),
                        primary: true,
                    },
                    Choice {
                        value: "no".into(),
                        label: "不用，我自己来".into(),
                        primary: false,
                    },
                ]),
        );
        let choices = vec![
            Choice {
                value: "yes".into(),
                label: label_yes,
                primary: true,
            },
            Choice {
                value: "no".into(),
                label: "不用，我自己来".into(),
                primary: false,
            },
        ];
        guard::audit(
            &call.id,
            &call.args,
            Proposer::Orchestrator,
            Some(spec.level),
            "等用户确认",
        );
        let yes = self.wait_answer(&format!("{}（{}）", spec.title, spec.why), &choices);
        self.bus.finish(
            ev,
            if yes { Status::Ok } else { Status::Skipped },
            if yes {
                "你同意了"
            } else {
                "你选择自己来"
            },
            None,
        );
        guard::audit(
            &call.id,
            &call.args,
            Proposer::User,
            Some(spec.level),
            if yes { "用户同意" } else { "用户拒绝" },
        );
        yes
    }
}

// ── 侦察员采到的证据 ──────────────────────────────────────────────────────

pub struct Evidence {
    pub report: probe::Report,
    /// 本机上别的 Hunter 安装。
    ///
    /// **用的是 [`crate::takeover::Candidate`] 而不是 [`crate::ports::OtherInstall`]**（I7）：
    /// 后者只有项目名 / 容器 / 端口，缺了**工作目录**。而 `reuse_existing_hunter`
    /// 的计划里会写出工作目录 —— 证据里没有它，复核员就会（正确地）判定
    /// 「这个事实在证据里找不到依据，属于凭空编造」并否决。实测撞到过一次。
    pub others: Vec<crate::takeover::Candidate>,
    pub stale: Vec<compose::StaleContainer>,
    pub port_lines: Vec<String>,
    /// 本机 docker 凭据助手：`(助手名, 补全后的 PATH 上找到的绝对路径)`（I6）
    pub cred_helpers: Vec<(String, Option<String>)>,
    /// 子进程 PATH 补了哪些目录 —— 「找不到」这类错误里，这一行是最要紧的证据
    pub sub_env: String,
    /// **容器到底能不能连上模型网关**（I10）。
    ///
    /// 只在「起容器」这一类失败上才采 —— 它要起一个一次性容器，最多要几十秒，
    /// 不该每一回合都白花。`None` = 这一次没采（**不是「不通」**）。
    pub container_net: Option<crate::runtime::netcheck::Outcome>,
    /// **正跑着、但手里那份 DNS 已经过时**的服务（I10 的 P0-3）。
    /// 空 = 没有这个问题，或者这一次没查。
    pub stale_dns: Vec<String>,
    /// 总指挥想额外告诉诊断员的事（例如「换过源了，原话一个字没变」）
    pub notes: Vec<String>,
}

impl Evidence {
    /// 「分析中…」那张卡片右边的一句话。**都是真实读到的东西**。
    pub fn one_line(&self) -> String {
        let mut v = Vec::new();
        v.push(format!("查了 {} 个端口", self.port_lines.len()));
        if !self.others.is_empty() {
            v.push(format!("本机还有 {} 套 Hunter", self.others.len()));
        }
        if !self.stale.is_empty() {
            v.push(format!("本项目有 {} 个残留容器", self.stale.len()));
        }
        if let Some(d) = &self.report.vm_dns {
            v.push(match d.resolves {
                Some(true) => "虚拟机解析得动域名".to_string(),
                Some(false) => "虚拟机解析不了域名".to_string(),
                None => "虚拟机 DNS 没查成".to_string(),
            });
        }
        if !self.stale_dns.is_empty() {
            v.push(format!("{} 个容器的 DNS 已过时", self.stale_dns.len()));
        }
        if let Some(c) = &self.container_net {
            v.push(if c.ok {
                "容器连得上模型网关".to_string()
            } else {
                format!("容器连不上模型网关（{}）", c.fail.cn())
            });
        }
        v.join(" · ")
    }

    /// 送给模型的那一份。
    pub fn to_prompt(&self) -> String {
        let mut s = self.report.to_prompt();
        s.push_str(
            "\n端口现场（三重确认：不带 SO_REUSEADDR 的通配绑定 + docker ps + 系统监听表）：\n",
        );
        for l in &self.port_lines {
            s.push_str("  ");
            s.push_str(l);
            s.push('\n');
        }
        if self.others.is_empty() {
            s.push_str("这台机器上没有别的 Hunter 安装。\n");
        } else {
            // 这句话的分寸是**实测调出来的**（I7）：原来写的是「**不许动它们**」，
            // 结果复核员把它当成了铁律 —— 连「用户亲手点了「直接用它」之后
            // 只读地接管它」这种计划也一并否决，理由是「证据里写着不许动」。
            // 那条铁律本来是给**诊断员**的（别提议停掉别人的东西），
            // 而复核员读的是同一份证据。所以这里把「禁止什么」与
            // 「唯一的例外是什么」都写清楚，而不是留一句绝对化的话。
            s.push_str(
                "这台机器上还有别的 Hunter 安装。**不要提议停掉、删掉、改动它们**；\
                 默认处置永远是给新安装换一组空闲端口、两套并存。\
                 唯一的例外是 `reuse_existing_hunter`（改为只读地管理已有的那一套），\
                 而它必须由用户亲自点过同意才会发生：\n",
            );
            for o in &self.others {
                s.push_str(&format!(
                    "  compose 项目 {} · 容器 {} · 端口 {} · 工作目录 {} · compose 文件 {} · web 端口 {}\n",
                    o.project,
                    o.containers.join("、"),
                    o.ports
                        .iter()
                        .map(|p| p.to_string())
                        .collect::<Vec<_>>()
                        .join("、"),
                    if o.working_dir.is_empty() {
                        "读不到".to_string()
                    } else {
                        crate::redact::mask_home(&o.working_dir)
                    },
                    if o.config_files.is_empty() {
                        "读不到".to_string()
                    } else {
                        crate::redact::mask_home(&o.config_files)
                    },
                    if o.web_port > 0 {
                        o.web_port.to_string()
                    } else {
                        "读不到".into()
                    }
                ));
            }
        }
        if self.stale.is_empty() {
            s.push_str("本项目 hunter 没有「已创建未启动」的残留容器。\n");
        } else {
            s.push_str(&format!(
                "本项目 hunter 有 {} 个「已创建未启动」的残留容器：{}\n",
                self.stale.len(),
                self.stale
                    .iter()
                    .map(|c| c.name.as_str())
                    .collect::<Vec<_>>()
                    .join("、")
            ));
        }
        // ── 网络代理（I8）──
        //
        // 「直连不通」这类失败里，这一行常常就是答案。**只说事实**：
        // 配没配、有没有靠它救过场；不提议去改它（改用户的网络设置是禁止项）
        let px = crate::netproxy::current();
        if px.any() {
            s.push_str(&format!(
                "这台机器配了网络代理（{}），启动器在直连失败时会自动沿用它（只读，不修改）。\n",
                px.source_cn()
            ));
            let rescued = crate::netproxy::rescued_hosts();
            if !rescued.is_empty() {
                s.push_str(&format!(
                    "本次安装里这些站点是靠代理才通的：{}\n",
                    rescued.join("、")
                ));
            }
        } else {
            s.push_str("这台机器没有配系统代理（环境变量与系统设置里都没有）。\n");
        }
        // ── 本机凭据助手（I6）──
        s.push_str(&format!("{}\n", self.sub_env));
        if self.cred_helpers.is_empty() {
            s.push_str(
                "这台机器的 docker 配置里没有 credsStore / credHelpers，拉公开镜像不需要凭据。\n",
            );
        } else {
            for (name, found) in &self.cred_helpers {
                match found {
                    Some(p) => s.push_str(&format!(
                        "docker 凭据助手 {name}：找得到（{}）\n",
                        crate::redact::mask_home(p)
                    )),
                    None => s.push_str(&format!(
                        "docker 凭据助手 {name}：**补全 PATH 之后仍然找不到**\n"
                    )),
                }
            }
        }
        // ── 容器到底能不能上网（I10）──
        match &self.container_net {
            Some(c) if c.ok => s.push_str(&format!(
                "起过一个一次性容器实测：{}。所以「连不上网关」这条可以排除。\n",
                c.one_line()
            )),
            Some(c) => s.push_str(&format!(
                "起过一个一次性容器实测：{}。命令：{}\n\
                 注意这是**真的跑过一次**的结果，不是从日志里猜的。\n",
                c.one_line(),
                c.command
            )),
            None => s.push_str("这一次没有做「容器能不能连上网关」的实测。\n"),
        }
        if !self.stale_dns.is_empty() {
            s.push_str(&format!(
                "这几个正跑着的容器手里那份 /etc/resolv.conf 与现在虚拟机上的对不上：{}。\
                 容器里那份文件是它创建那一刻写死的，不会自己更新 —— \
                 把它们重建一次（`compose_down_own` + `restart_stack`，不带 -v）就能拿到新的。\n",
                self.stale_dns.join("、")
            ));
        }
        for n in &self.notes {
            s.push_str(&format!("总指挥补充：{n}\n"));
        }
        s
    }
}

/// 一条失败原话的「指纹」：把随现场变动的部分（镜像源、主机名、数字）抹掉之后的样子。
///
/// 用来回答一个问题：**换过源之后，失败的还是不是同一件事**。
/// 是同一件事 → 与源无关，再换一百次也没用（I6）。
pub fn error_fingerprint(msg: &str) -> String {
    let mut s = msg.to_lowercase();
    for c in crate::registry::CANDIDATES.iter() {
        s = s.replace(&c.prefix.to_lowercase(), "<registry>");
        s = s.replace(&c.host.to_lowercase(), "<host>");
        s = s.replace(&c.label.to_lowercase(), "<label>");
    }
    // 端口、耗时、字节数这些数字不该让两条本质相同的原话看起来不一样
    let s: String = s
        .chars()
        .map(|c| if c.is_ascii_digit() { '#' } else { c })
        .collect();
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub const AUTO_SYSTEM_PROMPT: &str = "\
你是 Hunter 启动器里的安装诊断员。启动器正在**全自动**地帮用户装 Hunter（docker compose 起的一套服务），某一步失败了。

你的职责只有一件：看证据，挑动作。规则：
1. 只用中文，不要客套。给出你认为**最可能**的那一个原因，不要罗列可能性。
2. 你不能写命令、不能让用户去敲命令。想做事只有一条路：调用给你的工具。工具表之外的一律会被启动器拒绝并记进审计日志。
3. 证据里写「读不到」的就是真的读不到 —— 不要当成 0，也不要假设一个值。**证据里没有的事实一个字都不要写**。
4. 端口被占时，占用者是谁已经写在证据里了（进程名 / 容器名 / compose 项目名）。不要猜。
5. 用户电脑上可能已经有别的 Hunter（别的 compose 项目）。**绝对不要提议停掉、删掉、改动它们** —— 正确做法永远是给新安装换一组空闲端口，两套并存。工具表里的 reuse_existing_hunter（改为只读地管理已有的那一套）**只有在用户自己明确要求过的时候**才考虑；你不要主动提它。
6. 不要提议删除用户的文件、删数据卷、清理镜像、改代理 / DNS / hosts / 防火墙、用 sudo。这些动作在工具表里根本不存在，提了也只会被拒绝、白花一轮。
7. 挑最少的动作。能一步解决的不要挑三步。

背景（实测过的事实）：
- macOS 的 GUI 程序 PATH 只有 /usr/bin:/bin:/usr/sbin:/sbin，不含 /usr/local/bin，所以「终端里能跑 docker」不等于「启动器找得到 docker」。
- 国内直连 ghcr.io 经常超时，腾讯云香港的源（id 是 tencent）一般能通。
- Hunter 要 5 个端口：web 3100、api 8100、opencode 3921、postgres 5442、redis 6479，被占时可以往上挪。
- 启动器会**只读地**沿用用户在系统里配好的网络代理（直连失败时自动走一次代理重试，子进程与虚拟机也会带上）。所以「直连超时」这件事已经自动兜过一层了；不要提议去改代理、DNS、hosts —— 那些动作不存在，提了也只会被拒绝。
- 这台电脑上没有 Docker 时，启动器会自动依次试三条路把它装好（内置运行时 → OrbStack 官方安装包 → Homebrew），**全部由启动器执行**。把这件事推给用户是不允许的，也没有这样的工具可调。
";

// ── 讲解员（模板，不调模型） ──────────────────────────────────────────────

/// 把错误码翻成一句用户看得懂的话。**不含技术名词，也不编内容**。
pub fn narrate_issue(e: &AppError) -> String {
    match e.code {
        Code::PortConflict | Code::PortInUse => "发现问题：要用的端口被占住了".into(),
        Code::DockerMissing => "发现问题：找不到 Docker".into(),
        Code::DaemonDown => "发现问题：Docker 装了但没在运行".into(),
        Code::PullFailed => "发现问题：组件下载失败".into(),
        Code::CredHelper => "发现问题：Docker 取不到登录信息".into(),
        Code::ComposeFetch => "发现问题：取不到 Hunter 的配置文件".into(),
        Code::StartTimeout => "发现问题：服务没能全部启动".into(),
        Code::ProjectConflict => "发现问题：另一个位置的 Hunter 正占着同一个项目名".into(),
        Code::ConfigWrite => "发现问题：配置写不进去".into(),
        Code::QuotaExhausted => "发现问题：今天的免费额度用完了".into(),
        Code::KeyInvalid => "发现问题：这把 key 网关不认".into(),
        _ => format!("发现问题：{}", e.code.title()),
    }
}

/// 问题卡片右边那句。数字全部来自 `AppError` 里的真实内容。
pub fn narrate_detail(e: &AppError) -> String {
    crate::redact::mask_home(&crate::redact::redact(&first_line(&e.msg)))
}

/// **启动器自己做不到**的那几件事：说清是什么、为什么。
///
/// I8 之前这个函数的名字叫 `manual_step`，返回的是「一句话 + 一条要用户去敲的命令」。
/// 现在它只返回「一句话 + 为什么做不了」——
/// 能替用户做的都已经在兜底链（[`crate::runtime::chain`]）与系统授权框
/// （[`crate::runtime::elevate`]）里做掉了，走到这里的都是**真的做不了**的。
fn cannot_do(e: &AppError) -> Option<(String, String)> {
    match e.code {
        Code::DaemonDown if cfg!(target_os = "linux") => Some((
            "Docker 后台服务没能启动".to_string(),
            format!(
                "启动它要管理员权限。启动器试过让系统弹授权框，但{}",
                crate::runtime::elevate::available()
                    .err()
                    .unwrap_or_else(|| "授权没有通过（你可能点了取消）。".to_string())
            ),
        )),
        Code::DockerMissing if !cfg!(target_os = "macos") => Some((
            "这台电脑上没有 Docker".to_string(),
            crate::runtime::chain::platform_cannot_install(),
        )),
        Code::DockerMissing => Some((
            "三条路都没能把 Docker 装起来".to_string(),
            "每一条失败的原话都在上面，也会一起打进诊断包。".to_string(),
        )),
        Code::CredHelper => Some((
            "你的 Docker 配置里指定了一个这台电脑上不存在的登录助手".to_string(),
            "启动器已经替你另起了一份不带登录助手的配置（你自己的配置一个字节没动）；\
             如果还是不行，把诊断包发给开发者是最快的一条路。"
                .to_string(),
        )),
        _ => None,
    }
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").chars().take(300).collect()
}

fn short(s: &str) -> String {
    s.chars().take(40).collect()
}

fn fmt_secs(n: u64) -> String {
    if n < 60 {
        format!("{n} 秒")
    } else {
        format!("{} 分 {} 秒", n / 60, n % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bus() -> Arc<Bus> {
        Arc::new(Bus::new(Box::new(crate::assist::events::Null), false))
    }

    fn orch(mode: Mode) -> Orchestrator {
        Orchestrator::new(bus(), mode, None, Arc::new(AtomicBool::new(false)))
    }

    fn ev_empty() -> Evidence {
        Evidence {
            report: probe::collect(None, None, None),
            others: Vec::new(),
            stale: Vec::new(),
            port_lines: Vec::new(),
            cred_helpers: Vec::new(),
            sub_env: "子进程 PATH：原样继承（没有需要补的目录）".into(),
            container_net: None,
            stale_dns: Vec::new(),
            notes: Vec::new(),
        }
    }

    #[test]
    fn 预算就是设计文档里那三个数() {
        assert_eq!(MAX_ROUNDS_PER_ISSUE, 4);
        assert_eq!(MAX_ROUNDS_TOTAL, 10);
        assert_eq!(MAX_TOKENS, 200_000);
        assert_eq!(super::super::ai::TIMEOUT, Duration::from_secs(45));
    }

    #[test]
    fn 单问题回合到头就停() {
        let o = orch(Mode::Auto);
        assert!(o.budget_stop(MAX_ROUNDS_PER_ISSUE).is_some());
        assert!(o.budget_stop(1).is_none());
    }

    #[test]
    fn token_到上限就不再问模型() {
        let mut o = orch(Mode::Auto);
        o.tokens = MAX_TOKENS;
        let s = o.budget_stop(1).expect("到上限要停");
        assert!(
            s.contains("200000") || s.contains(&MAX_TOKENS.to_string()),
            "{s}"
        );
    }

    /// 端口冲突的规则层：**一个 token 都不花**就给出「换端口」，
    /// 而且绝不会提议去动用户那一套。
    #[test]
    fn 端口冲突走规则层且只换自己的端口() {
        let _g = crate::paths::test_home("auto-ports");
        let o = orch(Mode::Auto);
        let mut ev = ev_empty();
        ev.others = vec![crate::takeover::Candidate {
            project: "hunter-fresh".into(),
            containers: vec!["hunter-fresh-api-1".into()],
            ports: vec![8100],
            working_dir: "/home/u/hunter-fresh".into(),
            config_files: "/home/u/hunter-fresh/docker-compose.yml".into(),
            web_port: 3100,
        }];
        let e = AppError::new(Code::PortConflict, "Bind for 0.0.0.0:8100 failed");
        let (calls, why) = o
            .rule_plan(Step::Start, &e, &ev)
            .expect("端口冲突必须能被规则层认出来");
        let ids: Vec<&str> = calls.iter().map(|c| c.id.as_str()).collect();
        assert!(ids.contains(&"remap_ports"), "{ids:?}");
        // 不许出现任何会动别人那一套的动作
        for id in &ids {
            assert!(
                !id.contains("volume") && !id.contains("prune") && *id != "brew_install",
                "规则层不该提 {id}"
            );
        }
        assert!(why.contains("hunter-fresh"), "原因里要指名占用者：{why}");
        assert!(why.contains("并存"), "{why}");
    }

    #[test]
    fn 有残留容器时先清再换端口() {
        let _g = crate::paths::test_home("auto-stale");
        let o = orch(Mode::Auto);
        let mut ev = ev_empty();
        ev.stale = vec![compose::StaleContainer {
            id: "abc".into(),
            name: "hunter-api-1".into(),
            state: "created".into(),
        }];
        let e = AppError::new(Code::PortConflict, "port is already allocated");
        let (calls, _) = o.rule_plan(Step::Start, &e, &ev).unwrap();
        let ids: Vec<&str> = calls.iter().map(|c| c.id.as_str()).collect();
        let i_rm = ids.iter().position(|x| *x == "remove_own_stale_containers");
        let i_remap = ids.iter().position(|x| *x == "remap_ports");
        assert!(i_rm.is_some(), "{ids:?}");
        assert!(i_rm < i_remap, "清残留要排在换端口前面：{ids:?}");
    }

    /// I6 的 P0：凭据助手找不到时，规则层给的是**补 PATH 重试 / 另起一份配置**，
    /// 绝不能是「换个镜像源」。
    #[test]
    fn 凭据助手缺失时不换源而是另起一份配置() {
        let o = orch(Mode::Auto);
        let e = AppError::new(
            Code::CredHelper,
            "Docker 要用本机的凭据助手去取登录信息……原话：error getting credentials - err: exec: \"docker-credential-osxkeychain\": executable file not found in $PATH".to_string(),
        );
        let mut ev = ev_empty();
        ev.cred_helpers = vec![("docker-credential-osxkeychain".to_string(), None)];
        let (calls, why) = o
            .rule_plan(Step::Pull, &e, &ev)
            .expect("规则层必须认得出来");
        let ids: Vec<&str> = calls.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["use_isolated_docker_config", "retry_pull"]);
        assert!(
            !ids.contains(&"switch_registry"),
            "这是本机配置的问题，换源永远修不好"
        );
        assert!(why.contains("公开"), "要说清「公开镜像不需要登录」：{why}");
        assert!(
            why.contains("一个字节都不动"),
            "要承诺不动用户的文件：{why}"
        );
    }

    /// 补全 PATH 之后已经找得到了：那就只是重试一次的事，不必另起配置。
    #[test]
    fn 凭据助手补全后找得到就只重试() {
        let o = orch(Mode::Auto);
        let e = AppError::new(
            Code::CredHelper,
            "原话：error getting credentials".to_string(),
        );
        let mut ev = ev_empty();
        ev.cred_helpers = vec![(
            "docker-credential-osxkeychain".to_string(),
            Some("/usr/local/bin/docker-credential-osxkeychain".to_string()),
        )];
        let (calls, _) = o.rule_plan(Step::Pull, &e, &ev).unwrap();
        let ids: Vec<&str> = calls.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["retry_pull"]);
    }

    /// 同一条原话只换一次源。换过之后原话没变 → 规则层让路（交给诊断员）。
    #[test]
    fn 同一条原话最多换一次源() {
        let mut o = orch(Mode::Auto);
        let e = AppError::new(
            Code::PullFailed,
            "docker compose pull 退出码 1。原话：something odd".to_string(),
        );
        let ev = ev_empty();
        // 第一次：规则层可以提换源（能不能提得出来取决于另一个源当时通不通，
        // 所以这里只验「记过一次之后就不再提」这半边，不依赖网络）
        o.switched_registry.insert(error_fingerprint(&e.msg));
        assert!(
            o.rule_plan(Step::Pull, &e, &ev).is_none(),
            "换过一次之后规则层必须让路，否则就会来回兜圈"
        );
    }

    /// 指纹要抹掉镜像源与数字 —— 「换了个源、原话本质没变」得认得出来。
    #[test]
    fn 指纹抹掉镜像源与数字() {
        let a = error_fingerprint("从 ghcr.io/agentpit-io 拉 3 个镜像失败：i/o timeout");
        let b =
            error_fingerprint("从 hkccr.ccs.tencentyun.com/agentpit 拉 6 个镜像失败：i/o timeout");
        assert_eq!(a, b, "换了源、换了数量，本质是同一条");
        let c = error_fingerprint("磁盘满了");
        assert_ne!(a, c);
    }

    /// 送给模型的证据里要有「凭据助手找不到」与「PATH 补了什么」这两件事 ——
    /// 0.1.5 的模型什么都没被告知，只好去猜「源不通」。
    #[test]
    fn 证据里带着凭据助手与子进程_path() {
        let mut ev = ev_empty();
        ev.cred_helpers = vec![("docker-credential-osxkeychain".to_string(), None)];
        ev.sub_env = "子进程 PATH 补了 1 个目录：/usr/local/bin".into();
        ev.notes.push("上一回合已经换过下载源了".into());
        let p = ev.to_prompt();
        assert!(p.contains("docker-credential-osxkeychain"), "{p}");
        assert!(p.contains("仍然找不到"), "{p}");
        assert!(p.contains("/usr/local/bin"), "{p}");
        assert!(p.contains("换过下载源"), "{p}");
    }

    #[test]
    fn 规则层给出的动作全都在动作表里() {
        let o = orch(Mode::Auto);
        let ev = ev_empty();
        for code in [
            Code::PortConflict,
            Code::PortInUse,
            Code::DockerMissing,
            Code::DaemonDown,
            Code::PullFailed,
            Code::StartTimeout,
            Code::RuntimeNoDns,
            Code::ContainerOffline,
        ] {
            let e = AppError::new(code, "x");
            if let Some((calls, _)) = o.rule_plan(Step::Start, &e, &ev) {
                for c in &calls {
                    assert!(
                        actions::spec(&c.id).is_some(),
                        "{code:?} 给了表外动作 {}",
                        c.id
                    );
                }
            }
        }
    }

    #[test]
    fn 讲解员说的是人话不是错误码() {
        let e = AppError::new(Code::PortConflict, "Bind for 0.0.0.0:8100 failed");
        let t = narrate_issue(&e);
        assert!(!t.contains("E_PORT"), "{t}");
        assert!(t.contains("端口"), "{t}");
    }

    #[test]
    fn 讲解员的细节里不会漏_key() {
        let e = AppError::new(
            Code::Unknown,
            "请求 hunt_tools_abcdefghijklmnopqrstuvwxyz012345 失败",
        );
        let d = narrate_detail(&e);
        assert!(!d.contains("abcdefghij"), "{d}");
    }

    #[test]
    fn off_档不问模型() {
        let mut o = orch(Mode::Off);
        let ev = ev_empty();
        let e = AppError::new(Code::Unknown, "谁也认不出来的错");
        assert!(o.ask_model(0, Step::Start, &e, &ev).is_none());
    }

    #[test]
    fn 没有_key_时不问模型() {
        let mut o = orch(Mode::Auto);
        let ev = ev_empty();
        let e = AppError::new(Code::Unknown, "谁也认不出来的错");
        assert!(o.ask_model(0, Step::Start, &e, &ev).is_none());
    }

    #[test]
    fn 证据里带着占用者与其他安装() {
        let _g = crate::paths::test_home("auto-evidence");
        let mut ev = ev_empty();
        ev.port_lines = vec!["api 8100 被占用 · Docker 容器 hunter-fresh-api-1（compose 项目 hunter-fresh · 0.0.0.0:8100->8000/tcp）".into()];
        ev.others = vec![crate::takeover::Candidate {
            project: "hunter-fresh".into(),
            containers: vec!["hunter-fresh-api-1".into()],
            ports: vec![8100],
            working_dir: "/home/u/hunter-fresh".into(),
            config_files: "/home/u/hunter-fresh/docker-compose.yml".into(),
            web_port: 3100,
        }];
        let p = ev.to_prompt();
        assert!(p.contains("hunter-fresh"), "{p}");
        // **工作目录也要在证据里**（I7 实测）：`reuse_existing_hunter` 的计划里会写它，
        // 证据里没有的话复核员会判「凭空编造」并否决 —— 而且它判得对
        assert!(p.contains("工作目录"), "{p}");
        assert!(
            p.contains("hunter-fresh") && p.contains("compose 文件"),
            "{p}"
        );
        assert!(p.contains("不要提议停掉、删掉、改动它们"), "{p}");
        // 例外也要写明 —— 不写的话复核员会把「用户亲手同意的接管」也一并否决（I7 实测）
        assert!(p.contains("reuse_existing_hunter"), "{p}");
        assert!(p.contains("用户亲自点过同意"), "{p}");
        assert!(p.contains("8100"), "{p}");
    }

    #[test]
    fn 提示词里明确禁止动别人的东西() {
        for must in ["绝对不要提议停掉", "不要提议删除用户的文件", "sudo"] {
            assert!(AUTO_SYSTEM_PROMPT.contains(must), "提示词里缺：{must}");
        }
    }

    // ── I7 ────────────────────────────────────────────────────────────────

    /// 授权页勾过的**只有 `install_runtime` 这一个**。
    /// `reuse_existing_hunter` 动的是用户自己装的那一套，没有「一次授权、以后不问」的道理。
    #[test]
    fn 提前批过的只有装运行时那一个() {
        let _g = crate::paths::test_home("auto-preauth");
        let o = orch(Mode::Auto);
        let mut cfg = LauncherConfig::load();
        let old = cfg.assist.allow_install_runtime;
        cfg.assist.allow_install_runtime = true;
        cfg.save().unwrap();
        assert!(o.pre_authorized(&Call::new("install_runtime")));
        assert!(
            !o.pre_authorized(&Call::new("reuse_existing_hunter")),
            "动用户已有的那一套，永远要当场问"
        );
        assert!(!o.pre_authorized(&Call::new("compose_down_own")));
        // 取消勾选之后连它也要问
        cfg.assist.allow_install_runtime = false;
        cfg.save().unwrap();
        assert!(!o.pre_authorized(&Call::new("install_runtime")));
        cfg.assist.allow_install_runtime = old;
        cfg.save().unwrap();
    }

    /// 复核只对含 `Sensitive` 的计划触发；`Safe` 的计划一个 token 都不该花。
    #[test]
    fn 只有_sensitive_计划才触发复核() {
        assert!(!super::super::reviewer::needs_review(&[
            Call::new("remap_ports"),
            Call::new("compose_down_own"),
        ]));
        assert!(super::super::reviewer::needs_review(&[Call::new(
            "install_runtime"
        )]));
    }

    /// 没 key 时复核**不是默认放行**，而是「复核做不成」。
    #[test]
    fn 没_key_时复核不默认放行() {
        let mut o = orch(Mode::Auto);
        let ev = ev_empty();
        let v = o.review(0, &[Call::new("install_runtime")], "要装一套", &ev);
        assert!(!v.approve, "没 key 就放行等于这道门不存在");
        assert!(v.degraded.is_some(), "要说清为什么没复核成");
        assert_eq!(v.tokens, 0);
    }

    /// `off` 档下不问模型，复核也不做 —— 但同样**不默认放行**。
    #[test]
    fn off_档下复核也不默认放行() {
        let mut o = orch(Mode::Off);
        let ev = ev_empty();
        let v = o.review(0, &[Call::new("install_runtime")], "x", &ev);
        assert!(!v.approve);
        assert!(
            v.degraded.as_deref().unwrap_or("").contains("关着"),
            "{v:?}"
        );
    }

    /// 动作表里 I7 新增的那几条都在，级别也对。
    #[test]
    fn i7_的动作在表里且级别正确() {
        use super::super::guard::Level;
        for (id, lv) in [
            ("install_runtime", Level::Sensitive),
            ("reuse_existing_hunter", Level::Sensitive),
            ("start_builtin_runtime", Level::Safe),
            ("uninstall_builtin_runtime", Level::Safe),
        ] {
            let sp = actions::spec(id).unwrap_or_else(|| panic!("{id} 不在动作表里"));
            assert_eq!(sp.level, lv, "{id} 的级别不对");
        }
    }

    /// 「没有 Docker」这一条现在**先硬找一遍**，找不到才谈装 ——
    /// I5 场景 1 那 8,632 token 就花在「把这一步交给模型」上（待办池 P1-23）。
    #[test]
    fn 找不到_docker_时规则层自己先硬找一遍() {
        let o = orch(Mode::Auto);
        let e = AppError::new(Code::DockerMissing, "找不到 docker");
        let ev = ev_empty();
        // 这台机器上有没有 docker 不一定，所以两种结果都接受 ——
        // 唯一不接受的是「规则层什么都不给、直接把它甩给模型」
        match o.rule_plan(Step::Docker, &e, &ev) {
            Some((calls, why)) => {
                let ids: Vec<&str> = calls.iter().map(|c| c.id.as_str()).collect();
                assert!(
                    ids.contains(&"set_docker_path")
                        || ids.contains(&"install_runtime")
                        || ids.contains(&"start_runtime")
                        || ids.contains(&"start_builtin_runtime"),
                    "{ids:?}"
                );
                assert!(!why.is_empty());
                for c in &calls {
                    assert!(actions::spec(&c.id).is_some(), "表外动作 {}", c.id);
                }
            }
            None => {
                // 只有在「这个平台装不了内置运行时、也没有可点亮的运行时」时才允许
                assert!(
                    !cfg!(target_os = "macos") || crate::runtime::builtin::supported().is_err(),
                    "macOS 上应该给得出办法"
                );
            }
        }
    }

    /// 「直接用它」那张卡片**不阻塞**：它的 value 带 `takeover:` 前缀，
    /// 走的是另一条通路，不经过那个会挂住的 condvar。
    #[test]
    fn 接管请求不走阻塞通道() {
        let _g = crate::paths::test_home("auto-takeover-req");
        let o = orch(Mode::Auto);
        let h = o.handle();
        // 没有任何正在等的提问，普通回答返回 false
        assert!(!h.answer("yes"));
        // 接管请求照样收得下 —— 因为它根本不需要有人在等
        assert!(h.answer(&format!("{TAKEOVER_PREFIX}hunter-community")));
        assert_eq!(
            o.takeover.lock().unwrap().clone(),
            Some("hunter-community".to_string())
        );
        // 空项目名不收
        assert!(!h.answer(TAKEOVER_PREFIX));
        // 「和它并存」也收得下（它只是把话说死，什么都不改）
        assert!(h.answer("coexist"));
    }

    // ── I9 ──────────────────────────────────────────────────────────────

    use crate::runtime::effective::{make_fake_tools, make_fake_tools_partial};

    /// **总指挥这一侧的同一个 P0**：`E_BUILTIN_DOWN` 必须走内置主线，
    /// 零 token、不碰用户的 colima。
    #[test]
    fn 内置运行时没起来时总指挥走内置主线() {
        let _g = crate::paths::test_home("auto-builtin-down");
        make_fake_tools();
        let o = orch(Mode::Auto);
        let ev = ev_empty();
        let e = AppError::new(Code::BuiltinRuntimeDown, "内置运行时装好了，虚拟机没起来");
        let (calls, why) = o
            .rule_plan_raw(Step::Docker, &e, &ev)
            .expect("这一条规则层必须认得出来");
        assert_eq!(calls[0].id, "start_builtin_runtime", "{calls:?}");
        assert!(
            !calls.iter().any(|c| c.id == "start_runtime"),
            "绝不能提 start_runtime：{calls:?}"
        );
        assert!(why.contains("hunter"), "{why}");
    }

    /// 起过一次没成 → 第二回合改成「清残骸重建」，而不是原样再试一遍。
    /// （0.1.8 那次日志里同一条裸 `colima start` 被执行了四次。）
    #[test]
    fn 起过一次没成就清残骸重建而不是再试一遍() {
        let _g = crate::paths::test_home("auto-builtin-repair");
        make_fake_tools();
        let mut o = orch(Mode::Auto);
        o.failed_actions.insert("start_builtin_runtime".to_string());
        let ev = ev_empty();
        let e = AppError::new(Code::BuiltinRuntimeDown, "虚拟机还是没起来");
        let (calls, why) = o
            .rule_plan_raw(Step::Docker, &e, &ev)
            .expect("第二回合也要有办法");
        assert_eq!(calls[0].id, "repair_builtin_runtime", "{calls:?}");
        // 话里要写清**清的范围**（这是承诺，不是措辞）
        assert!(why.contains("~/.hunter/runtime"), "{why}");
        assert!(why.contains("~/.colima"), "{why}");
    }

    /// 装了一半 → 先补齐（`install_runtime`），不是直接去起一台起不来的虚拟机。
    #[test]
    fn 内置运行时装了一半时总指挥先补齐() {
        let _g = crate::paths::test_home("auto-builtin-partial");
        make_fake_tools_partial();
        let o = orch(Mode::Auto);
        let ev = ev_empty();
        let e = AppError::new(Code::BuiltinRuntimeDown, "还缺几个文件");
        let (calls, why) = o
            .rule_plan_raw(Step::Docker, &e, &ev)
            .expect("装了一半也要认得出来");
        assert_eq!(calls[0].id, "install_runtime", "{calls:?}");
        assert!(why.contains("还缺"), "{why}");
    }

    /// `E_DAEMON_DOWN` 那条路上，`builtin` **不能**被当成「用户装了但没起」。
    #[test]
    fn daemon_down_那条路不碰_builtin() {
        let _g = crate::paths::test_home("auto-daemon-down");
        let o = orch(Mode::Auto);
        let mut ev = ev_empty();
        ev.report.apps = vec![crate::assist::probe::AppPresence {
            id: "builtin".into(),
            label: "Hunter 内置运行时".into(),
            installed: Some(true),
            running: Some(false),
            evidence: "测试".into(),
        }];
        let e = AppError::new(Code::DaemonDown, "daemon 没起");
        assert!(
            o.rule_plan_raw(Step::Docker, &e, &ev).is_none(),
            "builtin 不该走 start_runtime 那条"
        );
    }

    /// 计划的**头一条**已经失败过时，整条计划作废 —— 不能把后面那条
    /// 「等它生效」单独拎出来跑。
    ///
    /// 测试机上复现内置运行时残骸时真撞到了：第 3 回合只剩一条 `wait_daemon`，
    /// 白等 121 秒；而事件流里「找到原因」写的还是「清掉残骸重建」。
    #[test]
    fn 头一条动作失败过就不要单独跑后面那条等待() {
        let _g = crate::paths::test_home("auto-lead-failed");
        make_fake_tools();
        let mut o = orch(Mode::Auto);
        let ev = ev_empty();
        let e = AppError::new(Code::BuiltinRuntimeDown, "虚拟机没起来");
        // 头一条还没失败过：计划里既有「起虚拟机」也有「等它就绪」
        let (calls, _) = o.rule_plan(Step::Docker, &e, &ev).expect("该有计划");
        assert_eq!(calls[0].id, "start_builtin_runtime");
        assert!(calls.iter().any(|c| c.id == "wait_daemon"), "{calls:?}");

        // 起虚拟机失败过 → 换成「清残骸重建」，头一条仍然是「做事」的那一条
        o.failed_actions.insert("start_builtin_runtime".to_string());
        let (calls, _) = o.rule_plan(Step::Docker, &e, &ev).expect("该换一条路");
        assert_eq!(calls[0].id, "repair_builtin_runtime");

        // 重建也失败过 → **整条作废**，不许只剩一条 wait_daemon 去干等
        o.failed_actions
            .insert("repair_builtin_runtime".to_string());
        assert!(
            o.rule_plan(Step::Docker, &e, &ev).is_none(),
            "只剩 wait_daemon 的计划不该再出"
        );
    }

    // ── I10：虚拟机没有 DNS ──────────────────────────────────────────

    fn ev_with_container_fail(fail: crate::runtime::netcheck::Fail) -> Evidence {
        let mut ev = ev_empty();
        ev.container_net = Some(crate::runtime::netcheck::Outcome {
            ok: false,
            fail,
            command: "docker run --rm --entrypoint curl …".into(),
            detail: "curl: (6) Could not resolve host: hunter.agentpit.io".into(),
            elapsed_ms: 1200,
        });
        ev
    }

    /// 0.1.9 在用户 Mac 上那一幕：`E_START_TIMEOUT` + 容器解析不了域名。
    /// **这一轮要的就是「规则层自己认得出来」**，而不是落到模型那边 unknown。
    #[test]
    fn 启动超时但容器解析不了域名时走修_dns_而不是别的() {
        let o = orch(Mode::Auto);
        let ev = ev_with_container_fail(crate::runtime::netcheck::Fail::NoDns);
        let e = AppError::new(
            Code::StartTimeout,
            "等了 180 秒，还有 1 个服务没就绪：opencode",
        );
        let (calls, why) = o
            .rule_plan(Step::Start, &e, &ev)
            .expect("这个现场规则层必须认得出来");
        assert_eq!(calls[0].id, "fix_vm_dns", "第一步就该是修 DNS：{calls:?}");
        assert!(why.contains("解析不了域名"), "{why}");
    }

    /// 反过来：容器连得上、只是慢 —— 那就**不该**去动 DNS。
    #[test]
    fn 容器连得上时启动超时不会被当成_dns_问题() {
        let o = orch(Mode::Auto);
        let mut ev = ev_empty();
        ev.container_net = Some(crate::runtime::netcheck::Outcome {
            ok: true,
            fail: crate::runtime::netcheck::Fail::None,
            command: "docker run …".into(),
            detail: "401".into(),
            elapsed_ms: 700,
        });
        let e = AppError::new(Code::StartTimeout, "等了 180 秒");
        let plan = o.rule_plan(Step::Start, &e, &ev);
        assert!(
            plan.as_ref().is_none_or(|(c, _)| c[0].id != "fix_vm_dns"),
            "不该去动 DNS：{plan:?}"
        );
    }

    /// 「解析超时」和「一个 nameserver 都没有」是同一类事，都该被认出来。
    #[test]
    fn 解析超时也算没有_dns() {
        let o = orch(Mode::Auto);
        let ev = ev_with_container_fail(crate::runtime::netcheck::Fail::NoDns);
        let e = AppError::new(Code::RuntimeNoDns, "虚拟机解析不了域名");
        let (calls, _) = o.rule_plan(Step::Docker, &e, &ev).expect("该有计划");
        assert_eq!(calls[0].id, "fix_vm_dns");
    }

    /// 修过一次还是同一个码 —— **不再原样提第二遍**（沿用 I9 那条规矩）。
    #[test]
    fn 修过一次_dns_还不行就不再提同一条() {
        let mut o = orch(Mode::Auto);
        o.failed_actions.insert("fix_vm_dns".to_string());
        let ev = ev_with_container_fail(crate::runtime::netcheck::Fail::NoDns);
        for (code, step) in [
            (Code::RuntimeNoDns, Step::Docker),
            (Code::ContainerOffline, Step::Start),
            (Code::StartTimeout, Step::Start),
        ] {
            let e = AppError::new(code, "x");
            let plan = o.rule_plan(step, &e, &ev);
            assert!(
                plan.as_ref().is_none_or(|(c, _)| c[0].id != "fix_vm_dns"),
                "{code:?} 不该再提一次：{plan:?}"
            );
        }
    }

    /// **「生成一份诊断包」不是一次修复。**
    ///
    /// 这一条是实测补的（I10 第四节场景 G）：对着一台「容器没有 DNS」的 docker
    /// 跑全自动安装，规则层正确地让路、模型合理地选了 `export_feedback_bundle`，
    /// 然后总指挥把它当成「做了点什么」→ 重跑 → 又失败 → 再来一遍，
    /// 转了 4 个回合、22,400 token。
    #[test]
    fn 只生成了诊断包不算做过事() {
        assert!(actions::REPAIRS_NOTHING.contains(&"export_feedback_bundle"));
        // 反面：真会改变现场的那几个一个都不能进这张表
        for id in [
            "remap_ports",
            "switch_registry",
            "fix_vm_dns",
            "restart_stack",
            "compose_down_own",
            "install_runtime",
            "start_builtin_runtime",
        ] {
            assert!(
                !actions::REPAIRS_NOTHING.contains(&id),
                "{id} 是真的会改变现场的，不该被当成空转"
            );
        }
        // 表里每一条都得真的在动作表里（写错字就会静默失效）
        for id in actions::REPAIRS_NOTHING {
            assert!(actions::spec(id).is_some(), "{id} 不在动作表里");
        }
    }

    /// 容器连不上、但**不是 DNS 的事**（代理挡了 443 之类）——
    /// 那不是我们能修的，规则层让路，不去瞎改虚拟机。
    #[test]
    fn 容器连不上但不是_dns_的事就不乱动() {
        let o = orch(Mode::Auto);
        let ev = ev_with_container_fail(crate::runtime::netcheck::Fail::Unreachable);
        let e = AppError::new(Code::StartTimeout, "等了 180 秒");
        let plan = o.rule_plan(Step::Start, &e, &ev);
        assert!(
            plan.as_ref().is_none_or(|(c, _)| c[0].id != "fix_vm_dns"),
            "{plan:?}"
        );
    }

    /// I9 的 P0-3：`can_resume_install` 的判据得站得住。
    ///
    /// 没有可用的 docker 时一律为假 —— 不然会在 daemon 还没起来的时候
    /// 把安装反复踢起来。
    #[test]
    fn 没有可用的_docker_时不会自动续装() {
        let _g = crate::paths::test_home("auto-resume-no-docker");
        if crate::runtime::effective::current().usable() {
            // 这台机器上真有一个在跑的 docker（测试机就是），这一条断言不了
            return;
        }
        assert!(!crate::assist::can_resume_install());
    }
}
