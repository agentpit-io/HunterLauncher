//! 模型网关与 key 校验（技术方案 §5.4、§8，按 M0 §1 实测全面修正）。
//!
//! 方案写的 `POST https://llm.agentpit.io/v1/keys/validate` **不存在**。真实接口是：
//!
//! | 用途 | 真实接口 |
//! |---|---|
//! | 额度 | `GET https://hunter.agentpit.io/api/saas/llm/quota` |
//! | 模型列表 | `GET https://hunter.agentpit.io/api/saas/llm/v1/models`（OpenAI 兼容） |
//!
//! key 前缀 `hunt_tools_`，长度 43。模型别名是 `hunter-chat` / `hunter-deep`。
//!
//! **关于「区分无效原因」**：方案 §18 要求把「格式错 / 不存在 / 已吊销 / 未带 key」分开给文案，
//! M0 §1.3 实测这四种网关返回**字节级完全相同**的 401，网关侧做不到。
//! 所以这里只做两件事：本地判格式（不发请求），以及把 401 统一归成「网关不认」。
//! **不假装能区分「已吊销」** —— 那会是编造的诊断结论（红线 1）。

use std::time::Duration;

use serde::Serialize;

use crate::err::{AppError, AppResult, Code};

pub const GATEWAY_BASE: &str = "https://hunter.agentpit.io/api/saas/llm";
pub const LLM_BASE_URL: &str = "https://hunter.agentpit.io/api/saas/llm/v1";
pub const APPLY_URL: &str = "https://hunter.agentpit.io/dev/api-keys";
pub const DEFAULT_MODEL: &str = "hunter-chat";
pub const KEY_PREFIX: &str = "hunt_tools_";
pub const KEY_LEN: usize = 43;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaInfo {
    pub used_today: i64,
    pub limit_daily: i64,
    pub remaining: i64,
    pub reset_at: Option<String>,
    pub rpm: Option<i64>,
    pub concurrency: Option<i64>,
    /// 今天的额度是不是已经用完了。
    ///
    /// **这一项只能从 `/quota` 的响应里读，不能靠 `check_key` 收到 402 来判** ——
    /// I1 实测：额度压到 1 token 之后 `GET /quota` 仍然回 **HTTP 200**
    /// （体里 `exhausted: true`、`remaining: 0`），402 只会出现在
    /// `POST /v1/chat/completions` 上。也就是说 [`check_key`] 永远见不到 402。
    pub exhausted: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelAlias {
    pub id: String,
    pub purpose: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyReason {
    Ok,
    /// 本地就判得出来的格式问题，**不发请求**
    Malformed,
    /// 网关回了 401：可能填错了，也可能被吊销了 —— 网关分不出（M0 §1.3）
    Rejected,
    /// 额度用尽（HTTP 402）。key 本身是好的
    Exhausted,
    /// 网络层没通（DNS、超时、代理拦截）
    Network,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyCheckResult {
    pub valid: bool,
    pub reason: KeyReason,
    pub quota: Option<QuotaInfo>,
    pub models: Vec<ModelAlias>,
    /// 给用户看的中文说明。valid 时为 None
    pub message: Option<String>,
    /// 对应的错误码，界面与 headless 共用
    pub code: Option<String>,
}

/// 本地格式判断（M0 §1.1：前缀 11 位 + 随机 32 位 = 43）。
/// 格式不对就**不发请求** —— 既省一次往返，也避免把一串乱码送到网关去。
pub fn key_shape_ok(key: &str) -> bool {
    key.len() == KEY_LEN
        && key.starts_with(KEY_PREFIX)
        && key[KEY_PREFIX.len()..]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// 完整校验：格式 → quota → models。
pub fn check_key(key: &str, timeout: Duration) -> KeyCheckResult {
    if !key_shape_ok(key) {
        return KeyCheckResult {
            valid: false,
            reason: KeyReason::Malformed,
            quota: None,
            models: Vec::new(),
            message: Some(format!(
                "key 的格式不对。应该是 {KEY_PREFIX} 开头、一共 {KEY_LEN} 位的一串字符（你填的是 {} 位）。到 {APPLY_URL} 免费申请一把。",
                key.chars().count()
            )),
            code: Some(Code::KeyInvalid.as_str().to_string()),
        };
    }
    crate::redact::register_secret(key);

    let auth = format!("Bearer {key}");
    let quota_resp = crate::http::get(
        &format!("{GATEWAY_BASE}/quota"),
        &[("Authorization", &auth)],
        timeout,
    );

    let resp = match quota_resp {
        Err(e) => {
            return KeyCheckResult {
                valid: false,
                reason: KeyReason::Network,
                quota: None,
                models: Vec::new(),
                message: Some(format!(
                    "连不上 Hunter 网关（{}）。检查一下网络或代理设置；用 Clash 之类代理时把 hunter.agentpit.io 放行。",
                    e.msg
                )),
                code: Some(Code::ProxyBlock.as_str().to_string()),
            };
        }
        Ok(r) => r,
    };

    match resp.status {
        200 => {}
        401 => {
            return KeyCheckResult {
                valid: false,
                reason: KeyReason::Rejected,
                quota: None,
                models: Vec::new(),
                // M0 §1.3：网关对「填错了」和「已吊销」返回一模一样的 401，这里不编
                message: Some(format!(
                    "这把 key 网关不认（可能填错了，也可能已经被吊销）。到 {APPLY_URL} 可以免费再申请一把（约 30 秒）。"
                )),
                code: Some(Code::KeyInvalid.as_str().to_string()),
            };
        }
        402 => {
            let q = resp.json().and_then(|v| parse_quota(&v));
            return KeyCheckResult {
                valid: false,
                reason: KeyReason::Exhausted,
                quota: q,
                models: Vec::new(),
                message: Some("今天的免费额度已经用完了。等明天 0 点（上海时间）重置，或者在下一步改用你自己的模型 key。".to_string()),
                code: Some(Code::QuotaExhausted.as_str().to_string()),
            };
        }
        s => {
            return KeyCheckResult {
                valid: false,
                reason: KeyReason::Network,
                quota: None,
                models: Vec::new(),
                message: Some(format!("网关返回了 HTTP {s}：{}", first_line(&resp.body))),
                code: Some(Code::Unknown.as_str().to_string()),
            };
        }
    }

    let quota = resp.json().and_then(|v| parse_quota(&v));
    let models = fetch_models(&auth, timeout).unwrap_or_default();

    // 额度用完了的 key **仍然是一把好 key**：明天 0 点（上海）就会重置，
    // 而且下一步就能改用自带模型 key。所以 `valid` 还是 true，装照装 ——
    // 但必须把话说在前面，不能让用户装完之后在 Hunter 里撞一鼻子灰
    // （I1 之前就是这样：`/quota` 回 200，校验直接过，一个字都不提）。
    if quota.as_ref().is_some_and(|q| q.exhausted) {
        let q = quota.as_ref().expect("上面刚判过");
        return KeyCheckResult {
            valid: true,
            reason: KeyReason::Exhausted,
            message: Some(format!(
                "这把 key 是好的，但今天的额度已经用完了（已用 {} / 上限 {}）。{}装可以照装，\
                 装完之后对话会被网关挡住，直到额度重置。现在就想用的话，下一步改用你自己的模型 key。",
                q.used_today,
                q.limit_daily,
                match q.reset_at.as_deref().map(pretty_reset) {
                    Some(t) => format!("额度在 {t} 重置。"),
                    None => String::new(),
                }
            )),
            code: Some(Code::QuotaExhausted.as_str().to_string()),
            quota,
            models,
        };
    }

    KeyCheckResult {
        valid: true,
        reason: KeyReason::Ok,
        quota,
        models,
        message: None,
        code: None,
    }
}

/// 只查额度（运行面板每次刷新用）。
pub fn quota(key: &str, timeout: Duration) -> AppResult<QuotaInfo> {
    let auth = format!("Bearer {key}");
    let r = crate::http::get(
        &format!("{GATEWAY_BASE}/quota"),
        &[("Authorization", &auth)],
        timeout,
    )?;
    if r.status == 401 {
        return Err(AppError::new(
            Code::KeyInvalid,
            "网关不认这把 key".to_string(),
        ));
    }
    if !r.ok() && r.status != 402 {
        return Err(AppError::new(
            Code::Unknown,
            format!("额度接口返回 HTTP {}", r.status),
        ));
    }
    r.json()
        .and_then(|v| parse_quota(&v))
        .ok_or_else(|| AppError::new(Code::Unknown, "额度响应里没有认得的字段".to_string()))
}

/// 网关给的是 ISO 8601（`2026-09-21T00:00:00+08:00`），直接上屏太长也不像话。
/// 网关自己就说了是上海时间（`reset_tz: Asia/Shanghai`），所以只要把 `T` 换成空格、
/// 砍掉秒与时区，再补一句「（上海）」。**不做时区换算** —— 那需要一个时区库，
/// 而这里唯一要传达的信息是「明天 0 点」。认不出来的格式原样返回，不猜。
fn pretty_reset(iso: &str) -> String {
    let (date, rest) = match iso.split_once('T') {
        Some(x) => x,
        None => return iso.to_string(),
    };
    let hm: String = rest.chars().take(5).collect();
    if date.len() != 10 || hm.len() != 5 || !hm.contains(':') {
        return iso.to_string();
    }
    format!("{date} {hm}（上海）")
}

pub fn parse_quota(v: &serde_json::Value) -> Option<QuotaInfo> {
    let used = v.get("used_today")?.as_i64().unwrap_or(0);
    let limit = v.get("limit_daily").and_then(|x| x.as_i64()).unwrap_or(0);
    let remaining = v
        .get("remaining")
        .and_then(|x| x.as_i64())
        .unwrap_or(limit - used);
    Some(QuotaInfo {
        used_today: used,
        limit_daily: limit,
        remaining,
        reset_at: v
            .get("reset_at")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string()),
        rpm: v.get("rate_per_min").and_then(|x| x.as_i64()),
        concurrency: v.get("max_concurrency").and_then(|x| x.as_i64()),
        // 网关自己给的结论优先；没有这个字段时按 remaining 兜底
        exhausted: v
            .get("exhausted")
            .and_then(|x| x.as_bool())
            .unwrap_or(limit > 0 && remaining <= 0),
    })
}

fn fetch_models(auth: &str, timeout: Duration) -> Option<Vec<ModelAlias>> {
    let r = crate::http::get(
        &format!("{LLM_BASE_URL}/models"),
        &[("Authorization", auth)],
        timeout,
    )
    .ok()?;
    if !r.ok() {
        return None;
    }
    Some(parse_models(&r.json()?))
}

pub fn parse_models(v: &serde_json::Value) -> Vec<ModelAlias> {
    v.get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    let id = m.get("id")?.as_str()?.to_string();
                    let purpose = m
                        .get("description")
                        .and_then(|x| x.as_str())
                        .or_else(|| m.get("display_name").and_then(|x| x.as_str()))
                        .unwrap_or("")
                        .to_string();
                    Some(ModelAlias { id, purpose })
                })
                .collect()
        })
        .unwrap_or_default()
}

