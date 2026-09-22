//! AI 诊断助手（I4 §三）。**分两层，顺序不能反。**
//!
//! ```text
//! 用户卡住了
//!   │
//!   ├─ 第一层 · 确定性规则（rules.rs）     零延迟、零 token、结果可复现
//!   │    ├─ 认出来了且有把握（confident）  → 就到这里为止，给动作，不调模型
//!   │    └─ 认不出来 / 用户点「还是不行」  ↓
//!   │
//!   └─ 第二层 · AI 兜底（ai.rs）          最多 3 轮，动作只能从白名单里挑
//!        └─ 任何一条前提不满足             → 退回第一层的静态指引，并说明原因
//! ```
//!
//! 边界划得很清楚：
//!
//! * **确定性的事永远不交给 AI**。可执行文件定位（`runtime::which`）、
//!   「装了但没起来」的识别、端口冲突、磁盘不足 —— 这些都在第一层，
//!   同一个现象每次给的都是同一句话。
//! * **AI 不碰执行**。它能做的只有「说一段话」和「从写死的白名单里挑一个动作」。
//!   [`actions`] 那一层逐项校验参数，表外的一律拒绝并记日志。
//! * **key 与用户名不出门**。诊断报文两道脱敏（[`crate::redact::redact`] +
//!   [`crate::redact::mask_home`]），发之前整份再过一遍
//!   [`probe::assert_no_secret`]。

pub mod actions;
pub mod ai;
pub mod auto;
pub mod events;
pub mod guard;
pub mod probe;
pub mod reviewer;
pub mod rules;

use std::sync::Mutex;

use serde::Serialize;

use crate::err::{AppError, AppResult, Code};

/// 界面拿到的完整快照。一次 IPC 就能把该显示的都拿全。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistState {
    /// 设置里开着没有
    pub enabled: bool,
    /// 有没有可用的 key（AI 那一层要）
    pub has_key: bool,
    /// 第一层给的结论。**任何时候都有**
    pub rule: rules::Suggestion,
    /// AI 那几轮。没问过就是空
    pub turns: Vec<ai::Turn>,
    /// 还在等用户确认的动作
    pub pending: Vec<actions::Plan>,
    /// 这次诊断一共花掉的 token
    pub total_tokens: u64,
    /// 已经问了几轮 / 上限几轮
    pub rounds: usize,
    pub max_rounds: usize,
    /// 退回第一层的原因；正常时为 `None`
    pub degraded: Option<Degraded>,
    /// AI 这一层是不是已经收尾了（给出结论或用完轮数）
    pub done: bool,
    /// 整份诊断报文（**已脱敏**）。界面上「复制诊断信息」用的就是它
    pub report_text: String,
    /// **现在可以接着往下装了**（I9 的 P0-3）。
    ///
    /// 为真的条件有两条，缺一不可：docker 后台服务连得上了，而且这台机器上
    /// 这一套 Hunter 还没装完。界面拿到它就自己回到安装页，不再把用户
    /// 晾在一张「没能自动装好」的卡片上 —— 0.1.8 在用户 Mac 上正是这样：
    /// 14:54 AI 把内置运行时起起来了、docker 好了，安装却一步都没往下走，
    /// `~/.hunter/app/` 到最后还是空的。
    pub can_resume_install: bool,
    /// 规则层这一次**自动替用户执行**了哪几个动作（全自动档才会有）。
    /// 界面上照样一条条显示出来 —— 自动做了什么必须看得见。
    pub auto_ran: Vec<AutoRan>,
}

/// 一条「规则层自动执行了」的记录。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoRan {
    pub id: String,
    pub title: String,
    pub ok: bool,
    /// 执行结果那一句（已脱敏）
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Degraded {
    pub reason: ai::Degrade,
    pub message: String,
}

impl Degraded {
    fn of(d: ai::Degrade) -> Self {
        Self {
            reason: d,
            message: d.message().to_string(),
        }
    }
}

/// 进程内的会话。用户确认动作发生在两轮之间，状态得存着。
struct Live {
    report: probe::Report,
    rule: rules::Suggestion,
    session: Option<ai::Session>,
    degraded: Option<Degraded>,
    /// 这一段会话里规则层**自动**跑过的动作（全自动档，I9 的 P0-3）
    auto_ran: Vec<AutoRan>,
}

fn slot() -> &'static Mutex<Option<Live>> {
    static S: std::sync::OnceLock<Mutex<Option<Live>>> = std::sync::OnceLock::new();
    S.get_or_init(|| Mutex::new(None))
}

