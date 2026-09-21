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
    // 定位缓存先清掉：用户很可能刚装完 Docker 或刚把 OrbStack 点起来
    crate::runtime::which::invalidate();
    let report = probe::collect(error_code, error_message, stage);
    let rule = rules::diagnose(&report);
    crate::linfo!(
        "诊断助手（规则层）：{} · confident={} · 动作 {} 个",
        rule.rule,
        rule.confident,
        rule.actions.len()
    );
    let mut g = lock()?;
    *g = Some(Live {
        report,
        rule,
        session: None,
        degraded: None,
    });
    // key 要带进来：向导途中它只在内存里（`.env` 还没写出来），
    // 不带的话界面右下角会写「还没填 key」—— 而用户上一步刚填过
    // （I4 场景 1 的界面截图上一眼就看出来了）
    snapshot_with(g.as_ref().expect("刚写进去"), key.as_deref())
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
    diagnose(code.as_deref(), msg.as_deref(), stage.as_deref(), key)
}

/// 规则层的 plan 反推回调用。规则层自己知道参数，所以这里用和 [`ai`] 同一套反推。
fn rule_call(p: &actions::Plan) -> actions::Call {
    // 与 ai::confirm 用的是同一个逻辑；放在这里是为了让规则层那条路不依赖会话
    let mut c = actions::Call::new(&p.id);
    match p.id.as_str() {
        "start_runtime" => {
            let app = actions::RUNTIME_APPS
                .iter()
                .find(|a| p.title.contains(actions::runtime_label(a)))
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

    #[test]
    fn 快照里的轮数上限与_ai_那边一致() {
        let st = diagnose(None, None, None, None).expect("能跑");
        assert_eq!(st.max_rounds, ai::MAX_ROUNDS);
        assert_eq!(st.max_rounds, 3);
        reset().unwrap();
    }
}