// ── 自带 key 模式 ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnKeyCheck {
    pub ok: bool,
    /// 真跑通的那条路径：`models` 或 `chat`
    pub via: Option<String>,
    pub message: String,
    /// 是不是自动开了 LLM_SCHEMA_SANITIZE
    pub schema_sanitize: bool,
    /// `/v1/models` 能列出来时，顺手把模型名列回去
    pub models: Vec<String>,
}

/// DeepSeek 的 OpenAI 兼容接口对 JSON Schema 的支持有出入，要开 `LLM_SCHEMA_SANITIZE=1`
/// （方案 §5.4 明确要求）。判断依据只看 BASE_URL 的主机名，不猜模型名。
pub fn needs_schema_sanitize(base_url: &str) -> bool {
    crate::http::host_of(base_url)
        .to_ascii_lowercase()
        .contains("deepseek")
}

/// 自带 key 模式的**真实连通性检查**（里程碑要求 3）。
///
/// 先试 `GET {base}/models`；有些兼容服务没开这个接口，再退而求其次发一次
/// 最小的 `POST {base}/chat/completions`（`max_tokens: 1`）。两条都不通才算失败。
/// 真发了一次请求，所以结论是真的 —— 不做「格式看起来对就算通过」那种假检查。
pub fn check_own_key(base_url: &str, model: &str, api_key: &str, timeout: Duration) -> OwnKeyCheck {
    let sanitize = needs_schema_sanitize(base_url);
    let base = base_url.trim_end_matches('/');
    if base.is_empty() || model.trim().is_empty() || api_key.trim().is_empty() {
        return OwnKeyCheck {
            ok: false,
            via: None,
            message: "BASE_URL、模型名、API Key 三项都要填。".to_string(),
            schema_sanitize: sanitize,
            models: Vec::new(),
        };
    }
    if !base.starts_with("https://") && !base.starts_with("http://") {
        return OwnKeyCheck {
            ok: false,
            via: None,
            message: "BASE_URL 要以 https:// 开头（少数自建服务可能是 http://）。".to_string(),
            schema_sanitize: sanitize,
            models: Vec::new(),
        };
    }
    crate::redact::register_secret(api_key);
    let auth = format!("Bearer {api_key}");

    // ① GET /models
    match crate::http::get(
        &format!("{base}/models"),
        &[("Authorization", &auth)],
        timeout,
    ) {
        Ok(r) if r.ok() => {
            let models: Vec<String> = r
                .json()
                .map(|v| parse_models(&v).into_iter().map(|m| m.id).collect())
                .unwrap_or_default();
            let hit = models.iter().any(|m| m == model);
            let message = if models.is_empty() {
                format!(
                    "连通。{} 接受了这把 key（/models 返回 200）。",
                    crate::http::host_of(base)
                )
            } else if hit {
                format!(
                    "连通，而且模型 {model} 就在列表里（共 {} 个）。",
                    models.len()
                )
            } else {
                format!(
                    "连通，但 /models 列出的 {} 个模型里没有 {model}。名字可能写错了 —— 仍然可以继续，真用起来才知道。",
                    models.len()
                )
            };
            return OwnKeyCheck {
                ok: true,
                via: Some("models".into()),
                message,
                schema_sanitize: sanitize,
                models,
            };
        }
        Ok(r) if r.status == 401 || r.status == 403 => {
            return OwnKeyCheck {
                ok: false,
                via: None,
                message: format!(
                    "{} 拒绝了这把 key（HTTP {}）。",
                    crate::http::host_of(base),
                    r.status
                ),
                schema_sanitize: sanitize,
                models: Vec::new(),
            };
        }
        Ok(_) => {} // 404 之类 → 试第二条路
        Err(e) => {
            return OwnKeyCheck {
                ok: false,
                via: None,
                message: format!("连不上 {}：{}", crate::http::host_of(base), e.msg),
                schema_sanitize: sanitize,
                models: Vec::new(),
            };
        }
    }

    // ② POST /chat/completions，最小请求
    let body = serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": "hi"}],
        "max_tokens": 1,
    });
    match crate::http::post_json(
        &format!("{base}/chat/completions"),
        &[("Authorization", &auth)],
        &body,
        timeout,
    ) {
        Ok(r) if r.ok() => OwnKeyCheck {
            ok: true,
            via: Some("chat".into()),
            message: format!(
                "连通。{} 上的 {model} 真的回了一次（这一次消耗了你自己额度里的几个 token）。",
                crate::http::host_of(base)
            ),
            schema_sanitize: sanitize,
            models: Vec::new(),
        },
        Ok(r) => OwnKeyCheck {
            ok: false,
            via: None,
            message: format!(
                "{} 返回 HTTP {}：{}",
                crate::http::host_of(base),
                r.status,
                first_line(&r.body)
            ),
            schema_sanitize: sanitize,
            models: Vec::new(),
        },
        Err(e) => OwnKeyCheck {
            ok: false,
            via: None,
            message: format!("连不上 {}：{}", crate::http::host_of(base), e.msg),
            schema_sanitize: sanitize,
            models: Vec::new(),
        },
    }
}

