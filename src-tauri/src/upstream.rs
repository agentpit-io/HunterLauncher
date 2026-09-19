//! 从**本机 api 容器**读运行面板要的那几个数字。
//!
//! ## 先说清楚哪些读得到、哪些读不到
//!
//! 视觉稿第 3 张的四张卡片要：今日模型额度、今日对话（含深度分析）、晨报、数据源。
//! M3 开工时把 api 容器上的接口逐条探了一遍（记在成果文档），结论是：
//!
//! | 卡片 | 有没有接口 | 本轮怎么做 |
//! |---|---|---|
//! | 今日模型额度 | ✅ 网关 `GET /api/saas/llm/quota` | 真数字（M2 就通了） |
//! | 数据源 | ✅ `GET /api/setup/status` 的 `llm.source` + `data_supply.configured` | 真数字（本轮接上） |
//! | 今日对话 / 深度分析 | ❌ | 显示 `—` + 原因；接口清单写进「需上游配合」 |
//! | 晨报 | ❌ | 同上 |
//!
//! 实测（2026-09-20，测试机 api 1.2.0）：`/api/chat/sessions`、`/api/signals`、
//! `/api/preference`、`/api/user_sources`、`/api/system/metrics/daily` 全部 **401**
//! —— 它们要一个登录用户的 JWT，启动器手里没有、也**不应该**替用户造一个。
//! 不登录能读的只有 `/api/health`、`/api/setup/status`、`/api/setup/llm/quota`、
//! `/api/setup/env-check` 这四条。
//!
//! 拿不到就是拿不到，显示 `—` 并写明原因（红线 1）。
//!
//! ## 一个安全注意点
//!
//! `/api/setup/status` 的响应里**带着打码后的 key**（`hunt_tools_q6sK****QMo2` 这种形状）。
//! 打码归打码，它仍然露了 4 位随机字符，所以这个模块**只取自己要的几个字段**，
//! 绝不把整个响应体塞进日志或诊断包。

use std::time::Duration;

use serde::Serialize;

/// 从 api 读回来的「面板要用的那几项」。每一项都可能是 `None` —— 拿不到就是 `None`。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpstreamFacts {
    /// api 活着没有
    pub reachable: bool,
    /// key 有没有真的落进容器（`/api/health` 的 `hunter_api_key`）
    pub api_key_configured: Option<bool>,
    /// 大模型配置的来源：`env` / `db` / `none`
    pub llm_source: Option<String>,
    /// 当前生效的模型别名
    pub llm_model: Option<String>,
    /// 走不走内置额度（走 = 数据源是 Hunter 网关）
    pub builtin_quota: Option<bool>,
    /// 平台 key（数据供给）配好了没有
    pub data_supply_configured: Option<bool>,
    /// 读不到时的原因，直接显示给用户
    pub reason: Option<String>,
}

/// 上游**确实没有**、界面因此只能显示 `—` 的几项。
/// 前端按 id 显示对应的「上游暂无此接口」说明；成果文档「需上游配合」一节也列同一串。
pub const MISSING_ENDPOINTS: [(&str, &str); 4] = [
    (
        "conversations",
        "GET /api/system/metrics/daily（今日对话数、深度分析次数）",
    ),
    (
        "morning_brief",
        "GET /api/system/metrics/daily 或 /api/signal_settings 的免登录只读版（晨报开关与推送时间）",
    ),
    (
        "user_sources",
        "GET /api/user_sources 的免登录只读计数（自有数据源个数）",
    ),
    (
        "last_tool_call",
        "GET /api/system/last-tool-call（反馈诊断里的最近一次工具调用摘要）",
    ),
];

