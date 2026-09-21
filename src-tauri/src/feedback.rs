//! 反馈与诊断包（技术方案 §11.1 的启动器侧、§12.3 的脱敏规则）。
//!
//! ## 本轮为什么不上报
//!
//! 方案 §11.1 写的是「提交到 `POST https://telemetry.agentpit.io/v1/feedback`」。
//! M0 实测这个域名**不存在**，上游 api 也没有 `/api/feedback`。按总控规则对
//! 「不存在的服务」的处理办法，启动器侧只做三件**真的做得到**的事：
//!
//! 1. 把诊断信息**收集齐、脱敏好、摆出来给用户看**，每一节都能单独勾掉不带；
//! 2. 导出成一个 zip 放在用户自己的磁盘上（`~/.hunter/diagnostics/`）；
//! 3. 打开一个**预填好标题与正文**的 GitHub issue 链接 —— 诊断包**不自动上传**，
//!    用户自己决定要不要把那个 zip 拖进 issue。
//!
//! 界面上把这一条写在最显眼的地方，绝不做「提交成功」的假象（红线 1）。
//!
//! ## 脱敏（§12.3）
//!
//! 方案原文说的前缀是 `hk_`，实测 key 前缀是 `hunt_tools_`（总控规则已更正），
//! `redact` 两个都认。这个模块在 `redact` 之上再加两条方案要求的规则：
//!
//! * `.env` 只留**白名单里的四项**，而且 `LLM_BASE_URL` 只留主机名（[`env_summary`]）；
//! * 容器日志额外过一遍 [`crate::redact::mask_content_fields`]，把 `content:` / `text:`
//!   整段换成 `[redacted]`。
//!
//! 打包之前还有一道**兜底自查**（[`assert_clean`]）：整包扫一遍，发现 key 形状、
//! 完整邮箱、完整 IPv4 就直接报错，不让这个 zip 生成出来。

use std::collections::BTreeMap;

use crate::compose;
use crate::config::LauncherConfig;
use crate::err::{AppError, AppResult, Code};
use crate::paths;
use crate::redact;
use crate::runtime::docker;
use crate::zip;

/// `.env` 里**允许出现在诊断包里**的键（方案 §12.3 第二条）。
/// 除了这四个，其余的值一律换成 `<已移除>` —— 连长度都不透露。
pub const ENV_ALLOWLIST: [&str; 4] = [
    "LLM_BASE_URL",
    "LLM_DEFAULT_MODEL",
    "DATA_SOURCE_PROVIDER",
    "OPENCODE_TAG",
];

/// 诊断包里的一节。用户可以逐节勾选带不带（方案 §11.1「用户可预览与删除」）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Section {
    /// 稳定的 id，前端按它记住勾选状态
    pub id: String,
    pub title: String,
    /// **已经脱敏过**的正文
    pub body: String,
    /// 默认勾不勾
    pub default_on: bool,
    /// 这一节为什么是空的（拿不到时写原因，不编内容）
    pub note: Option<String>,
}

/// 用户填的表单。**联系方式不进 zip、不进 issue 正文**，只在用户自己复制时用得上。
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Form {
    /// deploy | result | feature | other
    pub kind: String,
    pub description: String,
    #[serde(default)]
    pub contact: String,
    #[serde(default)]
    pub error_code: String,
}

impl Form {
    pub fn kind_cn(&self) -> &'static str {
        match self.kind.as_str() {
            "deploy" => "部署问题",
            "result" => "分析结果不对",
            "feature" => "想要的功能",
            _ => "其他",
        }
    }
}

/// 把 `.env` 压成诊断包能带的那几行（方案 §12.3 第二条）。
///
/// `LLM_BASE_URL` 只留主机名 —— 完整 URL 里可能带着路径形式的凭证，而排查时
/// 我们只需要知道「他连的是哪一家」。
pub fn env_summary(env: &BTreeMap<String, String>) -> String {
    let mut out = String::new();
    for (k, v) in env {
        if ENV_ALLOWLIST.contains(&k.as_str()) {
            let shown = if k == "LLM_BASE_URL" {
                crate::http::host_of(v)
            } else {
                v.clone()
            };
            out.push_str(&format!("{k}={shown}\n"));
        } else {
            out.push_str(&format!("{k}=<已移除>\n"));
        }
    }
    out
}

