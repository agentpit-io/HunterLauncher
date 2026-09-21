//! 复核员（I7 · 设计文档 §四的第二个模型角色）。
//!
//! ## 它是第三道，不是唯一一道
//!
//! 计划里含 `Sensitive` 动作时触发。换一段**完全独立的系统提示词**，
//! 让模型扮演审查者，只回答两个问题：
//!
//! 1. 这个计划会不会伤到用户**已有的**数据 / 网络 / 其他项目？
//! 2. 它**对得上证据里的原话**吗，还是在说证据里没有的事？
//!
//! 输出是结构化的 `{approve, reasons[]}`。否决 → 退回诊断员并附理由，
//! 那一轮照样计进回合数与 token 预算（**否决不是免费的**，否则模型可以
//! 靠不停否决把预算耗光）。
//!
//! ### 复核员通过**不能**替代任何一道
//!
//! ```text
//! 诊断员给计划
//!    ↓
//! ① 复核员   ← 这个文件。只有 Sensitive 才走，模型说了算
//!    ↓
//! ② 守卫     ← guard.rs。代码说了算，模型无论如何绕不过
//!    ↓
//! ③ 用户确认 ← 「需要你」卡片。人说了算
//! ```
//!
//! 三道都要过。复核员说「可以」之后，守卫照样可能拒，用户照样可能点「不用」。
//! 反过来也一样：复核员说「不行」，那一条就到此为止，**不会**因为守卫觉得没问题
//! 就放行。三道是**与**的关系，不是投票。
//!
//! 这一点值得写死在测试里（见本文件末尾 `复核通过不等于可以执行`）：
//! 「多加一个模型来把关」很容易滑成「有模型把关了，那道代码就可以松一松」——
//! 那正好是反过来的。

use std::time::{Duration, Instant};

use serde::Serialize;

use super::actions::{self, Call};
use super::events::{Bus, EventDraft, Kind, Status};
use super::guard::Level;

/// 复核员的结论。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Verdict {
    /// 通过了吗
    pub approve: bool,
    /// 理由（一条条）。**通过也要给理由** —— 只有「同意」两个字等于没审
    pub reasons: Vec<String>,
    /// 这一次花掉的 token（真实值，网关给的）
    pub tokens: u64,
    pub elapsed_ms: u64,
    /// 模型没能给出结论时的原因（降级）。有值时 `approve` 一定是 false
    pub degraded: Option<String>,
}

impl Verdict {
    /// 模型这条路走不通时的结论。
    ///
    /// **走不通就是「没通过」，不是「默认通过」。** 复核员存在的意义就是多一道
    /// 把关，把关的人不在场时把门打开，那这道门不如不装。
    /// 但也不能因此把整次安装卡死 —— 所以这里返回「没通过」，
    /// 由 [`Outcome`] 那一层决定退回哪里（结果是退回去问用户，不是直接失败）。
    pub fn unavailable(why: &str) -> Self {
        Self {
            approve: false,
            reasons: vec![format!("复核这一步没能做成：{why}")],
            tokens: 0,
            elapsed_ms: 0,
            degraded: Some(why.to_string()),
        }
    }
    pub fn one_line(&self) -> String {
        let head = if self.approve { "通过" } else { "否决" };
        if self.reasons.is_empty() {
            return head.to_string();
        }
        format!("{head}：{}", self.reasons.join("；"))
    }
}

/// 这份计划要不要复核。**只看级别，不看是谁提的。**
///
/// 规则层提的 `Sensitive` 动作一样要复核 —— 规则是人写的，人也会写错。
pub fn needs_review(calls: &[Call]) -> bool {
    calls.iter().any(|c| {
        actions::spec(&c.id)
            .map(|s| s.level == Level::Sensitive)
            .unwrap_or(false)
    })
}