fn lock() -> AppResult<std::sync::MutexGuard<'static, Option<Live>>> {
    slot()
        .lock()
        .map_err(|_| AppError::new(Code::Unknown, "诊断助手的状态锁坏了".to_string()))
}

/// **第一层**：采现场 + 跑规则。零 token，任何时候都能调。
///
/// 每调一次就开一段新会话 —— 用户点「重新诊断」时不该带着上一次的上下文。
pub fn diagnose(
    error_code: Option<&str>,
    error_message: Option<&str>,
    stage: Option<&str>,
    key: Option<String>,
) -> AppResult<AssistState> {
    diagnose_inner(error_code, error_message, stage, key, false, Vec::new(), 0)
}

/// 同上，**但全自动档下会把规则层给的 Safe 动作自己跑掉**（I9 的 P0-3）。
///
/// 界面走这一条；命令行的 `--diagnose` 走上面那条只读的。
/// 分成两个入口是因为 `--diagnose` 在文档里写的是「打印诊断信息」——
/// 一条印东西的命令不该顺手改机器。
pub fn diagnose_and_fix(
    error_code: Option<&str>,
    error_message: Option<&str>,
    stage: Option<&str>,
    key: Option<String>,
) -> AppResult<AssistState> {
    diagnose_inner(error_code, error_message, stage, key, true, Vec::new(), 0)
}

/// 这个级别能不能在**诊断面板这条路**上自动跑（I9 的 P0-3）。
///
/// 只有 Safe / ReadOnly。`Sensitive` 走总指挥那条完整通路 ——
/// 那边在执行前还有复核员那道门，这里没有；级别放宽一档、又少一道门，
/// 两件事凑一起就是「悄悄把门拆了」。
fn auto_runnable_level(level: guard::Level) -> bool {
    level != guard::Level::Sensitive
}

/// 同一个问题最多自动跑几轮动作。到头了就停下来如实说，不无限试（I9）。
const MAX_AUTO_ROUNDS: usize = 3;

fn diagnose_inner(
    error_code: Option<&str>,
    error_message: Option<&str>,
    stage: Option<&str>,
    key: Option<String>,
    auto_fix: bool,
    mut auto_ran: Vec<AutoRan>,
    round: usize,
) -> AppResult<AssistState> {
    // 定位缓存先清掉：用户很可能刚装完 Docker 或刚把 OrbStack 点起来
    crate::runtime::which::invalidate();
    crate::runtime::env::invalidate();
    let report = probe::collect(error_code, error_message, stage);
    let rule = rules::diagnose(&report);
    crate::linfo!(
        "诊断助手（规则层）：{} · confident={} · 动作 {} 个",
        rule.rule,
        rule.confident,
        rule.actions.len()
    );

    // **全自动档下，规则层有把握的 Safe / ReadOnly 动作自己跑掉**（I9 的 P0-3）。
    //
    // 0.1.8 在用户 Mac 上，自动安装失败之后界面掉进了「每一步都要你点确认」
    // 的模式 —— 日志里 14:52:52 / 14:52:58 / 14:53:24 / 14:57:40 四次
    // 「Safe · confirm」，**用户点了四次**。可他在授权页上选的是全自动。
    // 档位是 auto 就不该再问：把关的门一道没少（动作表 + 守卫 + 级别判定），
    // 只是不再让用户去按那个按钮。
    let mode = crate::config::LauncherConfig::load().assist.mode();
    if auto_fix && mode == guard::Mode::Auto && rule.confident && round < MAX_AUTO_ROUNDS {
        let todo: Vec<actions::Plan> = rule
            .actions
            .iter()
            // **只自动跑 Safe / ReadOnly**（任务书 P0-3 的原话就是「Safe 级动作」）。
            //
            // `Sensitive` 不在这条路上：总指挥那一侧在执行它之前还有**复核员**
            // 那道门（`reviewer::needs_review`），这里没有。级别放宽一档、
            // 少一道门——两件事凑一起就是「悄悄把门拆了」。
            // 真要装运行时，走的是总指挥那条完整通路。
            .filter(|p| auto_runnable_level(p.level))
            .filter(|p| !mode.needs_confirm(p.level))
            .filter(|p| !p.user_only)
            .filter(|p| !auto_ran.iter().any(|r| r.id == p.id))
            .cloned()
            .collect();
        let mut any = false;
        for p in &todo {
            let call = rule_call(p);
            crate::linfo!(
                "诊断助手（全自动档）自己执行规则层动作 {}（{}）",
                call.id,
                p.level.cn()
            );
            match actions::execute_as(&call, mode, false, guard::Proposer::Rule) {
                Ok(o) => {
                    auto_ran.push(AutoRan {
                        id: o.id.clone(),
                        title: p.title.clone(),
                        ok: true,
                        text: o.text.clone(),
                    });
                    any = true;
                }
                Err(e) => {
                    auto_ran.push(AutoRan {
                        id: p.id.clone(),
                        title: p.title.clone(),
                        ok: false,
                        text: e.msg.clone(),
                    });
                    // 后面那几个多半是接着这一个来的，前一个没成就别白试
                    break;
                }
            }
        }
        if any {
            // 跑过动作就**重新采一遍现场**（验证员的老规矩：不问模型「好了吗」，
            // 自己重跑一遍看结果）
            return diagnose_inner(
                error_code,
                error_message,
                stage,
                key,
                auto_fix,
                auto_ran,
                round + 1,
            );
        }
    }

    let mut g = lock()?;
    *g = Some(Live {
        report,
        rule,
        session: None,
        degraded: None,
        auto_ran,
    });
    // key 要带进来：向导途中它只在内存里（`.env` 还没写出来），
    // 不带的话界面右下角会写「还没填 key」—— 而用户上一步刚填过
    // （I4 场景 1 的界面截图上一眼就看出来了）
    snapshot_with(g.as_ref().expect("刚写进去"), key.as_deref())
}