fn first_line(body: &str) -> String {
    let b = body.trim();
    // 网关的错误体是 {"error":{"message":"…"}}，直接把 message 拎出来最好读
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(b) {
        if let Some(m) = v
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
        {
            return truncate(m, 200);
        }
        if let Some(m) = v.get("message").and_then(|m| m.as_str()) {
            return truncate(m, 200);
        }
    }
    truncate(b.lines().next().unwrap_or(""), 200)
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_格式判断() {
        // 43 位、hunt_tools_ 开头（编造的假 key）
        let good = "hunt_tools_q6sKaaaaaaaaaaaaaaaaaaaaaaaaQMo2";
        assert_eq!(good.len(), 43);
        assert!(key_shape_ok(good));

        assert!(!key_shape_ok(""), "空串");
        assert!(!key_shape_ok("sk-totally-wrong-format-123"), "别家的前缀");
        assert!(!key_shape_ok("hunt_tools_short"), "长度不够");
        assert!(!key_shape_ok(&format!("{good}x")), "多一位");
        assert!(
            !key_shape_ok(
                &"hunt_tools_"
                    .chars()
                    .chain(std::iter::repeat('中').take(11))
                    .collect::<String>()
            ),
            "非 ASCII"
        );
    }

    #[test]
    fn 格式错时不发请求并给出位数() {
        let r = check_key("sk-wrong", Duration::from_secs(1));
        assert!(!r.valid);
        assert_eq!(r.reason, KeyReason::Malformed);
        assert_eq!(r.code.as_deref(), Some("E_KEY_INVALID"));
        let m = r.message.unwrap();
        assert!(m.contains("hunt_tools_"), "{m}");
        assert!(m.contains("8 位"), "要告诉用户他填了几位：{m}");
    }

    #[test]
    fn 额度响应解析用的是_m0_实测的真实响应体() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"ok":true,"used_today":115,"limit_daily":300000,"remaining":299885,
                "reset_at":"2026-09-20T00:00:00+08:00","reset_tz":"Asia/Shanghai",
                "rate_per_min":20,"max_concurrency":4,"unit":"token","enabled":true,
                "exhausted":false,"global_exhausted":false}"#,
        )
        .unwrap();
        let q = parse_quota(&v).unwrap();
        assert_eq!(q.used_today, 115);
        assert_eq!(q.limit_daily, 300_000);
        assert_eq!(q.remaining, 299_885);
        assert_eq!(q.reset_at.as_deref(), Some("2026-09-20T00:00:00+08:00"));
        assert_eq!(q.rpm, Some(20));
        assert_eq!(q.concurrency, Some(4));
        assert!(!q.exhausted);
    }

    #[test]
    fn 额度用尽时_quota_接口回的仍然是_200() {
        // I1 实测：把测试 key 的日额度压到 1 token 之后，`GET /quota` **不是 402**，
        // 而是 200 + `exhausted: true`。402 只出现在 `POST /v1/chat/completions` 上。
        // 下面这一段是当时真实抓到的响应体，一个字没改。
        let v: serde_json::Value = serde_json::from_str(
            r#"{"ok":true,"used_today":170206,"limit_daily":1,"remaining":0,"exhausted":true,
                "reset_at":"2026-09-21T00:00:00+08:00","reset_tz":"Asia/Shanghai",
                "rate_per_min":20,"max_concurrency":4,
                "unit":"token（输入 + 含 thinking 的输出）","enabled":true,"global_exhausted":false}"#,
        )
        .unwrap();
        let q = parse_quota(&v).unwrap();
        assert!(q.exhausted, "额度用尽必须认出来，否则装完才发现对话被拒");
        assert_eq!(q.remaining, 0);
        assert_eq!(q.limit_daily, 1);
    }

    #[test]
    fn 重置时间上屏前会被整理成人话() {
        assert_eq!(
            pretty_reset("2026-09-21T00:00:00+08:00"),
            "2026-09-21 00:00（上海）"
        );
        // 认不出来的格式原样返回，不猜
        assert_eq!(pretty_reset("明天"), "明天");
        assert_eq!(pretty_reset("2026-09-21"), "2026-09-21");
        assert_eq!(pretty_reset("2026-09-21T0"), "2026-09-21T0");
    }

    #[test]
    fn 网关没给_exhausted_字段时按剩余量兜底() {
        let v: serde_json::Value =
            serde_json::from_str(r#"{"used_today":300000,"limit_daily":300000,"remaining":0}"#)
                .unwrap();
        assert!(parse_quota(&v).unwrap().exhausted);

        let v2: serde_json::Value =
            serde_json::from_str(r#"{"used_today":1,"limit_daily":300000,"remaining":299999}"#)
                .unwrap();
        assert!(!parse_quota(&v2).unwrap().exhausted);

        // 上限读不到（0）时不能一口咬定「用完了」—— 那是「不知道」，不是「用完」
        let v3: serde_json::Value = serde_json::from_str(r#"{"used_today":5}"#).unwrap();
        assert!(!parse_quota(&v3).unwrap().exhausted);
    }

    #[test]
    fn 模型列表解析用的是_m0_实测的真实响应体() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"object":"list","data":[
               {"id":"hunter-chat","object":"model","description":"默认对话与工具调用 · 快、便宜","display_name":"Gemini 3.8 Flash"},
               {"id":"hunter-deep","object":"model","description":"深度分析与长任务 · 更强但更贵","display_name":"Gemini 3.1 Pro"}]}"#,
        )
        .unwrap();
        let m = parse_models(&v);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].id, "hunter-chat", "不是方案里写的 hunter-default");
        assert_eq!(m[1].id, "hunter-deep");
        assert!(m[0].purpose.contains("对话"));
    }

    #[test]
    fn 网关的错误体能拎出_message() {
        let body = r#"{"error":{"message":"这个地址是 HunterCode 的内置模型额度网关，需要一把 Hunter 平台 key 才能用。","type":"invalid_api_key","code":"hunter_key_required"}}"#;
        assert!(first_line(body).starts_with("这个地址是 HunterCode"));
        // 数据网关是扁平结构（M0 §1.4 的注），也要能吃
        assert_eq!(
            first_line(r#"{"error":"hunter_key_required","message":"不在白名单里"}"#),
            "不在白名单里"
        );
        assert_eq!(first_line("plain text\nsecond line"), "plain text");
    }

    #[test]
    fn deepseek_自动开_schema_sanitize() {
        assert!(needs_schema_sanitize("https://api.deepseek.com/v1"));
        assert!(needs_schema_sanitize("https://API.DeepSeek.com/v1/"));
        assert!(!needs_schema_sanitize("https://api.openai.com/v1"));
        assert!(!needs_schema_sanitize(
            "https://dashscope.aliyuncs.com/compatible-mode/v1"
        ));
        // 只看主机名：路径里带 deepseek 不算
        assert!(!needs_schema_sanitize("https://gw.example.com/deepseek/v1"));
    }

    #[test]
    fn 自带_key_三项没填全时不发请求() {
        let r = check_own_key("", "", "", Duration::from_millis(1));
        assert!(!r.ok);
        assert!(r.message.contains("三项都要填"));

        let r = check_own_key(
            "api.deepseek.com/v1",
            "deepseek-chat",
            "sk-x",
            Duration::from_millis(1),
        );
        assert!(!r.ok);
        assert!(r.message.contains("https://"), "{}", r.message);
        assert!(r.schema_sanitize, "deepseek 的判断不该受这个早退影响");
    }

    #[test]
    fn 常量与_m0_实测一致() {
        assert_eq!(LLM_BASE_URL, "https://hunter.agentpit.io/api/saas/llm/v1");
        assert_eq!(DEFAULT_MODEL, "hunter-chat");
        assert_eq!(KEY_PREFIX, "hunt_tools_");
        assert_eq!(KEY_LEN, 43);
    }
}