/// 收一份完整诊断。**每一节都已经脱敏**，调用方直接显示或打包即可。
pub fn collect(cfg: &LauncherConfig, launcher_version: &str) -> Vec<Section> {
    let mut out = Vec::new();

    // ① 概要：方案 §11.1 要求的「Hunter 版本、启动器版本、OS、Docker 版本、架构、模型别名」
    let d = docker::detect();
    let mut s = String::new();
    s.push_str(&format!("时间: {}\n", crate::timefmt::now_shanghai()));
    s.push_str(&format!("启动器: {launcher_version}\n"));
    s.push_str(&format!(
        "系统: {} {}\n",
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    s.push_str(&format!(
        "Docker: {:?} · server {} · compose {}\n",
        d.runtime,
        d.server_version.clone().unwrap_or_else(|| "—".into()),
        d.compose_version.clone().unwrap_or_else(|| "—".into()),
    ));
    // I4：可执行文件定位的结果。0.1.3 在用户 mac 上那个 P0 之后，
    // 「启动器用的是哪一个 docker」是收到 issue 时第一个要问的问题，
    // 与其来回问，不如让诊断包自带
    let probe = crate::runtime::which::docker_probe();
    s.push_str(&format!("docker 定位: {}\n", probe.one_line()));
    if !probe.found() {
        for l in probe.detail_lines() {
            s.push_str(&format!("  {l}\n"));
        }
        s.push_str(&format!(
            "  PATH: {}\n",
            std::env::var("PATH").unwrap_or_else(|_| "（没有）".into())
        ));
    }
    s.push_str(&format!("Hunter tag: {}\n", cfg.hunter.tag));
    s.push_str(&format!(
        "镜像源: {} ({})\n",
        cfg.hunter.registry_id, cfg.hunter.registry_prefix
    ));
    s.push_str(&format!(
        "端口: web {} · api {} · opencode {} · postgres {} · redis {}\n",
        cfg.hunter.ports.web,
        cfg.hunter.ports.api,
        cfg.hunter.ports.opencode,
        cfg.hunter.ports.postgres,
        cfg.hunter.ports.redis
    ));
    s.push_str(&format!(
        "模型: {} · {}\n",
        cfg.model.mode,
        if cfg.model.model.is_empty() {
            "—"
        } else {
            &cfg.model.model
        }
    ));
    s.push_str(&format!(
        "遥测: {} · 上报端点: {}\n",
        if cfg.telemetry.enabled { "开" } else { "关" },
        if cfg.telemetry.endpoint.is_empty() {
            "（无 · 暂未开启上报）"
        } else {
            &cfg.telemetry.endpoint
        }
    ));
    out.push(Section {
        id: "summary".into(),
        title: "概要".into(),
        body: redact::redact(&s),
        default_on: true,
        note: None,
    });

    // ② docker compose ps
    let (ps_body, ps_note) = match compose::ps() {
        Ok(list) if !list.is_empty() => {
            let mut b = String::new();
            for p in &list {
                b.push_str(&format!(
                    "{:<10} {:<10} {:<8} 端口 {}\n",
                    p.service,
                    p.state,
                    compose::health_cn(p.health),
                    p.port
                        .map(|x| x.to_string())
                        .unwrap_or_else(|| "内部".into())
                ));
            }
            (b, None)
        }
        Ok(_) => (
            String::new(),
            Some("docker compose ps 没有返回任何容器（这一套还没起过？）".to_string()),
        ),
        Err(e) => (String::new(), Some(format!("{}: {}", e.code, e.msg))),
    };
    out.push(Section {
        id: "ps".into(),
        title: "docker compose ps".into(),
        body: redact::redact(&ps_body),
        default_on: true,
        note: ps_note,
    });

    // ③ .env（只留白名单四项）
    let env = crate::config::parse_env_file(&paths::env_file());
    let (env_body, env_note) = if env.is_empty() {
        (
            String::new(),
            Some(format!("{} 不存在或是空的", paths::env_file().display())),
        )
    } else {
        (env_summary(&env), None)
    };
    out.push(Section {
        id: "env".into(),
        title: ".env（只保留白名单四项，其余的值一律移除）".into(),
        body: redact::redact(&env_body),
        default_on: true,
        note: env_note,
    });

    // ④ 启动器日志
    let ll = crate::log::tail_file(200);
    out.push(Section {
        id: "launcher-log".into(),
        title: "启动器日志（最近 200 行）".into(),
        body: redact::redact(&ll.join("\n")),
        default_on: true,
        note: if ll.is_empty() {
            Some("还没有日志".into())
        } else {
            None
        },
    });

    // ⑤ api 与 opencode 的容器日志（方案 §11.1 点名要这两个，各 100 行）
    for svc in ["api", "opencode"] {
        let (body, note) = match compose::logs(Some(svc), 100) {
            Ok(l) if !l.is_empty() => (l.join("\n"), None),
            Ok(_) => (String::new(), Some(format!("{svc} 没有日志输出"))),
            Err(e) => (String::new(), Some(format!("{}: {}", e.code, e.msg))),
        };
        out.push(Section {
            id: format!("log-{svc}"),
            title: format!("{svc} 容器日志（最近 100 行 · content/text 已整段抹除）"),
            // 容器日志多过一道：模型的输入输出整段换掉（§12.3 第三条）
            body: redact::redact(&redact::mask_content_fields(&body)),
            default_on: true,
            note,
        });
    }

    // ⑥ 最近一次工具调用的脱敏摘要 —— 方案 §11.1 要，但上游没有这个接口。
    //    如实写明，不编（红线 1）。
    out.push(Section {
        id: "last-tool-call".into(),
        title: "最近一次工具调用摘要".into(),
        body: String::new(),
        default_on: false,
        note: Some(
            "上游 api 没有 /api/system/last-tool-call（方案 §13 列了，实测不存在）。\
             这一节没有数据来源，详见成果文档「需上游配合」。"
                .into(),
        ),
    });

    // I4：诊断包里的路径会带着用户名（`/Users/zhangsan/.hunter/...`），
    // 而这份东西的去处是 GitHub issue。统一再过一道 —— 用户名换成 `<用户目录>`，
    // 后半截留着（「`.hunter/app/.env` 写不进去」还得靠它才看得出来）。
    for sec in &mut out {
        sec.body = redact::mask_home(&sec.body);
        if let Some(n) = &sec.note {
            sec.note = Some(redact::mask_home(n));
        }
    }

    out
}

/// 诊断包里绝不允许出现的形状。返回第一处命中的描述。
///
/// 这是**生成 zip 之前的最后一道闸**：上面每一节其实都已经过了 `redact`，
/// 这里再整包扫一遍，是为了挡住「日后谁加了一节却忘了脱敏」这种事。
pub fn assert_clean(text: &str) -> Option<String> {
    for (i, line) in text.lines().enumerate() {
        if let Some(what) = dirty_in_line(line) {
            return Some(format!("第 {} 行出现{}：{}", i + 1, what, line.trim()));
        }
    }
    None
}

fn dirty_in_line(line: &str) -> Option<&'static str> {
    // key 形状：前缀后面还跟着 8 个以上 token 字符（脱敏后是 `前缀****`，不会命中）
    for prefix in ["hunt_tools_", "sk-", "hk_"] {
        let mut from = 0;
        while let Some(pos) = line[from..].find(prefix) {
            let start = from + pos + prefix.len();
            let n = line[start..]
                .bytes()
                .take_while(|b| b.is_ascii_alphanumeric() || *b == b'-' || *b == b'_')
                .count();
            if n >= 8 {
                return Some("疑似未脱敏的 key");
            }
            from = start.max(from + pos + 1);
        }
    }
    // 完整邮箱
    for word in line.split(|c: char| c.is_whitespace() || c == ',' || c == ';' || c == '"') {
        if let Some(at) = word.find('@') {
            let (local, rest) = word.split_at(at);
            if local.len() > 1
                && rest[1..].contains('.')
                && local
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._%+-".contains(&b))
                && !local.ends_with("***")
            {
                return Some("完整邮箱");
            }
        }
    }
    // 完整 IPv4（本机地址与广播地址除外 —— 它们不是隐私，红线 4 的自查还要靠 127.0.0.1）
    if let Some(ip) = first_full_ipv4(line) {
        if !matches!(ip.as_str(), "127.0.0.1" | "0.0.0.0" | "255.255.255.255") {
            return Some("完整 IP 地址");
        }
    }
    None
}

fn first_full_ipv4(line: &str) -> Option<String> {
    let b = line.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() && (i == 0 || !(b[i - 1].is_ascii_digit() || b[i - 1] == b'.')) {
            let mut j = i;
            let mut segs = 0;
            let mut ok = true;
            while segs < 4 {
                let s = j;
                while j < b.len() && b[j].is_ascii_digit() {
                    j += 1;
                }
                if j == s || j - s > 3 {
                    ok = false;
                    break;
                }
                segs += 1;
                if segs < 4 {
                    if j < b.len() && b[j] == b'.' {
                        j += 1;
                    } else {
                        ok = false;
                        break;
                    }
                }
            }
            if ok && segs == 4 && (j == b.len() || !(b[j].is_ascii_digit() || b[j] == b'.')) {
                return Some(line[i..j].to_string());
            }
            i = j.max(i + 1);
            continue;
        }
        i += 1;
    }
    None
}

