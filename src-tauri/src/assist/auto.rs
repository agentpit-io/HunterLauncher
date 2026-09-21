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

/// 单个问题最多来回几次。I4 实测 3 轮常常刚摸到答案，给 4。
pub const MAX_ROUNDS_PER_ISSUE: usize = 4;
/// 整次安装最多几个回合。防止在多个问题之间来回兜圈。
pub const MAX_ROUNDS_TOTAL: usize = 10;
/// 整次安装的 token 上限。日额度 30 万的 20%。
pub const MAX_TOKENS: u64 = 60_000;

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
}

// ── 需要用户点一下 ────────────────────────────────────────────────────────

/// 「需要你」的三种情况（设计文档 §三）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AskWhy {
    /// 要装新软件（本轮只会问，不会装 —— `install_runtime` 排在 I6）
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
}

impl Handle {
    /// 用户点了「需要你」卡片上的按钮。没有正在等的提问时返回 false。
    pub fn answer(&self, value: &str) -> bool {
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
    /// `Step::Prepare` 算出来的那一份。拉取那一步直接用它，
    /// **不要再 prepare 一遍** —— 那会白跑一次镜像源测速与 compose 下载
    prep: Option<flow::PrepareResult>,
    /// 侦察员采到的、这一次安装里已经报告过的「本机其他 Hunter」，只说一次
    reported_others: Mutex<bool>,
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
            prep: None,
            reported_others: Mutex::new(false),
            failed_actions: std::collections::HashSet::new(),
            switched_registry: std::collections::HashSet::new(),
            interactive: true,
        }
    }

    /// 命令行下没有人能点按钮，调用方要如实说一声。
    pub fn set_interactive(&mut self, v: bool) {
        self.interactive = v;
    }

    /// 跨线程的句柄：界面线程要能看事件树、回答提问、喊停，
    /// 而 [`Orchestrator::run`] 本身拿的是 `&mut self`（它是一台状态机，
    /// 不该让别的线程改它的回合数与预算）。两件事分开。
    pub fn handle(&self) -> Handle {
        Handle {
            bus: self.bus.clone(),
            ask: self.ask.clone(),
            cancel: self.cancel.clone(),
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

        for step in [Step::Docker, Step::Prepare, Step::Pull, Step::Start] {
            if self.cancelled() {
                return self.fail(Code::Unknown, "安装被取消了。");
            }
            if let Err(e) = self.run_step_with_repair(state, &mut opts, step) {
                // 最终页上要给一条能照着做的出路，而不是只丢一个错误码
                let msg = match manual_step(&e) {
                    Some((what, cmd)) => {
                        format!("{}\n{what}：在终端里执行 {cmd}，然后重试。", e.msg)
                    }
                    None => e.msg.clone(),
                };
                return self.fail(e.code, &msg);
            }
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
                    match self.repair(state, opts, step, &e) {
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
                Ok(format!(
                    "{} 正在运行",
                    d.runtime_label
                        .clone()
                        .unwrap_or_else(|| "Docker".to_string())
                ))
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

    /// 本机上别的 Hunter —— 一次安装里只报一次。
    fn report_other_installs(&self, parent: u64) {
        if self.reported_others.lock().map(|g| *g).unwrap_or(true) {
            return;
        }
        let pubs = crate::ports::docker_published();
        let others = crate::ports::other_hunter_installs(&pubs);
        if let Ok(mut g) = self.reported_others.lock() {
            *g = true;
        }
        if others.is_empty() {
            return;
        }
        let who: Vec<String> = others
            .iter()
            .map(|o| {
                format!(
                    "{}（{} 个容器，占着端口 {}）",
                    o.project,
                    o.containers.len(),
                    o.ports
                        .iter()
                        .map(|p| p.to_string())
                        .collect::<Vec<_>>()
                        .join("、")
                )
            })
            .collect();
        self.bus.emit(
            EventDraft::new(Kind::Issue, "你电脑上已经在运行另一套 Hunter")
                .under(parent)
                .status(Status::Warn)
                .detail(who.join("；"))
                .tech("处置：新装的这一套换一组空闲端口，两套并存。启动器不会停它、不会删它。"),
        );
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
        Evidence {
            report,
            others: crate::ports::other_hunter_installs(&survey.published),
            stale: compose::stale_own_containers(),
            port_lines,
            cred_helpers: crate::dockercfg::helper_status(),
            sub_env: crate::runtime::env::current().one_line(),
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

        // 第一层：确定性规则
        let plan = self.rule_plan(step, e, &ev);
        let (calls, why, by) = match plan {
            Some((c, w)) => (c, w, Proposer::Rule),
            None => {
                // 第二层：模型兜底
                match self.ask_model(issue, step, e, &ev) {
                    Some((c, w)) => (c, w, Proposer::Model),
                    None => {
                        // 规则与模型都没辙了。要不要请用户出手在函数末尾统一判，
                        // 免得同一个问题问两遍
                        self.bus.emit(
                            EventDraft::new(Kind::Failed, "这个问题我解决不了")
                                .under(issue)
                                .status(Status::Failed)
                                .detail("没有可以安全执行的办法"),
                        );
                        if let Some((what, cmd)) = manual_step(e) {
                            return Ok(self.ask_manual(issue, &what, &cmd));
                        }
                        return Ok(false);
                    }
                }
            }
        };
        if calls.is_empty() {
            return Ok(false);
        }

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
            // 「需要你」的第一种情况：Sensitive 动作在任何档位下都要先问
            if self.mode.needs_confirm(spec.level) {
                let ok = self.ask_user(issue, spec, &call);
                if !ok {
                    continue;
                }
            }
            let t = Instant::now();
            let title = actions::plan(&call)
                .map(|p| p.title)
                .unwrap_or_else(|_| spec.title.to_string());
            let aev = self
                .bus
                .emit(EventDraft::new(Kind::Action, format!("正在处理：{title}")).under(issue));
            match actions::execute_as(&call, self.mode, true, by) {
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
                    did = true;
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

        // 一个都没成。**有些事 AI 本来就不该替用户做** —— 比如 Linux 上
        // `systemctl start docker` 要 root：设计文档 §3.3 第 4 条说得很清楚，
        // 这时候降级成「请你执行这一条命令」，而不是替用户提权。
        if let Some((what, cmd)) = manual_step(e) {
            if self.ask_manual(issue, &what, &cmd) {
                return Ok(true);
            }
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
        let calls: Vec<Call> = calls
            .into_iter()
            .filter(|c| !self.failed_actions.contains(&c.id))
            .collect();
        if calls.is_empty() {
            return None;
        }
        Some((calls, why))
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
            // ② docker 找不到：多半是 GUI 程序拿不到终端的 PATH（I4 那个 macOS P0）
            Code::DockerMissing => {
                let pr = crate::runtime::which::docker_probe();
                let path = pr.resolved.clone()?;
                Some((
                    vec![Call::with("set_docker_path", "path", &path)],
                    format!(
                        "docker 其实装着，只是不在这个程序能看到的 PATH 里。按已知位置探到了 {}，写进设置。",
                        crate::redact::mask_home(&path)
                    ),
                ))
            }
            // ③ 装了没起：把它点起来再等
            Code::DaemonDown => {
                let app = ev
                    .report
                    .apps
                    .iter()
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
            // ⑤ 起不来但不是端口的事：有残留就先清，没残留就重建一次
            Code::StartTimeout if step == Step::Start => {
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
                None
            }
            _ => None,
        }
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

    /// 「这一件事只能你来做」——发一张卡片，把要敲的那一条命令原样给出来，等他做完回来。
    ///
    /// 这是设计文档 §3.3 第 4 条的落地：动作表里**没有**任何需要 sudo 的动作，
    /// 撞上要管理员权限的事就老老实实请用户出手，不替他提权、也不假装做过了。
    fn ask_manual(&self, parent: u64, what: &str, cmd: &str) -> bool {
        let ev = self.bus.emit(
            EventDraft::new(Kind::NeedUser, what)
                .under(parent)
                .status(Status::Waiting)
                .detail(format!("请在终端里执行：{cmd}"))
                .tech("这一条要管理员权限。启动器不会替你提权，也不会把它塞进动作表。")
                .choices(vec![
                    Choice {
                        value: "yes".into(),
                        label: "我执行完了，继续".into(),
                        primary: true,
                    },
                    Choice {
                        value: "no".into(),
                        label: "先不弄了".into(),
                        primary: false,
                    },
                ]),
        );
        let yes = self.wait_answer();
        self.bus.finish(
            ev,
            if yes { Status::Ok } else { Status::Skipped },
            if yes {
                "你说执行完了，这就再试一次"
            } else if self.interactive {
                "你选择先不弄"
            } else {
                "命令行下没有人能回答，按「先不弄」处理"
            },
            None,
        );
        guard::audit(
            "manual_step",
            &std::collections::BTreeMap::from([("command".to_string(), cmd.to_string())]),
            Proposer::User,
            None,
            if yes {
                "用户说已执行"
            } else {
                "用户放弃"
            },
        );
        yes
    }

    /// 挂一个提问并阻塞等答案。**不设超时** —— 悄悄超时然后自己决定，
    /// 正好是这一轮要修掉的那种行为。
    fn wait_answer(&self) -> bool {
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
                        label: label_yes,
                        primary: true,
                    },
                    Choice {
                        value: "no".into(),
                        label: "不用，我自己来".into(),
                        primary: false,
                    },
                ]),
        );
        guard::audit(
            &call.id,
            &call.args,
            Proposer::Orchestrator,
            Some(spec.level),
            "等用户确认",
        );
        let yes = self.wait_answer();
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
    pub others: Vec<crate::ports::OtherInstall>,
    pub stale: Vec<compose::StaleContainer>,
    pub port_lines: Vec<String>,
    /// 本机 docker 凭据助手：`(助手名, 补全后的 PATH 上找到的绝对路径)`（I6）
    pub cred_helpers: Vec<(String, Option<String>)>,
    /// 子进程 PATH 补了哪些目录 —— 「找不到」这类错误里，这一行是最要紧的证据
    pub sub_env: String,
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
            s.push_str("这台机器上还有别的 Hunter 安装（**不许动它们**）：\n");
            for o in &self.others {
                s.push_str(&format!(
                    "  compose 项目 {} · 容器 {} · 端口 {}\n",
                    o.project,
                    o.containers.join("、"),
                    o.ports
                        .iter()
                        .map(|p| p.to_string())
                        .collect::<Vec<_>>()
                        .join("、")
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

const AUTO_SYSTEM_PROMPT: &str = "\
你是 Hunter 启动器里的安装诊断员。启动器正在**全自动**地帮用户装 Hunter（docker compose 起的一套服务），某一步失败了。

你的职责只有一件：看证据，挑动作。规则：
1. 只用中文，不要客套。给出你认为**最可能**的那一个原因，不要罗列可能性。
2. 你不能写命令、不能让用户去敲命令。想做事只有一条路：调用给你的工具。工具表之外的一律会被启动器拒绝并记进审计日志。
3. 证据里写「读不到」的就是真的读不到 —— 不要当成 0，也不要假设一个值。**证据里没有的事实一个字都不要写**。
4. 端口被占时，占用者是谁已经写在证据里了（进程名 / 容器名 / compose 项目名）。不要猜。
5. 用户电脑上可能已经有别的 Hunter（别的 compose 项目）。**绝对不要提议停掉、删掉、改动它们** —— 正确做法永远是给新安装换一组空闲端口，两套并存。
6. 不要提议删除用户的文件、删数据卷、清理镜像、改代理 / DNS / hosts / 防火墙、用 sudo。这些动作在工具表里根本不存在，提了也只会被拒绝、白花一轮。
7. 挑最少的动作。能一步解决的不要挑三步。

背景（实测过的事实）：
- macOS 的 GUI 程序 PATH 只有 /usr/bin:/bin:/usr/sbin:/sbin，不含 /usr/local/bin，所以「终端里能跑 docker」不等于「启动器找得到 docker」。
- 国内直连 ghcr.io 经常超时，腾讯云香港的源（id 是 tencent）一般能通。
- Hunter 要 5 个端口：web 3100、api 8100、opencode 3921、postgres 5442、redis 6479，被占时可以往上挪。
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

/// AI 没有权限做、只能请用户自己敲的那几条。**不带 sudo 的动作表里一条都没有**，
/// 所以这里给的是「要你去终端里执行的命令」，不是一个会被执行的动作。
fn manual_step(e: &AppError) -> Option<(String, String)> {
    match e.code {
        Code::DaemonDown if cfg!(target_os = "linux") => Some((
            "Docker 后台服务要用管理员权限才能启动".to_string(),
            "sudo systemctl start docker".to_string(),
        )),
        Code::DockerMissing if cfg!(target_os = "linux") => Some((
            "这台机器上没有 Docker，装它要管理员权限".to_string(),
            "curl -fsSL https://get.docker.com | sh".to_string(),
        )),
        Code::DockerMissing if cfg!(target_os = "macos") => Some((
            "这台 Mac 上没有 Docker".to_string(),
            "brew install --cask orbstack（或者去 orbstack.dev 下载）".to_string(),
        )),
        // 自动这条路（补 PATH → 另起一份配置）都走不通时，剩下的只有用户自己动手。
        // **给的是「去掉 credsStore」而不是「装个助手」** —— 前者一行就能做完
        Code::CredHelper => Some((
            "你的 ~/.docker/config.json 里配了一个这台机器上找不到的凭据助手".to_string(),
            "把 config.json 里的 \"credsStore\" 那一行删掉（拉公开镜像不需要它）".to_string(),
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
            notes: Vec::new(),
        }
    }

    #[test]
    fn 预算就是设计文档里那三个数() {
        assert_eq!(MAX_ROUNDS_PER_ISSUE, 4);
        assert_eq!(MAX_ROUNDS_TOTAL, 10);
        assert_eq!(MAX_TOKENS, 60_000);
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
            s.contains("60000") || s.contains(&MAX_TOKENS.to_string()),
            "{s}"
        );
    }

    /// 端口冲突的规则层：**一个 token 都不花**就给出「换端口」，
    /// 而且绝不会提议去动用户那一套。
    #[test]
    fn 端口冲突走规则层且只换自己的端口() {
        let o = orch(Mode::Auto);
        let mut ev = ev_empty();
        ev.others = vec![crate::ports::OtherInstall {
            project: "hunter-fresh".into(),
            containers: vec!["hunter-fresh-api-1".into()],
            ports: vec![8100],
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
        let mut ev = ev_empty();
        ev.port_lines = vec!["api 8100 被占用 · Docker 容器 hunter-fresh-api-1（compose 项目 hunter-fresh · 0.0.0.0:8100->8000/tcp）".into()];
        ev.others = vec![crate::ports::OtherInstall {
            project: "hunter-fresh".into(),
            containers: vec!["hunter-fresh-api-1".into()],
            ports: vec![8100],
        }];
        let p = ev.to_prompt();
        assert!(p.contains("hunter-fresh"), "{p}");
        assert!(p.contains("不许动它们"), "{p}");
        assert!(p.contains("8100"), "{p}");
    }

    #[test]
    fn 提示词里明确禁止动别人的东西() {
        for must in ["绝对不要提议停掉", "不要提议删除用户的文件", "sudo"] {
            assert!(AUTO_SYSTEM_PROMPT.contains(must), "提示词里缺：{must}");
        }
    }
}