/// 复核员的系统提示词。**和诊断员那一份没有一个字是共用的。**
///
/// 为什么要独立：同一段上下文里换个角色说「现在请你批评自己」，模型多半会
/// 顺着上文继续同意。所以这里是**另一次调用、另一段提示词、不带诊断员的对话历史**，
/// 它看到的只有「计划 + 证据」，看不到诊断员的推理过程。
pub const SYSTEM_PROMPT: &str = "\
你是 Hunter 启动器的安全复核员。有另一个模型提出了一个修复计划，里面含有会影响用户这台电脑上**别的东西**的动作。你的职责只有一件：判断这个计划该不该放行。

你只回答两个问题：
1. 它会不会伤到用户**已有的**东西 —— 已有的数据（数据卷、数据库）、网络设置、别的 compose 项目、别的软件？
2. 它给出的理由，**对不对得上下面证据里的原话**？证据里没写的事实，计划里出现了就是编的。

判定尺度：
- 只要沾上「删用户的文件 / 删数据卷 / 停或删别的项目的容器 / 改代理、DNS、hosts、防火墙 / 用管理员权限」，一律否决。
- 计划说的原因在证据里找不到依据 —— 否决。
- 只动 Hunter 自己那一套（compose 项目 hunter）、或只动 ~/.hunter 目录里的东西 —— 这些本身不是否决理由。
- 拿不准的时候**否决**。放行一个坏计划的代价，比多问用户一次大得多。

你不是在帮用户完成安装，你是在挑毛病。不要客套，不要替别人找补。

必须只输出一个 JSON 对象，不要代码块围栏，不要任何别的文字：
{\"approve\": true 或 false, \"reasons\": [\"一句话\", \"一句话\"]}
reasons 用中文，每条一句话，至少一条。approve 为 false 时要写清是哪一条踩线了。
";

/// 真去复核一次。
///
/// * `calls` —— 诊断员给的计划（已经过 [`actions::plan`] 校验）
/// * `why` —— 诊断员给的那句原因
/// * `evidence` —— 侦察员采的证据（**原样**给复核员，它要拿来对原话）
pub fn review(calls: &[Call], why: &str, evidence: &str, key: &str) -> Verdict {
    let t = Instant::now();
    let plan_text = describe(calls);
    let user = format!(
        "## 待复核的计划\n{plan_text}\n\n## 诊断员给的理由\n{}\n\n## 现场证据（唯一可信的事实来源）\n{}\n\n\
         请按你的职责判断：放行还是否决。",
        crate::redact::redact(why),
        evidence
    );
    let messages = vec![
        serde_json::json!({"role": "system", "content": SYSTEM_PROMPT}),
        serde_json::json!({"role": "user", "content": user}),
    ];
    // **不给工具**：复核员只出结论，不该有任何动手的能力
    let resp = match super::ai::call_gateway_plain(&messages, key, Duration::from_secs(45)) {
        Ok(r) => r,
        Err(d) => return Verdict::unavailable(d.message()),
    };
    let tokens = resp
        .get("usage")
        .and_then(|u| u.get("total_tokens"))
        .and_then(|x| x.as_u64())
        .unwrap_or(0);
    let text = resp
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();
    let mut v = parse(&text);
    v.tokens = tokens;
    v.elapsed_ms = t.elapsed().as_millis() as u64;
    crate::linfo!(
        "复核员：{} · token {} · {} ms",
        if v.approve { "通过" } else { "否决" },
        v.tokens,
        v.elapsed_ms
    );
    v
}

/// 把模型回的那段文字解析成结论。**解析不出来就是「没通过」**（见 [`Verdict::unavailable`]）。
///
/// 现实里模型经常会用 ```json 围栏包起来，或者在 JSON 前后带一句话 ——
/// 所以这里从第一个 `{` 到最后一个 `}` 截一刀再解析，而不是要求整段都是 JSON。
pub fn parse(text: &str) -> Verdict {
    let t = text.trim();
    if t.is_empty() {
        return Verdict::unavailable("复核员什么都没说");
    }
    let slice = match (t.find('{'), t.rfind('}')) {
        (Some(a), Some(b)) if b > a => &t[a..=b],
        _ => return Verdict::unavailable("复核员回的不是 JSON"),
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(slice) else {
        return Verdict::unavailable("复核员回的 JSON 解析不了");
    };
    let Some(approve) = v.get("approve").and_then(|x| x.as_bool()) else {
        // `approve` 这个字段都没有 —— 不当成通过
        return Verdict::unavailable("复核员没给出明确的通过 / 否决");
    };
    let reasons: Vec<String> = v
        .get("reasons")
        .and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str())
                .map(|s| crate::redact::mask_home(&crate::redact::redact(s.trim())))
                .filter(|s| !s.is_empty())
                .take(6)
                .collect()
        })
        .unwrap_or_default();
    let reasons = if reasons.is_empty() {
        vec![if approve {
            "复核员放行了，但没写理由。".to_string()
        } else {
            "复核员否决了，但没写理由。".to_string()
        }]
    } else {
        reasons
    };
    Verdict {
        approve,
        reasons,
        tokens: 0,
        elapsed_ms: 0,
        degraded: None,
    }
}