/// **现在可以接着往下装了吗**（I9 的 P0-3）。
///
/// 两条都要成立：
///
/// 1. docker 后台服务连得上了（[`crate::runtime::effective`] 说了算）；
/// 2. 这一套 Hunter 还**没装完** —— 判据是 compose 文件与 `.env` 有没有落地、
///    六个容器起没起齐。装完了还自动重装一遍是另一种毛病。
///
/// 为什么需要它：0.1.8 在用户 Mac 上 14:54:27 已经把内置运行时起起来了
/// （docker 29.5.2 可用，本地 Claude 事后实测 `docker info` 正常），
/// 可安装一步都没往下走 —— `~/.hunter/app/` 到 14:58 还是空的，
/// 界面停在「没能自动装好」，用户只能导出诊断包。
/// **修好了就该接着装**，这一条就是那个判据。
///
/// 这个函数没有副作用，界面与命令层都可以随便调。
pub fn can_resume_install() -> bool {
    if !crate::runtime::effective::current().usable() {
        return false;
    }
    // compose 文件或 .env 还没写出来 —— 那是「配置这一步都没走完」
    if !crate::paths::compose_file().exists() || !crate::paths::env_file().exists() {
        return true;
    }
    // 写出来了，但容器没起齐
    match crate::compose::ps() {
        Ok(v) => v.len() < guard::OWN_SERVICES.len(),
        // 读不到就当「还没装完」：多试一次的代价，远小于把人晾在失败页上
        Err(_) => true,
    }
}

/// **第二层**：问一轮 AI。规则层认不出来、或者用户点了「还是不行」时才调。
///
/// `key` 由调用方给 —— 这一点很要紧：I4 把「输入 key」挪到了「检测 Docker」**前面**，
/// 而这时候 `~/.hunter/app/.env` **还没写出来**（它是拉取那一步才生成的），
/// key 只在 [`crate::flow::AppState`] 的内存里。只从 `.env` 读的话，
/// 恰恰在这一层最需要它的时刻读不到（自审时撞到的）。
pub fn ask(key: Option<String>) -> AppResult<AssistState> {
    let mut g = lock()?;
    let live = g.as_mut().ok_or_else(|| {
        AppError::new(
            Code::Unknown,
            "还没有采过现场，先调 assist_diagnose".to_string(),
        )
    })?;

    // 降级判断按顺序来，每一条都要能说清楚
    let cfg = crate::config::LauncherConfig::load();
    if !cfg.assist.enabled {
        live.degraded = Some(Degraded::of(ai::Degrade::Disabled));
        return snapshot_with(live, key.as_deref());
    }
    let Some(key) = usable_key(key) else {
        live.degraded = Some(Degraded::of(ai::Degrade::NoKey));
        return snapshot_with(live, None);
    };

    if live.session.is_none() {
        live.session = Some(ai::start(&live.report));
    }
    let session = live.session.as_mut().expect("刚建好");
    match ai::step(session, &key) {
        Ok(t) => {
            crate::linfo!(
                "诊断助手（AI 第 {} 轮）：自动执行 {} 个 · 待确认 {} 个 · 拒绝 {} 个 · token {}",
                t.round,
                t.ran.len(),
                t.pending.len(),
                t.rejected.len(),
                t.tokens
                    .map(|x| x.to_string())
                    .unwrap_or_else(|| "未知".into())
            );
            live.degraded = None;
        }
        Err(d) => {
            crate::lwarn!("诊断助手退回规则层：{:?}", d);
            live.degraded = Some(Degraded::of(d));
        }
    }
    snapshot_with(live, Some(&key))
}