/// AI 自己导一份**全量**诊断包（I5 动作表 v2 的 `export_feedback_bundle`）。
///
/// 和用户在反馈页导的是同一条路 —— 同一套 [`collect`] + 同一套脱敏，
/// 区别只是「勾选哪几节」由代码给（全勾）而不是由用户点。
/// **不上传**，只落到 `~/.hunter/diagnostics/`。
pub fn export_bundle() -> AppResult<std::path::PathBuf> {
    let cfg = LauncherConfig::load();
    let sections = collect(&cfg, env!("CARGO_PKG_VERSION"));
    let include: Vec<String> = sections.iter().map(|s| s.id.clone()).collect();
    let form = Form {
        kind: "deploy".into(),
        description: "AI 自动安装没能解决问题，由启动器自动导出的现场".into(),
        contact: String::new(),
        error_code: String::new(),
    };
    let (path, _bytes) = export_zip(&sections, &include, &form, env!("CARGO_PKG_VERSION"))?;
    Ok(std::path::PathBuf::from(path))
}

/// 把选中的几节 + 表单打成一个 zip，返回 (落地路径, 字节数)。
///
/// `include` 是要带上的节 id；空数组表示一节都不带（只带表单）。
pub fn export_zip(
    sections: &[Section],
    include: &[String],
    form: &Form,
    launcher_version: &str,
) -> AppResult<(String, usize)> {
    let mut entries = Vec::new();

    let mut readme = String::new();
    readme.push_str("# Hunter 启动器 · 诊断包\n\n");
    readme.push_str(&format!("导出时间：{}\n", crate::timefmt::now_shanghai()));
    readme.push_str(&format!("启动器版本：{launcher_version}\n\n"));
    readme.push_str("这个包是**你自己机器上**导出的，启动器没有把它发给任何人。\n");
    readme.push_str("要不要贴进 GitHub issue 由你决定。\n\n");
    readme.push_str("已按技术方案 §12.3 脱敏：key、Bearer、邮箱、手机号、IP 末段、\n");
    readme.push_str("`.env` 里白名单之外的值、容器日志里的 content/text 字段，全部已抹除。\n\n");
    readme.push_str("## 反馈内容\n\n");
    readme.push_str(&format!("类型：{}\n", form.kind_cn()));
    if !form.error_code.is_empty() {
        readme.push_str(&format!("错误码：{}\n", form.error_code));
    }
    readme.push_str(&format!("\n{}\n\n", form.description.trim()));
    readme.push_str("## 这个包里有什么\n\n");
    for s in sections {
        let on = include.iter().any(|i| i == &s.id);
        readme.push_str(&format!(
            "- {} {}{}\n",
            if on { "[x]" } else { "[ ]" },
            s.title,
            s.note
                .as_ref()
                .map(|n| format!("（{n}）"))
                .unwrap_or_default()
        ));
    }
    entries.push(zip::Entry {
        name: "README.md".into(),
        // 用户填的描述是自由文本 —— 同样要过脱敏（他可能把 key 粘进来了）
        data: redact::redact(&readme).into_bytes(),
    });

    for s in sections {
        if !include.iter().any(|i| i == &s.id) {
            continue;
        }
        let mut body = String::new();
        body.push_str(&format!("# {}\n\n", s.title));
        if let Some(n) = &s.note {
            body.push_str(&format!("（{n}）\n\n"));
        }
        body.push_str(&s.body);
        if !body.ends_with('\n') {
            body.push('\n');
        }
        entries.push(zip::Entry {
            name: format!("{}.txt", s.id),
            data: body.into_bytes(),
        });
    }

    // 兜底自查：整包扫一遍，脏了就不生成
    let all: String = entries
        .iter()
        .map(|e| String::from_utf8_lossy(&e.data).into_owned())
        .collect::<Vec<_>>()
        .join("\n");
    if let Some(what) = assert_clean(&all) {
        return Err(AppError::new(
            Code::Unknown,
            format!("诊断包自查没通过，已取消导出：{what}"),
        ));
    }

    let bytes = zip::write(&entries);
    paths::ensure_dirs()?;
    let name = format!(
        "hunter-diagnostics-{}.zip",
        crate::timefmt::now_shanghai()
            .replace(['-', ':'], "")
            .replace(' ', "-")
    );
    let path = paths::diagnostics_dir().join(&name);
    std::fs::write(&path, &bytes).map_err(|e| {
        AppError::new(
            Code::ConfigWrite,
            format!("写 {} 失败：{e}", path.display()),
        )
    })?;
    paths::chmod_600(&path)?;
    crate::linfo!("诊断包已导出：{} · {} 字节", path.display(), bytes.len());
    Ok((path.to_string_lossy().into_owned(), bytes.len()))
}