/// 把计划渲染成复核员看得懂的一段文字。**用的是 [`actions::plan`] 的结果**，
/// 也就是真要执行的那个命令 —— 展示一套、执行另一套是最糟的骗法。
pub fn describe(calls: &[Call]) -> String {
    let mut s = String::new();
    for (i, c) in calls.iter().enumerate() {
        match actions::plan(c) {
            Ok(p) => {
                s.push_str(&format!(
                    "{}. {}（风险级别：{}）\n   要做什么：{}\n",
                    i + 1,
                    p.title,
                    p.level.cn(),
                    if p.argv.is_empty() {
                        p.summary.clone().unwrap_or_default()
                    } else {
                        p.command_line()
                    }
                ));
                s.push_str(&format!("   为什么：{}\n", p.why));
            }
            Err(e) => {
                s.push_str(&format!(
                    "{}. {}（这一条已经被守卫拒了：{}）\n",
                    i + 1,
                    c.id,
                    e.msg
                ));
            }
        }
    }
    s
}

/// 把复核结果发成事件流里那张**单独的「复核」卡片**。
pub fn emit_card(bus: &Bus, parent: u64, v: &Verdict) -> u64 {
    let mut d = EventDraft::new(
        Kind::Review,
        if v.approve {
            "复核：放行"
        } else {
            "复核：否决"
        },
    )
    .under(parent)
    .status(if v.approve { Status::Ok } else { Status::Warn })
    .detail(v.reasons.join("；"))
    .tokens(v.tokens)
    .elapsed(v.elapsed_ms);
    d = d.tech("第二个模型，单独的提示词，只判「会不会伤到你已有的东西」「对不对得上证据」");
    d = d.tech("复核通过 ≠ 可以执行：守卫与你的确认这两道照样要过");
    if let Some(x) = &v.degraded {
        d = d.tech(format!("降级：{x}"));
    }
    bus.emit(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 只有含_sensitive_的计划才复核() {
        assert!(!needs_review(&[Call::new("remap_ports")]));
        assert!(!needs_review(&[
            Call::new("check_ports"),
            Call::new("compose_down_own")
        ]));
        assert!(needs_review(&[
            Call::new("remap_ports"),
            Call::new("install_runtime")
        ]));
        assert!(needs_review(&[Call::new("reuse_existing_hunter")]));
        // 表外的动作不触发复核（它在 plan 那一步就会被拒）
        assert!(!needs_review(&[Call::new("rm_rf_everything")]));
    }

    #[test]
    fn 解析带围栏的_json() {
        let v = parse("```json\n{\"approve\": true, \"reasons\": [\"只动 ~/.hunter\"]}\n```");
        assert!(v.approve);
        assert_eq!(v.reasons, vec!["只动 ~/.hunter"]);
        assert!(v.degraded.is_none());
    }

    #[test]
    fn 解析带前后废话的_json() {
        let v =
            parse("我的判断如下：{\"approve\": false, \"reasons\": [\"会停掉别的项目\"]} 以上。");
        assert!(!v.approve);
        assert!(v.reasons[0].contains("别的项目"));
    }

    /// **解析不出来就是没通过。** 这一条是这个模块的底线。
    #[test]
    fn 解析不出来一律判没通过() {
        for bad in [
            "",
            "   ",
            "好的，我同意这个计划",
            "{不是 json}",
            "{\"reasons\": [\"x\"]}",
            "{\"approve\": \"true\"}",
        ] {
            let v = parse(bad);
            assert!(!v.approve, "「{bad}」不该被当成通过");
            assert!(v.degraded.is_some(), "「{bad}」要说清为什么没结论");
        }
    }

    #[test]
    fn 通过也要有理由() {
        let v = parse("{\"approve\": true}");
        assert!(v.approve);
        assert!(!v.reasons.is_empty(), "只有「同意」两个字等于没审");
    }

    #[test]
    fn 理由里不会漏_key() {
        let v = parse(
            "{\"approve\": false, \"reasons\": [\"用 hunt_tools_abcdefghijklmnopqrstuvwxyz012345 这把 key\"]}",
        );
        assert!(!v.reasons[0].contains("abcdefghij"), "{:?}", v.reasons);
    }

    /// 网关不通时是「没通过」，**不是**「默认通过」。
    #[test]
    fn 网关不通时不默认放行() {
        let v = Verdict::unavailable("连不上网关");
        assert!(!v.approve);
        assert!(v.one_line().contains("否决"), "{}", v.one_line());
    }

    /// 提示词里那几条判定尺度一条都不能少。
    #[test]
    fn 提示词里写死了判定尺度() {
        for must in [
            "删数据卷",
            "拿不准的时候**否决**",
            "证据里找不到依据",
            "approve",
        ] {
            assert!(SYSTEM_PROMPT.contains(must), "提示词里缺：{must}");
        }
        // 复核员**不该**拿到工具表 —— 提示词里也不提工具
        assert!(!SYSTEM_PROMPT.contains("工具表"), "复核员不该动手");
    }

    /// 复核员那一份提示词和诊断员那一份**不是同一段**。
    #[test]
    fn 与诊断员的提示词互不相同() {
        assert_ne!(SYSTEM_PROMPT, super::super::auto::AUTO_SYSTEM_PROMPT);
        assert!(SYSTEM_PROMPT.contains("复核员"));
        assert!(!SYSTEM_PROMPT.contains("诊断员。"));
    }

    /// **复核通过 ≠ 可以执行。** 守卫那一道照样把越界动作拦住。
    #[test]
    fn 复核通过不等于可以执行() {
        // 假设复核员放行了一个表外动作
        let v = Verdict {
            approve: true,
            reasons: vec!["我觉得没问题".into()],
            tokens: 0,
            elapsed_ms: 0,
            degraded: None,
        };
        assert!(v.approve);
        // 守卫仍然拒绝 —— 这一道和模型说什么毫无关系
        assert!(actions::plan(&Call::new("rm_user_documents")).is_err());
        // Sensitive 动作即使复核通过，档位判定照样要求用户确认
        let sp = actions::spec("install_runtime").expect("表里有");
        assert_eq!(sp.level, Level::Sensitive);
        for m in [
            super::super::guard::Mode::Auto,
            super::super::guard::Mode::Confirm,
        ] {
            assert!(m.needs_confirm(sp.level), "{m:?} 下 Sensitive 仍要问");
        }
    }

    #[test]
    fn 计划描述里写的就是要执行的那条命令() {
        let s = describe(&[Call::new("compose_down_own")]);
        assert!(s.contains("compose"), "{s}");
        assert!(s.contains("风险级别"), "{s}");
        assert!(s.contains("为什么"), "{s}");
    }
}
