//! **一键上传日志**（I16 · P0-2）。
//!
//! ## 为什么要有它
//!
//! 2026-09-25，客户在自己的 Mac 上点「升级 Hunter v1.2.0 → v1.2.2」，
//! 界面卡在「正在拉取新版本的镜像…」一个多小时不动，最后他把应用关了。
//! 我们想查，**手上一份日志都没有** —— 客户已经休息，导诊断包这条路走不通了。
//!
//! 原来的排障链条是：让用户导诊断包 → 他把 zip 发给我们 → 我们人工翻。
//! 三步里有两步要他在场。改成：**他点一下，脱敏后的日志直接进我们的库，
//! 他只要念一个 8 位追踪码给我们。**
//!
//! ## 服务端
//!
//! `POST https://www.agentpit.io/api/v1/launcher/logs`（不需要登录），
//! 200 回 `{"ok":true,"traceCode":"HL-7K3Q9F","truncated":false}`，
//! 429 是限流（同机器 5 次/小时、20 次/天），413 是请求体超过 2 MB。
//! 正文超 1 MB 服务端会**从头部截断**（线索在末尾）。
//!
//! ## 三条底线，写在代码里不靠自觉
//!
//! 1. **默认不传。** 只有用户亲手点了才发请求；设置里那个「出错时自动上传」
//!    默认关，打开之后也只在**出错**时传，正常流程一个字节都不出门
//!    （[`should_auto_upload`]）。
//! 2. **传之前给他看。** [`preview`] 返回的是**一模一样的那份正文**，
//!    界面上分节摆出来；看到的和传出去的是同一串字节，不存在两套。
//! 3. **key 一个字节都不传。** 总控规则红线 2 写的是「不写日志、不进遥测、
//!    不进诊断包」，上传日志就是一种上报。服务端那个可选的 `hunterKey`
//!    字段**我们不填**（详见 [`build_request`] 的注释）。
//!
//! 脱敏一律走现有的出口闸（[`crate::feedback::collect`] → [`crate::redact`]
//! → [`crate::redact::mask_names_here`] → [`crate::feedback::assert_clean_strict`]），
//! **没有新写一套**。

use std::time::Duration;

use serde::Serialize;

use crate::config::LauncherConfig;
use crate::err::{AppError, AppResult, Code};
use crate::feedback::{self, Section};
use crate::redact;

/// 上传地址。做成常量而不是配置项：这是「把日志交给我们」，
/// 不是一个用户该指向别处的东西；真要改，改的是代码与版本号。
pub const ENDPOINT: &str = "https://www.agentpit.io/api/v1/launcher/logs";

/// 客户端自己先截一次的上限。
///
/// 服务端超过 1 MB 会截、超过 2 MB 直接 413。我们截在 **900 KB**：
/// 留出 JSON 转义（中文在 JSON 里不会膨胀，但引号与换行会）与其余字段的余量，
/// 也免得把用户的上行带宽浪费在一段服务端反正要丢掉的文本上。
pub const BODY_LIMIT: usize = 900 * 1024;

/// 一次上传给多久。慢网上 900 KB 也要几十秒。
const TIMEOUT: Duration = Duration::from_secs(60);

// ── machineId ─────────────────────────────────────────────────────────────

/// 这台机器的随机标识，没有就现生成一个并写进 `launcher.toml`。
///
/// **不许掺任何硬件信息**（MAC、序列号、主机名）—— 那种东西一旦发出去
/// 就不再是「一个用来串起同一台机器几次上报的随机数」，而是一个跨应用可关联的指纹。
/// 用的是和 `install_id` 同一个生成器：系统 CSPRNG 出来的 UUID v4。
///
/// 会改 `cfg`，改了就返回 `true`，由调用方决定什么时候 `save` 与
/// `set_config`（配置在内存里有一份，直接落盘会让那一份变陈旧）。
pub fn ensure_machine_id(cfg: &mut LauncherConfig) -> bool {
    if !cfg.support.machine_id.trim().is_empty() {
        return false;
    }
    let id = crate::telemetry::new_install_id();
    if id.is_empty() {
        // 拿不到系统随机源时**不要**退回一个可预测的值（时间戳、主机名哈希…）。
        // 宁可这一次不带 machineId 上传，也不要送一个假装随机的指纹出去。
        crate::lwarn!("拿不到系统随机源，这次没能生成 machineId");
        return false;
    }
    crate::linfo!("第一次要用到 machineId，已生成一个随机 UUID 并写进 launcher.toml");
    cfg.support.machine_id = id;
    true
}