/// 用户确认执行一个动作。执行 → 自动复验 → 带着新结果再问一轮。
///
/// `next_round` 为真时顺手把下一轮也问了（界面上是一次点击的事）。
pub fn confirm(action_id: &str, next_round: bool, key: Option<String>) -> AppResult<AssistState> {
    {
        let mut g = lock()?;
        let live = g
            .as_mut()
            .ok_or_else(|| AppError::new(Code::Unknown, "还没有采过现场".to_string()))?;
        let session = live
            .session
            .as_mut()
            .ok_or_else(|| AppError::new(Code::Unknown, "这一轮没有等待确认的动作".to_string()))?;
        ai::confirm(session, action_id)?;
    }
    if next_round {
        return ask(key);
    }
    let g = lock()?;
    snapshot_with(g.as_ref().expect("上面刚用过"), key.as_deref())
}

/// 执行**规则层**给的动作（那条路不经过模型，所以也不需要会话）。
/// 执行完重新跑一遍规则 —— 用户要看的是「现在好了没有」。
pub fn run_rule_action(action_id: &str, key: Option<String>) -> AppResult<AssistState> {
    let call = {
        let g = lock()?;
        let live = g
            .as_ref()
            .ok_or_else(|| AppError::new(Code::Unknown, "还没有采过现场".to_string()))?;
        let p = live
            .rule
            .actions
            .iter()
            .find(|p| p.id == action_id)
            .ok_or_else(|| {
                AppError::new(
                    Code::NotImplemented,
                    format!("动作「{action_id}」不在这一条建议里。"),
                )
            })?;
        rule_call(p)
    };
    let out = actions::execute(&call, true)?;
    crate::linfo!("诊断助手执行规则层动作 {}：ok={}", call.id, out.ok);
    // 复验：重新采一遍
    let (code, msg, stage) = {
        let g = lock()?;
        let live = g.as_ref().expect("上面用过");
        (
            live.report.error_code.clone(),
            live.report.error_message.clone(),
            live.report.stage.clone(),
        )
    };
    // 复验走**带自动修复**的那一条（I9 的 P0-3）：全自动档下，这一步之后
    // 紧跟着的 Safe 动作不该再让用户点一次。0.1.8 在用户 Mac 上就是这么
    // 连点四次的（14:52:52 / 14:52:58 / 14:53:24 / 14:57:40）。
    diagnose_and_fix(code.as_deref(), msg.as_deref(), stage.as_deref(), key)
}

/// 规则层的 plan 反推回调用。规则层自己知道参数，所以这里用和 [`ai`] 同一套反推。
fn rule_call(p: &actions::Plan) -> actions::Call {
    // 与 ai::confirm 用的是同一个逻辑；放在这里是为了让规则层那条路不依赖会话
    let mut c = actions::Call::new(&p.id);
    match p.id.as_str() {
        "start_runtime" => {
            // **按标题反推是有坑的**：「Colima」四个字同时出现在
            // 「启动 Colima」和「启动 你自己的 Colima」里。挑**最长**的那一个匹配项，
            // 短的那个不会把长的顶掉（I9 自审）
            let app = actions::RUNTIME_APPS
                .iter()
                .filter(|a| p.title.contains(actions::runtime_label(a)))
                .max_by_key(|a| actions::runtime_label(a).len())
                .copied()
                .unwrap_or("systemd");
            c.args.insert("app".into(), app.to_string());
        }
        "switch_registry" => {
            let id = crate::registry::CANDIDATES
                .iter()
                .find(|x| {
                    p.summary
                        .as_deref()
                        .unwrap_or("")
                        .contains(x.label.as_ref())
                })
                .map(|x| x.id.to_string())
                .unwrap_or_default();
            c.args.insert("registry".into(), id);
        }
        _ => {}
    }
    c
}

/// 清掉会话（用户关掉面板 / 重新走一遍向导）。
pub fn reset() -> AppResult<()> {
    let mut g = lock()?;
    *g = None;
    Ok(())
}

/// 拿一把能用的 key：调用方给的优先（向导途中它只在内存里），
/// 没有就从已经装好的 `~/.hunter/app/.env` 里读回来（第二次打开启动器时走这条）。
///
/// 形状不对的一律当成没有 —— 拿一串乱码去打网关只会换回一个 401。
fn usable_key(given: Option<String>) -> Option<String> {
    let k = given.filter(|s| !s.is_empty()).or_else(|| {
        crate::config::parse_env_file(&crate::paths::env_file())
            .get("HUNTER_API_KEY")
            .cloned()
            .filter(|s| !s.is_empty())
    })?;
    if !crate::gateway::key_shape_ok(&k) {
        return None;
    }
    crate::redact::register_secret(&k);
    Some(k)
}