// ── 一键反馈直达（I7） ────────────────────────────────────────────────────

/// 「发送诊断给开发者」按一下之后，界面拿到的那一份。
///
/// **注意它里面没有「已上报」这种字段。** 启动器不会把任何东西发到我们的服务器
/// （`telemetry.agentpit.io` 根本不存在，总控规则里写着不许假装上报成功）。
/// 它做的是三件事：在本机生成脱敏包、把包扫一遍、拼一条预填好的 issue 链接。
/// 发不发、发什么，全程由用户点。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OneClick {
    /// 诊断包在本机的绝对路径（用户要能找得到它）
    pub bundle_path: String,
    /// 包有多大（字节，真实值）
    pub bundle_bytes: usize,
    /// 预填好的 GitHub issue 链接。**用户确认之后**才由界面打开浏览器
    pub issue_url: String,
    /// issue 的标题与正文（界面上先给用户看一眼，再决定要不要打开浏览器）
    pub issue_title: String,
    pub issue_body: String,
    /// 出门前的整包扫描结果。`None` = 干净
    pub scan_hit: Option<String>,
    /// 给用户看的一句话
    pub note: String,
}

/// 生成诊断包 + 预填 issue。**不打开浏览器、不发送任何东西。**
///
/// 打开浏览器是界面上另一次点击的事（`feedback_open_issue`），
/// 中间隔着一屏「这是要发出去的内容」。理由很简单：一键直达指的是
/// 「少点几下」，不是「不给人看就发出去」。
pub fn one_click(error_code: &str, error_message: &str) -> AppResult<OneClick> {
    let cfg = LauncherConfig::load();
    let sections = collect(&cfg, env!("CARGO_PKG_VERSION"));
    let include: Vec<String> = sections.iter().map(|s| s.id.clone()).collect();
    let desc = if error_message.trim().is_empty() {
        "AI 自动安装没能把问题解决，导出现场请开发者看看。".to_string()
    } else {
        format!(
            "AI 自动安装没能把问题解决。最后卡在：{}",
            redact::mask_home(&redact::redact(error_message.trim()))
        )
    };
    let form = Form {
        kind: "deploy".into(),
        description: desc,
        contact: String::new(),
        error_code: error_code.trim().to_string(),
    };
    let (path, bytes) = export_zip(&sections, &include, &form, env!("CARGO_PKG_VERSION"))?;

    let summary = sections
        .iter()
        .find(|s| s.id == "summary")
        .map(|s| s.body.clone())
        .unwrap_or_default();
    let url = issue_url(&form, &summary);
    let (title, body) = issue_parts(&form, &summary);

    // 出门前再扫一遍**将要贴出去的那段文字**。包本身在 `export_zip` 里已经扫过，
    // 这一段是另外拼的，得单独过闸（I7 新增：连同用户名与主机名一起查）
    let scan_hit = assert_clean_strict(&format!(
        "{title}
{body}"
    ));
    if let Some(h) = &scan_hit {
        crate::lwarn!("一键反馈的 issue 正文没过出口闸：{h}");
    }
    let note = format!(
        "诊断包已经生成在你自己的机器上：{}（{} 字节）。启动器**没有**把它发给任何人 ——          下一步会打开浏览器、把标题和正文预填进一条 GitHub issue，要不要发由你决定；         包要不要附上，也由你自己拖进去。",
        redact::mask_home(&path),
        bytes
    );
    Ok(OneClick {
        bundle_path: path,
        bundle_bytes: bytes,
        issue_url: url,
        issue_title: title,
        issue_body: body,
        scan_hit,
        note,
    })
}