// ── 正文 ──────────────────────────────────────────────────────────────────

/// 上传前摆给用户看的那一份。**界面上显示的 `body` 就是将要送出去的字节**。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    /// 分节预览（和反馈页是同一套 [`Section`]，同一套脱敏）
    pub sections: Vec<Section>,
    /// 拼好、脱敏好、截断好的正文
    pub body: String,
    pub body_bytes: usize,
    /// 客户端这一侧截过没有
    pub truncated: bool,
    pub machine_id: String,
    pub endpoint: String,
    pub stage: String,
    pub error_code: String,
    pub summary: String,
    /// `meta` 里那几项，摆成人看得懂的行（界面上照样要给他看）
    pub meta_lines: Vec<String>,
    /// 出口闸的扫描结果。`None` = 干净；有值就**不给上传按钮**
    pub scan_hit: Option<String>,
    /// 设置里「出错时自动上传」现在开着没有
    pub auto_on_error: bool,
}

/// 把选中的几节拼成上传正文。
///
/// 分节之间用一行 `===` 隔开，每节带标题 —— 服务端存的是一整段文本，
/// 我们自己翻的时候要分得清哪一段是什么。
pub fn build_body(sections: &[Section], include: &[String]) -> String {
    let mut out = String::new();
    for s in sections {
        if !include.iter().any(|i| i == &s.id) {
            continue;
        }
        out.push_str(&format!("===== {} ({}) =====\n", s.title, s.id));
        if let Some(n) = &s.note {
            out.push_str(&format!("（{n}）\n"));
        }
        if !s.body.trim().is_empty() {
            out.push_str(s.body.trim_end());
            out.push('\n');
        }
        out.push('\n');
    }
    out
}

/// 超长时截断。**从头部截、保住末尾**：服务端也是这么做的，
/// 而拉取卡死这类问题的线索永远在最后几行。
///
/// 第一节（概要：版本、系统、Docker、端口）**整节保住不截** ——
/// 那几行只有几百字节，却是看任何一份日志的前提。
pub fn truncate_body(body: &str, limit: usize) -> (String, bool) {
    if body.len() <= limit {
        return (body.to_string(), false);
    }
    // 概要那一节到**第二节的标题**为止。
    // 正文一定以 `===== 概要 …` 开头（[`build_body`] 就是这么拼的），
    // 所以第二节的起点 = 第一个出现在行首的 `===== `
    let head_end = body
        .match_indices("\n===== ")
        .next()
        .map(|(i, _)| i + 1)
        .unwrap_or(0);
    let head = &body[..head_end.min(body.len())];
    let rest = &body[head_end.min(body.len())..];
    let keep = limit.saturating_sub(head.len() + 200);
    // 从末尾往回留 `keep` 个字节，落到一个字符边界上
    let mut cut = rest.len().saturating_sub(keep);
    while cut < rest.len() && !rest.is_char_boundary(cut) {
        cut += 1;
    }
    let dropped = cut;
    let out = format!(
        "{head}（中间 {dropped} 字节被启动器截掉了 —— 上传上限 {limit} 字节，线索通常在末尾，所以留的是后半截）\n{}",
        &rest[cut..]
    );
    (out, true)
}