/// 打一次本机 api。**只连 127.0.0.1**，超时短 —— 面板每几秒刷一次，不能卡住。
pub fn fetch(api_port: u16, timeout: Duration) -> UpstreamFacts {
    let mut f = UpstreamFacts::default();

    match crate::http::get(
        &format!("http://127.0.0.1:{api_port}/api/health"),
        &[],
        timeout,
    ) {
        Ok(r) if r.ok() => {
            f.reachable = true;
            f.api_key_configured = r.json().and_then(|v| {
                v.get("hunter_api_key")
                    .and_then(|x| x.as_str())
                    .map(|s| s != "missing")
            });
        }
        Ok(r) => {
            f.reason = Some(format!("api /api/health 返回 HTTP {}", r.status));
            return f;
        }
        Err(e) => {
            f.reason = Some(e.msg);
            return f;
        }
    }

    // `/api/setup/status` 的放行规则（上游 `routers/setup.py::_guard`）是
    // 「**setup 口令没配置** 且 来源判为本机/内网」。启动器写 `.env` 时把
    // `HUNTER_SETUP_TOKEN` 留空（M0 §3.3 的结论：不照抄 `.env.example` 里那个公开值），
    // 所以默认情况下这条走得通 —— 实测 `via: "local"`。
    //
    // 但用户**手工给 `.env` 填了 setup 口令**时这里就会 401。那不是错误，
    // 是他自己把这道门锁上了：如实把原因写进 `reason`，界面显示「—」。
    match crate::http::get(
        &format!("http://127.0.0.1:{api_port}/api/setup/status"),
        &[],
        timeout,
    ) {
        Ok(r) if r.ok() => {
            if let Some(v) = r.json() {
                // ⚠ 只挑这几个字段。整个响应体里有打码 key，不许往别处传。
                f.llm_source = v
                    .pointer("/llm/source")
                    .and_then(|x| x.as_str())
                    .map(str::to_string);
                f.llm_model = v
                    .pointer("/llm/model")
                    .and_then(|x| x.as_str())
                    .filter(|s| !s.is_empty())
                    .map(str::to_string);
                f.builtin_quota = v.pointer("/llm/builtin").and_then(|x| x.as_bool());
                f.data_supply_configured = v
                    .pointer("/data_supply/configured")
                    .and_then(|x| x.as_bool());
            }
        }
        Ok(r) if r.status == 401 => {
            f.reason = Some("api /api/setup/status 要 setup 口令（HTTP 401）".into());
        }
        Ok(r) => {
            f.reason = Some(format!("api /api/setup/status 返回 HTTP {}", r.status));
        }
        Err(e) => f.reason = Some(e.msg),
    }
    f
}

/// 「数据源」那张卡上显示什么。返回 (主值, 副行)；拿不到主值就返回 `None`。
pub fn data_source_label(f: &UpstreamFacts) -> (Option<String>, String) {
    if !f.reachable {
        return (
            None,
            f.reason.clone().unwrap_or_else(|| "api 连不上".into()),
        );
    }
    let main = match (f.builtin_quota, f.llm_source.as_deref()) {
        (Some(true), _) => Some("Hunter 网关".to_string()),
        (Some(false), Some("none")) | (None, Some("none")) => Some("尚未配置".to_string()),
        (Some(false), _) => Some("自带模型".to_string()),
        (None, _) => None,
    };
    // 「自有数据源 N 个」那一行上游没有免登录接口，如实写清楚而不是写个 0
    let sub = match f.data_supply_configured {
        Some(true) => "平台数据供给已配置 · 自有数据源个数需登录后才能读".to_string(),
        Some(false) => "平台数据供给未配置".to_string(),
        None => f
            .reason
            .clone()
            .unwrap_or_else(|| "读不到 /api/setup/status".into()),
    };
    (main, sub)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 连不上的时候主值是_none_并且带原因() {
        let f = UpstreamFacts {
            reachable: false,
            reason: Some("Connection refused".into()),
            ..Default::default()
        };
        let (main, sub) = data_source_label(&f);
        assert!(main.is_none());
        assert!(sub.contains("Connection refused"));
    }

    #[test]
    fn 内置额度为真时数据源显示网关() {
        let f = UpstreamFacts {
            reachable: true,
            builtin_quota: Some(true),
            llm_source: Some("env".into()),
            data_supply_configured: Some(true),
            ..Default::default()
        };
        let (main, sub) = data_source_label(&f);
        assert_eq!(main.as_deref(), Some("Hunter 网关"));
        // 副行不能编一个「自有数据源 0 个」出来
        assert!(!sub.contains("0 个"), "{sub}");
        assert!(sub.contains("需登录"), "{sub}");
    }

    #[test]
    fn 自带模型与未配置分得开() {
        let own = UpstreamFacts {
            reachable: true,
            builtin_quota: Some(false),
            llm_source: Some("db".into()),
            ..Default::default()
        };
        assert_eq!(data_source_label(&own).0.as_deref(), Some("自带模型"));

        let none = UpstreamFacts {
            reachable: true,
            builtin_quota: Some(false),
            llm_source: Some("none".into()),
            ..Default::default()
        };
        assert_eq!(data_source_label(&none).0.as_deref(), Some("尚未配置"));
    }

    #[test]
    fn 缺的接口清单不为空且都写了路径() {
        assert_eq!(MISSING_ENDPOINTS.len(), 4);
        for (id, desc) in MISSING_ENDPOINTS {
            assert!(!id.is_empty());
            assert!(desc.contains("/api/"), "要写清楚缺的是哪条接口：{desc}");
        }
    }
}