/// 比 [`assert_clean`] 再严一档：连**用户名**与**主机名**都不许出现（I7）。
///
/// 为什么要单独一档：`mask_home` 把 `/Users/zhang` 这种路径抹成了 `~`，
/// 但用户名可能以别的形态混进来（容器名前缀、`whoami` 的输出、
/// compose 项目名跟着目录名走……）。主机名同理 —— 它会出现在
/// `docker info` 的 `Name:` 一行里，而那一行本身是有用的诊断信息，
/// 所以是**这里**负责在出门前拦住它，而不是把整段扔掉。
pub fn assert_clean_strict(text: &str) -> Option<String> {
    assert_clean_strict_with(text, current_user(), hostname())
}

/// [`assert_clean_strict`] 的纯函数内核 —— 用户名与主机名由调用方给。
///
/// 为什么要拆出来：原来的单测直接拿**跑测试这台机器**的真实用户名与主机名去断言，
/// 于是它在「主机名里恰好含有用户名」的机器上必然失败 ——
/// GitHub 的 runner 就是这种（用户 `runner`、主机名也带 `runner`），
/// I7 第一次推上 CI 就红在这里。本地 446 条全绿、CI 红一条，差别只是机器名字。
///
/// 闸门本身没问题（那台机器上照样拦住了，只是理由写成「用户名」），
/// 有问题的是**把机器状态当成测试前提**。拆开之后逻辑可以喂死值去考。
fn assert_clean_strict_with(
    text: &str,
    user: Option<String>,
    host: Option<String>,
) -> Option<String> {
    if let Some(h) = assert_clean(text) {
        return Some(h);
    }
    for (what, value) in [("用户名", user), ("主机名", host)] {
        let Some(v) = value else { continue };
        // 太短的名字（`u`、`ci`）会把正常词误伤成命中，只查 3 个字符以上的
        if v.len() < 3 {
            continue;
        }
        for (i, line) in text.lines().enumerate() {
            if line.contains(&v) {
                return Some(format!(
                    "第 {} 行出现{}：{}",
                    i + 1,
                    what,
                    line.trim().chars().take(120).collect::<String>()
                ));
            }
        }
    }
    None
}