/// 收一份可以上传的正文。**脱敏走的是现有那三道闸，没有新写一套。**
///
/// 1. [`feedback::collect`]：每一节出来时就已经过了 [`redact::redact`]
///    与 [`redact::mask_home`]（key、Bearer、邮箱、手机号、IP 末段、
///    `.env` 白名单之外的值、容器日志里的 content/text）；
/// 2. [`redact::mask_names_here`]：把用户名与主机名**换掉**（不是只发现）；
/// 3. [`feedback::assert_clean_strict`]：出门前整份再扫一遍，
///    发现 key 形状 / 完整邮箱 / 完整 IP / 用户名 / 主机名就**不给传**。
pub fn preview(
    cfg: &LauncherConfig,
    stage: &str,
    error_code: &str,
    summary: &str,
    include: Option<&[String]>,
) -> AppResult<Preview> {
    let mut sections = feedback::collect(cfg, env!("CARGO_PKG_VERSION"));
    for s in &mut sections {
        s.body = redact::mask_names_here(&s.body);
        if let Some(n) = &s.note {
            s.note = Some(redact::mask_names_here(n));
        }
    }
    let default_include: Vec<String> = sections
        .iter()
        .filter(|s| s.default_on)
        .map(|s| s.id.clone())
        .collect();
    let include: Vec<String> = include
        .map(|v| v.to_vec())
        .unwrap_or_else(|| default_include.clone());

    let raw = build_body(&sections, &include);
    let (body, truncated) = truncate_body(&raw, BODY_LIMIT);
    let scan_hit = feedback::assert_clean_strict(&body);
    if let Some(h) = &scan_hit {
        crate::lwarn!("要上传的日志正文没过出口闸，这一份不会被送出去：{h}");
    }
    Ok(Preview {
        body_bytes: body.len(),
        body,
        truncated,
        sections,
        machine_id: cfg.support.machine_id.clone(),
        endpoint: ENDPOINT.to_string(),
        stage: stage.to_string(),
        error_code: error_code.to_string(),
        summary: redact::mask_names_here(&redact::redact(summary)),
        meta_lines: meta_lines(cfg),
        scan_hit,
        auto_on_error: cfg.support.auto_on_error,
    })
}

// ── meta ──────────────────────────────────────────────────────────────────

/// `meta` 里带的那几项。**每一项都是实测值，读不到就不带这一项**（红线 1）。
fn meta(cfg: &LauncherConfig) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    m.insert(
        "registry".into(),
        serde_json::Value::String(format!(
            "{} ({})",
            cfg.hunter.registry_id, cfg.hunter.registry_prefix
        )),
    );
    m.insert(
        "proxy".into(),
        serde_json::Value::Bool(crate::netproxy::current().any()),
    );
    // `disk_free` 给的是 (剩余, 总量)
    if let Some((free, _total)) = crate::assist::probe::disk_free(&crate::paths::root()) {
        m.insert(
            "diskFreeGb".into(),
            serde_json::Value::from(free / 1_000_000_000),
        );
    }
    // 只有内置运行时那条路才有「虚拟机」可言；用户自己的 Docker 上这一项没有意义
    let st = crate::runtime::builtin::status();
    if st.installed {
        m.insert("vmRunning".into(), serde_json::Value::Bool(st.running));
    }
    serde_json::Value::Object(m)
}

/// `meta` 摆成人看得懂的行 —— 界面上「要传什么」必须**全部**看得见，
/// 不能只给正文、把一堆字段偷偷塞在 JSON 里。
fn meta_lines(cfg: &LauncherConfig) -> Vec<String> {
    let m = meta(cfg);
    let mut out = Vec::new();
    if let Some(o) = m.as_object() {
        for (k, v) in o {
            let shown = match v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            let label = match k.as_str() {
                "registry" => "镜像源",
                "proxy" => "这台机器配了网络代理",
                "diskFreeGb" => "磁盘剩余（GB）",
                "vmRunning" => "内置虚拟机在跑",
                other => other,
            };
            out.push(format!("{label}: {shown}"));
        }
    }
    out
}

// ── 请求 ──────────────────────────────────────────────────────────────────