fn snapshot_with(live: &Live, key: Option<&str>) -> AppResult<AssistState> {
    let cfg = crate::config::LauncherConfig::load();
    let (turns, tokens, rounds, done) = match &live.session {
        Some(s) => (s.rounds.clone(), s.total_tokens, s.round_count(), s.done),
        None => (Vec::new(), 0, 0, false),
    };
    let pending = live
        .session
        .as_ref()
        .map(|s| s.pending.clone())
        .unwrap_or_default();
    Ok(AssistState {
        enabled: cfg.assist.enabled,
        has_key: usable_key(key.map(str::to_string)).is_some(),
        rule: live.rule.clone(),
        turns,
        pending,
        total_tokens: tokens,
        rounds,
        max_rounds: ai::MAX_ROUNDS,
        degraded: live.degraded.clone(),
        done,
        report_text: live.report.to_prompt(),
        can_resume_install: can_resume_install(),
        auto_ran: live.auto_ran.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 第一层永远能跑，不需要 key、不需要网络、不花 token。
    #[test]
    fn 规则层不依赖_key_与网络() {
        let st = diagnose(Some("E_DOCKER_MISSING"), Some("测试"), Some("docker"), None)
            .expect("规则层永远能跑");
        assert_eq!(st.total_tokens, 0, "规则层不许花 token");
        assert_eq!(st.rounds, 0);
        assert!(st.turns.is_empty());
        assert!(!st.rule.title.is_empty());
        // 报文里不能有 key（红线 2）
        assert_eq!(probe::assert_no_secret(&st.report_text), None);
        reset().unwrap();
    }

    #[test]
    fn 没采过现场时问_ai_会如实报错而不是_panic() {
        reset().unwrap();
        let e = ask(None).expect_err("没采过现场就该报错");
        assert!(e.msg.contains("还没有采过现场"), "{}", e.msg);
    }

    #[test]
    fn 规则层给的动作必须在白名单里() {
        let st = diagnose(Some("E_PULL_FAILED"), None, Some("pull"), None).expect("能跑");
        for a in &st.rule.actions {
            assert!(
                actions::spec(&a.id).is_some(),
                "规则层给了一个白名单之外的动作：{}",
                a.id
            );
        }
        reset().unwrap();
    }

    #[test]
    fn 规则层给的动作_id_不存在时如实拒绝() {
        diagnose(None, None, None, None).expect("能跑");
        let e = run_rule_action("不存在的动作", None).expect_err("该拒绝");
        assert!(e.msg.contains("不在这一条建议里"), "{}", e.msg);
        reset().unwrap();
    }

    /// **全自动档下自动跑掉的只能是 Safe / ReadOnly。**
    ///
    /// `Sensitive` 走的是总指挥那条完整通路（那边在执行前还有复核员那道门）。
    /// 在诊断面板这条路上放宽一档、又少一道门，等于悄悄把门拆了。
    #[test]
    fn 诊断面板这条路不自动跑_sensitive_动作() {
        let _g = crate::paths::test_home("assist-no-sensitive");
        // 表里每一个 Sensitive 动作都要被这道过滤挡住
        // 造一份「规则层给了一个 Sensitive 动作」的建议，走 `auto_runnable`
        // 那一道过滤 —— 它就是 `diagnose_inner` 里用的同一个判据
        let sensitive: Vec<&'static actions::Spec> = actions::ACTIONS
            .iter()
            .filter(|s| s.level == guard::Level::Sensitive)
            .collect();
        assert!(
            !sensitive.is_empty(),
            "动作表里应当有 Sensitive 动作，否则这条测试没有意义"
        );
        for spec in sensitive {
            assert!(
                !auto_runnable_level(spec.level),
                "{} 是 Sensitive，不该在诊断面板这条路上自动跑",
                spec.id
            );
        }
        // 反过来：Safe / ReadOnly 放行
        assert!(auto_runnable_level(guard::Level::Safe));
        assert!(auto_runnable_level(guard::Level::ReadOnly));
    }

    #[test]
    fn 快照里的轮数上限与_ai_那边一致() {
        let st = diagnose(None, None, None, None).expect("能跑");
        assert_eq!(st.max_rounds, ai::MAX_ROUNDS);
        assert_eq!(st.max_rounds, 3);
        reset().unwrap();
    }
}