/// 当前用户名。取不到就是 `None`（**不猜**）。
pub fn current_user() -> Option<String> {
    for k in ["USER", "USERNAME", "LOGNAME"] {
        if let Ok(v) = std::env::var(k) {
            let v = v.trim().to_string();
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    // 退一步：家目录的最后一段通常就是用户名
    crate::paths::home()
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty() && s != "/")
}

/// 本机主机名。取不到就是 `None`。
pub fn hostname() -> Option<String> {
    if let Ok(v) = std::env::var("HOSTNAME") {
        let v = v.trim().to_string();
        if !v.is_empty() {
            return Some(v);
        }
    }
    if let Ok(v) = std::env::var("COMPUTERNAME") {
        let v = v.trim().to_string();
        if !v.is_empty() {
            return Some(v);
        }
    }
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// GitHub 仓库。issue 开在启动器自己的仓库里 —— 用户遇到的是部署问题。
pub const ISSUE_REPO: &str = "https://github.com/agentpit-io/HunterLauncher";

/// 拼一个**预填好**的 issue 链接。诊断包不自动上传，正文里只放概要那一节。
pub fn issue_url(form: &Form, summary: &str) -> String {
    let (title, body) = issue_parts(form, summary);
    format!(
        "{ISSUE_REPO}/issues/new?title={}&body={}",
        urlencode(&redact::redact(&title)),
        urlencode(&redact::redact(&body))
    )
}

/// issue 的标题与正文。**链接里编的就是这两段** —— 界面上给用户预览的也是它们，
/// 不存在「看到的是一套、发出去的是另一套」。
pub fn issue_parts(form: &Form, summary: &str) -> (String, String) {
    let title = if form.error_code.is_empty() {
        format!("[{}] ", form.kind_cn())
    } else {
        format!("[{}] {} ", form.kind_cn(), form.error_code)
    };
    let title = format!(
        "{}{}",
        title,
        form.description
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(60)
            .collect::<String>()
    );

    let mut body = String::new();
    body.push_str("## 遇到了什么\n\n");
    body.push_str(form.description.trim());
    body.push_str("\n\n## 环境\n\n```\n");
    body.push_str(summary.trim());
    body.push_str("\n```\n\n");
    body.push_str(
        "## 诊断包\n\n> 启动器已经在本机导出了一份脱敏诊断包，**没有自动上传**。\n\
         > 需要的话把它拖进这条 issue：`~/.hunter/diagnostics/` 下最新的那个 zip。\n",
    );
    (redact::redact(&title), redact::redact(&body))
}

/// URL 百分号编码（RFC 3986 的 unreserved 之外一律编码）。
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(*b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_摘要只留白名单四项() {
        let mut env = BTreeMap::new();
        env.insert(
            "HUNTER_API_KEY".to_string(),
            "hunt_tools_AbCdEfGhIjKlMnOpQrStUvWxYz012345".to_string(),
        );
        env.insert("POSTGRES_PASSWORD".to_string(), "s3cr3t-pw".to_string());
        env.insert("JWT_SECRET".to_string(), "abcdef==".to_string());
        env.insert(
            "LLM_BASE_URL".to_string(),
            "https://hunter.agentpit.io/api/saas/llm/v1".to_string(),
        );
        env.insert("LLM_DEFAULT_MODEL".to_string(), "hunter-chat".to_string());
        env.insert("DATA_SOURCE_PROVIDER".to_string(), "hunter".to_string());

        let s = env_summary(&env);
        assert!(s.contains("HUNTER_API_KEY=<已移除>"), "{s}");
        assert!(s.contains("POSTGRES_PASSWORD=<已移除>"), "{s}");
        assert!(s.contains("JWT_SECRET=<已移除>"), "{s}");
        // BASE_URL 只留主机名，路径一并去掉
        assert!(s.contains("LLM_BASE_URL=hunter.agentpit.io"), "{s}");
        assert!(!s.contains("/api/saas/llm/v1"), "路径不该留下来：{s}");
        assert!(s.contains("LLM_DEFAULT_MODEL=hunter-chat"), "{s}");
        assert!(s.contains("DATA_SOURCE_PROVIDER=hunter"), "{s}");
        assert!(!s.contains("s3cr3t-pw"), "口令泄漏了：{s}");
    }

    #[test]
    fn 自查能抓出没脱敏的_key() {
        assert!(
            assert_clean("HUNTER_API_KEY=hunt_tools_AbCdEfGhIjKlMnOpQrStUvWxYz012345").is_some()
        );
        assert!(assert_clean("token sk-1234567890abcdef").is_some());
        assert!(assert_clean("old hk_abcdefghijklmnop").is_some());
        // 已经脱敏过的形状不该被误报
        assert!(assert_clean("HUNTER_API_KEY=hunt_tools_****").is_none());
        assert!(assert_clean("Authorization: Bearer ****").is_none());
    }

    #[test]
    fn 自查能抓出邮箱与完整_ip() {
        assert!(assert_clean("联系方式 zhang.san@example.com").is_some());
        assert!(assert_clean("client 203.0.113.42 connected").is_some());
        // 脱敏之后的形状放行
        assert!(assert_clean("联系方式 z***@example.com").is_none());
        assert!(assert_clean("client 203.0.113.* connected").is_none());
        // 本机地址不算隐私，必须放行 —— 红线 4 的自查靠它
        assert!(assert_clean("0.0.0.0:3100->3100/tcp 127.0.0.1:8100").is_none());
        // 版本号不能被误当成 IP
        assert!(assert_clean("compose v2.20.1 docker 29.8.1").is_none());
    }

    #[test]
    fn 诊断包自查挡得住脏内容() {
        let dirty = vec![Section {
            id: "x".into(),
            title: "故意没脱敏的一节".into(),
            body: "HUNTER_API_KEY=hunt_tools_AbCdEfGhIjKlMnOpQrStUvWxYz012345".into(),
            default_on: true,
            note: None,
        }];
        let e = export_zip(&dirty, &["x".to_string()], &Form::default(), "0.1.0")
            .expect_err("脏内容必须导不出来");
        assert!(e.msg.contains("自查没通过"), "{}", e.msg);
    }

    #[test]
    fn issue_链接是预填好的而且不带联系方式() {
        let f = Form {
            kind: "deploy".into(),
            description: "拉镜像一直卡在 30%\n第二行".into(),
            contact: "me@example.com".into(),
            error_code: "E_PULL_FAILED".into(),
        };
        let u = issue_url(&f, "启动器: 0.1.0\n系统: linux x86_64");
        assert!(
            u.starts_with(&format!("{ISSUE_REPO}/issues/new?title=")),
            "{u}"
        );
        // 标题里有类型、错误码与第一行描述
        let dec = percent_decode(&u);
        assert!(
            dec.contains("[部署问题] E_PULL_FAILED 拉镜像一直卡在 30%"),
            "{dec}"
        );
        assert!(dec.contains("没有自动上传"), "{dec}");
        // 联系方式绝不能进 issue 正文
        assert!(!dec.contains("me@example.com"), "联系方式泄漏了：{dec}");
    }

    #[test]
    fn issue_链接里的用户描述也会脱敏() {
        let f = Form {
            kind: "other".into(),
            description: "我的 key 是 hunt_tools_AbCdEfGhIjKlMnOpQrStUvWxYz012345 报错".into(),
            ..Default::default()
        };
        let dec = percent_decode(&issue_url(&f, ""));
        assert!(!dec.contains("AbCdEfGhIjKl"), "{dec}");
        assert!(dec.contains("hunt_tools_****"), "{dec}");
    }

    /// 测试里把百分号编码解回来，方便断言。
    fn percent_decode(s: &str) -> String {
        let b = s.as_bytes();
        let mut out: Vec<u8> = Vec::with_capacity(b.len());
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'%' && i + 2 < b.len() {
                if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                    out.push(v);
                    i += 3;
                    continue;
                }
            }
            out.push(b[i]);
            i += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    // ── I7 · 一键反馈直达 ─────────────────────────────────────────────────

    /// 出口闸比 [`assert_clean`] 再严一档：**用户名与主机名也不许出现**。
    ///
    /// 喂死值，不看跑测试这台机器叫什么 —— 那正是这条测试第一次上 CI 红掉的原因。
    #[test]
    fn 严格闸门扫得出用户名与主机名() {
        let u = || Some("zhangsan".to_string());
        let h = || Some("zhangsan-macbook".to_string());

        let hit = assert_clean_strict_with("容器名 zhangsan-hunter-web-1", u(), None);
        assert!(hit.as_deref().unwrap_or("").contains("用户名"), "{hit:?}");

        let hit = assert_clean_strict_with("Name: zhangsan-macbook", None, h());
        assert!(hit.as_deref().unwrap_or("").contains("主机名"), "{hit:?}");

        // 主机名里含着用户名时（GitHub runner 就是这样），**先命中哪一条不重要** ——
        // 重要的是它一定被拦住。这一条以前被写成了「必须报主机名」，那是多余的要求。
        let hit = assert_clean_strict_with("Name: zhangsan-macbook", u(), h());
        assert!(hit.is_some(), "含用户名的主机名也必须被拦住");

        // 太短的名字不参与匹配，否则正常词会被误伤
        assert_eq!(
            assert_clean_strict_with("镜像源测速：ghcr 619 ms", Some("ci".into()), None),
            None
        );
        // 取不到用户名 / 主机名时这两档直接跳过，不影响其它档
        assert_eq!(assert_clean_strict_with("一切正常", None, None), None);

        // key 那一档照旧
        assert!(assert_clean_strict("hunt_tools_abcdefghijklmnopqrstuvwxyz012345").is_some());
        // 干净的文本要放行（这一句里不会有任何真实机器的用户名或主机名）
        assert_eq!(assert_clean_strict("启动器 0.1.7 · macos aarch64"), None);
    }

    /// 真机那一层（读 `USER` / `/etc/hostname`）也要走一遍：**拆出内核之后不能
    /// 把外层这层忘了**。不断言理由写的是哪一条 —— 那取决于这台机器叫什么。
    #[test]
    fn 严格闸门在真实机器上也拦得住() {
        if let Some(u) = current_user() {
            if u.len() >= 3 {
                assert!(
                    assert_clean_strict(&format!("容器名 {u}-hunter-web-1")).is_some(),
                    "用户名 {u} 该被扫出来"
                );
            }
        }
        if let Some(h) = hostname() {
            if h.len() >= 3 {
                assert!(
                    assert_clean_strict(&format!("Name: {h}")).is_some(),
                    "主机名 {h} 该被扫出来"
                );
            }
        }
    }

    /// 太短的用户名（`ci`、`u`）不参与匹配 —— 否则正常词会被误伤。
    #[test]
    fn 太短的名字不误伤() {
        // 这一条只能间接验：造一段必然含 "ci" 的正常文本，它不该被判脏
        let text = "镜像源测速：ghcr 619 ms";
        let hit = assert_clean_strict(text);
        // 只有当真实用户名/主机名恰好出现在这句话里才会命中；正常机器上不会
        if let (Some(u), Some(h)) = (current_user(), hostname()) {
            if !text.contains(&u) && !text.contains(&h) {
                assert_eq!(hit, None, "{hit:?}");
            }
        }
    }

    /// 一键反馈**不会**声称上报成功，而且路径与字节数都是真的。
    #[test]
    fn 一键反馈只在本机生成不发送() {
        let r = one_click("E_PULL_FAILED", "docker compose pull 退出码 1").expect("能生成");
        assert!(r.bundle_bytes > 0, "包不该是空的");
        assert!(
            std::path::Path::new(&r.bundle_path).exists(),
            "包要真的在：{}",
            r.bundle_path
        );
        assert!(r.issue_url.starts_with(ISSUE_REPO), "{}", r.issue_url);
        assert!(r.issue_title.contains("E_PULL_FAILED"), "{}", r.issue_title);
        // **不许出现「已上报 / 已发送」这类话**（总控规则：不得假装上报成功）
        for bad in ["已上报", "已发送", "上传成功"] {
            assert!(!r.note.contains(bad), "回执里不该写「{bad}」：{}", r.note);
        }
        assert!(r.note.contains("没有"), "要写清楚没发给任何人：{}", r.note);
        // 正文里不许出现 key
        assert_eq!(assert_clean(&r.issue_body), None);
        let _ = std::fs::remove_file(&r.bundle_path);
    }

    /// 界面上看到的标题正文，**就是链接里编的那两段**。
    #[test]
    fn 预览的内容与链接里的一致() {
        let f = Form {
            kind: "deploy".into(),
            description: "装不上".into(),
            contact: String::new(),
            error_code: "E_PORT_CONFLICT".into(),
        };
        let (title, body) = issue_parts(&f, "启动器: 0.1.7");
        let url = issue_url(&f, "启动器: 0.1.7");
        let dec = percent_decode(&url);
        assert!(dec.contains(&title), "链接里的标题和预览的不一样");
        assert!(dec.contains(body.trim()), "链接里的正文和预览的不一样");
    }
}