/// 请求体。字段名照服务端的约定（camelCase）。
///
/// ## 为什么没有 `hunterKey`
///
/// 服务端接受一个可选的 `hunterKey`（原文，它只存 sha256 前 16 位，
/// 用来把同一个用户的多次上报串起来）。**我们不填它。**
///
/// 总控规则红线 2：「hunter key 只存 `~/.hunter/app/.env`（权限 600）与内存；
/// **不写日志、不进遥测**、不进诊断包、不进仓库和文档」。一键上传日志
/// 就是一次上报，key 的原文出现在请求体里就是破这条红线 ——
/// 而这个字段本来就是可选的，不填不影响任何功能。
///
/// 串同一个用户靠 `machineId`（随机 UUID，同一台机器稳定）。
/// 真要按 key 串，那是一个需要用户明确松口红线 2 的决定，不该由代码默默做掉。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Request {
    machine_id: String,
    launcher_ver: String,
    os: String,
    body: String,
    arch: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    hunter_ver: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    stage: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    error_code: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    summary: String,
    meta: serde_json::Value,
}

/// 造请求体。抽出来是为了单测能直接考它（字段齐不齐、有没有 key 明文）。
pub fn build_request(cfg: &LauncherConfig, p: &Preview) -> serde_json::Value {
    let r = Request {
        machine_id: p.machine_id.clone(),
        launcher_ver: env!("CARGO_PKG_VERSION").to_string(),
        os: os_name().to_string(),
        body: p.body.clone(),
        arch: std::env::consts::ARCH.to_string(),
        hunter_ver: cfg.hunter.tag.clone(),
        stage: p.stage.clone(),
        error_code: p.error_code.clone(),
        summary: p.summary.chars().take(200).collect(),
        meta: meta(cfg),
    };
    serde_json::to_value(&r).unwrap_or(serde_json::Value::Null)
}

/// 服务端认的 os 取值。Rust 的 `std::env::consts::OS` 在 macOS 上是 `macos`，
/// Windows 上是 `windows`，Linux 上是 `linux` —— 正好就是要的那三个。
fn os_name() -> &'static str {
    std::env::consts::OS
}

/// 一次上传的结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub ok: bool,
    /// 成功时的追踪码（形如 `HL-7K3Q9F`）
    pub trace_code: Option<String>,
    /// 服务端那一侧截过没有
    pub truncated: bool,
    /// 给用户看的一整句话。**失败时说人话，并且说清楚东西还在哪**
    pub message: String,
    /// HTTP 状态码。压根没连上时为 `None`
    pub status: Option<u16>,
    /// 传不上去时当场导出来的诊断包路径（发文件给我们也一样管用）
    pub bundle_path: Option<String>,
    /// 本机日志文件的路径，失败时要告诉用户它在哪
    pub local_log_path: String,
}

/// 真的把这一份传上去。
///
/// **失败不是只留一句错误**：网络不通时当场把诊断包导到本机并把路径给出来 ——
/// 用户还是能把那个文件发给我们，一条路断了得有第二条。
pub fn upload(cfg: &LauncherConfig, p: &Preview) -> AppResult<Outcome> {
    if let Some(h) = &p.scan_hit {
        return Err(AppError::new(
            Code::Unknown,
            format!("这份日志没过出口闸，已经取消上传：{h}"),
        ));
    }
    if p.machine_id.trim().is_empty() {
        return Err(AppError::new(
            Code::Unknown,
            "没有 machineId（拿不到系统随机源），这一次不上传".to_string(),
        ));
    }
    // **这一条不脱敏。** 它是给用户看的「你的日志在哪」——
    // 抹成 `<用户目录>/.hunter/logs/launcher.log` 他就找不到那个文件了。
    // 它不会被上传（`Outcome` 只回给界面），也不会进日志正文。
    let local_log_path = crate::paths::launcher_log().to_string_lossy().into_owned();
    let payload = build_request(cfg, p);
    crate::linfo!(
        "开始上传日志：{} 字节 · stage={} · code={}",
        p.body.len(),
        if p.stage.is_empty() { "—" } else { &p.stage },
        if p.error_code.is_empty() {
            "—"
        } else {
            &p.error_code
        }
    );

    let r = match crate::http::post_json(
        ENDPOINT,
        &[("Content-Type", "application/json")],
        &payload,
        TIMEOUT,
    ) {
        Ok(r) => r,
        Err(e) => {
            // 连都没连上。**当场导一个诊断包出来**，别让用户两手空空
            // 同样不脱敏：这是要让用户在自己的文件管理器里找到的那个文件
            let bundle = feedback::export_bundle()
                .ok()
                .map(|p| p.to_string_lossy().into_owned());
            crate::lwarn!("上传日志失败（没连上）：{}", e.msg);
            return Ok(Outcome {
                ok: false,
                trace_code: None,
                truncated: false,
                message: match &bundle {
                    Some(b) => format!(
                        "传不上去（{}）。已经给你把诊断包导到 {b}，把这个文件发给我们也一样管用。",
                        e.msg
                    ),
                    None => format!(
                        "传不上去（{}），诊断包也没导成。日志就在你本机：{local_log_path}",
                        e.msg
                    ),
                },
                status: None,
                bundle_path: bundle,
                local_log_path,
            });
        }
    };

    let v = r.json();
    let server_msg = v
        .as_ref()
        .and_then(|j| j.get("error").or_else(|| j.get("message")))
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();

    if r.status == 200 {
        let trace = v
            .as_ref()
            .and_then(|j| j.get("traceCode"))
            .and_then(|x| x.as_str())
            .map(str::to_string);
        let truncated = v
            .as_ref()
            .and_then(|j| j.get("truncated"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let ok = v
            .as_ref()
            .and_then(|j| j.get("ok"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        if let Some(code) = &trace {
            // **追踪码同时写进本地日志** —— 用户关掉窗口之后还能翻出来
            crate::linfo!("日志已上传，追踪码 {code}（把这个码发给我们就能查到这份日志）");
            return Ok(Outcome {
                ok: true,
                trace_code: Some(code.clone()),
                truncated: truncated || p.truncated,
                message: format!("已上传。追踪码 {code}，把它发给我们，我们就能查到这份日志。"),
                status: Some(200),
                bundle_path: None,
                local_log_path,
            });
        }
        // 200 但没有追踪码：**不谎称成功**（红线 1）
        crate::lwarn!("上传返回 200 但没有 traceCode，ok={ok}，原话：{server_msg}");
        return Ok(Outcome {
            ok: false,
            trace_code: None,
            truncated,
            message: format!(
                "服务端回了 200，但没给追踪码（{}）。这一份到底有没有收下说不准，日志还在你本机：{local_log_path}",
                if server_msg.is_empty() {
                    "没有说明".to_string()
                } else {
                    server_msg
                }
            ),
            status: Some(200),
            bundle_path: None,
            local_log_path,
        });
    }

    let (message, want_bundle) = failure_message(r.status, &server_msg, &local_log_path);
    let bundle = if want_bundle {
        feedback::export_bundle()
            .ok()
            .map(|p| p.to_string_lossy().into_owned())
    } else {
        None
    };
    crate::lwarn!("上传日志失败：HTTP {} · {}", r.status, server_msg);
    Ok(Outcome {
        ok: false,
        trace_code: None,
        truncated: false,
        message: match &bundle {
            Some(b) => format!("{message} 已经给你把诊断包导到 {b}，发给我们也一样。"),
            None => message,
        },
        status: Some(r.status),
        bundle_path: bundle,
        local_log_path,
    })
}

/// 非 200 时那一句话。**说人话，而且说清楚东西还在哪**。
///
/// 第二个返回值是「要不要顺手把诊断包导出来」：
/// 限流是「过会儿再来」，导包没有意义（日志本来就在他机器上，路径也给了）；
/// 服务端出错则是「这条路今天可能都走不通」，那就给他第二条路。
pub fn failure_message(status: u16, server_msg: &str, local_log_path: &str) -> (String, bool) {
    let tail = format!("日志还在你本机，路径是 {local_log_path}。");
    match status {
        429 => (
            format!(
                "今天传得有点多，服务端暂时不收了（同一台机器每小时最多 5 次、每天 20 次），过一个小时再点一次就行。{}{tail}",
                if server_msg.is_empty() {
                    String::new()
                } else {
                    format!("服务端原话：{server_msg}。")
                }
            ),
            false,
        ),
        413 => (
            format!("这一份日志太大，服务端不收（上限 2 MB）。{tail}"),
            true,
        ),
        400 | 422 => (
            format!(
                "服务端说这个请求它认不了（HTTP {status}{}）。{tail}",
                if server_msg.is_empty() {
                    String::new()
                } else {
                    format!("，原话：{server_msg}")
                }
            ),
            true,
        ),
        s if (500..600).contains(&s) => (
            format!(
                "我们这边的服务器出错了（HTTP {s}），不怪你。{tail}过一会儿再试一次，或者把诊断包发给我们。"
            ),
            true,
        ),
        s => (
            format!(
                "上传没成功（HTTP {s}{}）。{tail}",
                if server_msg.is_empty() {
                    String::new()
                } else {
                    format!("，原话：{server_msg}")
                }
            ),
            true,
        ),
    }
}

// ── 自动上传 ──────────────────────────────────────────────────────────────

/// **现在该不该自动传**。两个条件缺一不可：开关开着，而且**真的出错了**。
///
/// 这个函数存在的意义就是让「正常流程一个字节都不传」变成一条**代码上的**
/// 保证，而不是一句注释里的承诺 —— 调用方只有这一个入口，
/// 而它拿不到错误码时一律返回 false。
pub fn should_auto_upload(cfg: &LauncherConfig, error_code: Option<&str>) -> bool {
    cfg.support.auto_on_error && error_code.is_some_and(|c| !c.trim().is_empty())
}

/// 出错之后那一下自动上传。**开关关着就一个请求都不发**，而且立刻返回。
///
/// 返回追踪码（传成了的话）。传不成不往上抛错 ——
/// 「日志没传上去」不该盖住用户真正撞上的那个问题。
pub fn auto_upload(
    cfg: &LauncherConfig,
    stage: &str,
    error_code: &str,
    summary: &str,
) -> Option<String> {
    if !should_auto_upload(cfg, Some(error_code)) {
        return None;
    }
    crate::linfo!("「出错时自动上传日志」开着，这就把这一次的现场传上去");
    let p = match preview(cfg, stage, error_code, summary, None) {
        Ok(p) => p,
        Err(e) => {
            crate::lwarn!("自动上传：收集现场失败，这次不传（{}）", e.msg);
            return None;
        }
    };
    match upload(cfg, &p) {
        Ok(o) => {
            if !o.ok {
                crate::lwarn!("自动上传没成功：{}", o.message);
            }
            o.trace_code
        }
        Err(e) => {
            crate::lwarn!("自动上传失败：{}", e.msg);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with(machine: &str) -> LauncherConfig {
        let mut c = LauncherConfig::default();
        c.support.machine_id = machine.to_string();
        c.hunter.tag = "1.2.0".into();
        c
    }

    fn sec(id: &str, title: &str, body: &str) -> Section {
        Section {
            id: id.into(),
            title: title.into(),
            body: body.into(),
            default_on: true,
            note: None,
        }
    }

    #[test]
    fn machine_id_生成一次之后不再变() {
        let mut c = LauncherConfig::default();
        assert!(c.support.machine_id.is_empty());
        assert!(ensure_machine_id(&mut c), "第一次要生成");
        let first = c.support.machine_id.clone();
        assert_eq!(first.len(), 36, "UUID v4 的形状：{first}");
        // 再叫十次都不许变
        for _ in 0..10 {
            assert!(!ensure_machine_id(&mut c), "已经有了就不该再生成");
            assert_eq!(c.support.machine_id, first);
        }
    }

    #[test]
    fn machine_id_里不许出现主机名或用户名() {
        let mut c = LauncherConfig::default();
        ensure_machine_id(&mut c);
        let id = c.support.machine_id.to_ascii_lowercase();
        for name in [feedback::current_user(), feedback::hostname()]
            .into_iter()
            .flatten()
        {
            if name.len() >= 3 {
                assert!(
                    !id.contains(&name.to_ascii_lowercase()),
                    "machineId 里混进了机器信息：{id}"
                );
            }
        }
        // 只能是 16 进制与连字符 —— 掺了别的就说明不是纯随机
        assert!(id.chars().all(|ch| ch.is_ascii_hexdigit() || ch == '-'));
    }

    #[test]
    fn 请求体字段齐全而且没有_key_明文() {
        let c = cfg_with("11111111-2222-4333-8444-555555555555");
        let p = Preview {
            sections: Vec::new(),
            body: "===== 概要 (summary) =====\nHUNTER_KEY=<已移除>\nkey=hunt_tools_****\n".into(),
            body_bytes: 0,
            truncated: false,
            machine_id: c.support.machine_id.clone(),
            endpoint: ENDPOINT.into(),
            stage: "upgrade".into(),
            error_code: "E_PULL_STALLED".into(),
            summary: "升级时从腾讯云香港拉镜像 90 秒没有数据进来".into(),
            meta_lines: Vec::new(),
            scan_hit: None,
            auto_on_error: false,
        };
        let v = build_request(&c, &p);
        // 四个必填项
        for k in ["machineId", "launcherVer", "os", "body"] {
            assert!(v.get(k).is_some(), "少了必填字段 {k}：{v}");
        }
        assert_eq!(v["machineId"], "11111111-2222-4333-8444-555555555555");
        assert_eq!(v["launcherVer"], env!("CARGO_PKG_VERSION"));
        assert_eq!(v["hunterVer"], "1.2.0");
        assert_eq!(v["stage"], "upgrade");
        assert_eq!(v["errorCode"], "E_PULL_STALLED");
        assert!(v.get("meta").is_some());
        // **服务端那个可选的 hunterKey 字段我们一个字节都不填**（红线 2）
        assert!(
            v.get("hunterKey").is_none(),
            "请求体里出现了 hunterKey：{v}"
        );
        // 整份 JSON 里不许有 key 明文形状
        let s = v.to_string();
        assert!(
            feedback::assert_clean(&s).is_none(),
            "请求体没过出口闸：{:?}",
            feedback::assert_clean(&s)
        );
    }

    #[test]
    fn 脱敏之后的正文里没有_key_口令_用户名_主机名() {
        // 造一份「什么都没抹」的原始现场，走一遍我们真正用的那条链
        let raw = format!(
            "HUNTER_KEY=hunt_tools_abcdefgh12345678\n\
             POSTGRES_PASSWORD=Sup3rSecretPw\n\
             Authorization: Bearer abcdefgh12345678\n\
             /Users/{user}/.hunter/app/.env\n\
             Name: {host}\n\
             user={user} host={host}\n",
            user = "zhangsan",
            host = "zhangsan-mbp"
        );
        let cleaned = redact::mask_names(
            &redact::mask_home(&redact::redact(&raw)),
            Some("zhangsan"),
            Some("zhangsan-mbp"),
        );
        assert!(!cleaned.contains("hunt_tools_abcdefgh"), "{cleaned}");
        assert!(!cleaned.contains("Sup3rSecretPw"), "{cleaned}");
        assert!(!cleaned.contains("zhangsan"), "{cleaned}");
        assert!(!cleaned.contains("zhangsan-mbp"), "{cleaned}");
        // 出口闸也得认它是干净的
        assert_eq!(
            feedback::assert_clean_strict_with_names(
                &cleaned,
                Some("zhangsan".into()),
                Some("zhangsan-mbp".into())
            ),
            None,
            "{cleaned}"
        );
    }

    #[test]
    fn 正文超限时从头部截_保住末尾与概要() {
        let mut body = String::from("===== 概要 (summary) =====\n启动器: 0.1.16\n\n");
        body.push_str("===== 启动器日志 (launcher-log) =====\n");
        for i in 0..50_000 {
            body.push_str(&format!("第 {i} 行日志\n"));
        }
        body.push_str("最后一行才是线索\n");
        let (out, cut) = truncate_body(&body, 10_000);
        assert!(cut);
        assert!(out.len() <= 10_000 + 400, "截完还有 {} 字节", out.len());
        assert!(out.contains("启动器: 0.1.16"), "概要被截掉了");
        assert!(out.ends_with("最后一行才是线索\n"), "线索在末尾，不许截掉");
        assert!(out.contains("被启动器截掉了"), "要说清楚截过");
        // 没超限的不动它
        let (same, cut2) = truncate_body("短的", 10_000);
        assert!(!cut2);
        assert_eq!(same, "短的");
    }

    #[test]
    fn 分节拼正文只带勾上的那几节() {
        let secs = vec![
            sec("summary", "概要", "启动器: 0.1.16"),
            sec("env", ".env", "LLM_BASE_URL=hunter.agentpit.io"),
            sec("log-api", "api 日志", "什么都没有"),
        ];
        let body = build_body(&secs, &["summary".to_string(), "log-api".to_string()]);
        assert!(body.contains("概要"));
        assert!(body.contains("api 日志"));
        assert!(!body.contains("LLM_BASE_URL"), "没勾的节不许出现：{body}");
    }

    #[test]
    fn 三档失败各有各的说法() {
        let (m429, bundle429) = failure_message(
            429,
            "上传太频繁：这台机器每小时最多 5 次",
            "/home/u/.hunter/logs/launcher.log",
        );
        assert!(m429.contains("每小时最多 5 次"));
        // **路径要是能用的那一个**，不能抹成 `<用户目录>/…` —— 抹了他就找不到文件了
        assert!(
            m429.contains("/home/u/.hunter/logs/launcher.log"),
            "要说清日志在哪：{m429}"
        );
        assert!(!bundle429, "限流没必要再导一个包");

        let (m413, bundle413) = failure_message(413, "", "/home/u/.hunter/logs/launcher.log");
        assert!(m413.contains("太大"));
        assert!(bundle413);

        let (m500, bundle500) = failure_message(500, "", "/home/u/.hunter/logs/launcher.log");
        assert!(m500.contains("我们这边"), "服务端的错不要甩给用户：{m500}");
        assert!(bundle500);
    }

    #[test]
    fn 开关关着时绝不上传() {
        let mut c = LauncherConfig::default();
        assert!(!c.support.auto_on_error, "默认必须是关的");
        assert!(!should_auto_upload(&c, Some("E_PULL_STALLED")));
        assert!(!should_auto_upload(&c, None));
        c.support.auto_on_error = true;
        // 开着，但**没出错**照样不传
        assert!(!should_auto_upload(&c, None));
        assert!(!should_auto_upload(&c, Some("")));
        assert!(!should_auto_upload(&c, Some("   ")));
        assert!(should_auto_upload(&c, Some("E_PULL_STALLED")));
    }

    #[test]
    fn 出口闸拦下的东西一个字节都不发() {
        let c = cfg_with("11111111-2222-4333-8444-555555555555");
        let p = Preview {
            sections: Vec::new(),
            body: "key=hunt_tools_abcdefgh12345678".into(),
            body_bytes: 31,
            truncated: false,
            machine_id: c.support.machine_id.clone(),
            endpoint: ENDPOINT.into(),
            stage: String::new(),
            error_code: String::new(),
            summary: String::new(),
            meta_lines: Vec::new(),
            scan_hit: Some("第 1 行出现疑似未脱敏的 key".into()),
            auto_on_error: false,
        };
        // 扫出东西就直接 Err，连请求都不会发出去
        let e = upload(&c, &p).expect_err("必须拒绝");
        assert!(e.msg.contains("取消上传"), "{}", e.msg);
    }
}
