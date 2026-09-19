//! 日志与诊断输出的脱敏（技术方案 §12.3 · 总控规则红线 2）。
//!
//! 两道保险：
//!
//! 1. **登记过的密文**：凡是启动器手里真实拿到过的 key（hunter key、自带模型 key、
//!    数据库口令、JWT_SECRET……）都调 [`register_secret`] 登记一次，之后任何一条日志里
//!    只要出现这串字符就会被整段替换。这一道不依赖正则认不认得出它的形状。
//! 2. **形状匹配**：`hunt_tools_…`、`sk-…`、`Bearer …`、邮箱、手机号、IP 末段。
//!    用来兜住那些**没经过启动器的手**、由 docker / compose 自己打出来的串。
//!
//! 两道都过一遍，顺序是先登记后形状 —— 登记的替换更精确。

use std::collections::HashSet;
use std::sync::{OnceLock, RwLock};

/// 登记过的密文集合。只增不减：进程活着期间任何一条日志都要躲开它们。
fn secrets() -> &'static RwLock<HashSet<String>> {
    static S: OnceLock<RwLock<HashSet<String>>> = OnceLock::new();
    S.get_or_init(|| RwLock::new(HashSet::new()))
}

/// 登记一段绝不能出现在日志里的字符串。太短的不登记 —— 否则会把正常文本切得七零八落。
pub fn register_secret(s: &str) {
    let s = s.trim();
    if s.len() < 8 {
        return;
    }
    if let Ok(mut set) = secrets().write() {
        set.insert(s.to_string());
    }
}

/// 写进文档 / 报告 / 界面副本的短打码形式：`hunt_tools_****`。
pub fn mask_key(key: &str) -> String {
    // 取「前缀分隔符」之前的那一段做可辨识前缀：hunt_tools_ / sk- / hk_ 都是这个形状。
    // 只在前 12 个字符里找分隔符，免得把 key 正文里碰巧有的 `-` 当成前缀边界。
    let head = key
        .char_indices()
        .take_while(|(i, _)| *i < 12)
        .filter(|(_, c)| *c == '_' || *c == '-')
        .map(|(i, _)| i)
        .last();
    match head {
        Some(i) if i + 1 < key.len() => format!("{}****", &key[..=i]),
        _ if key.len() > 3 => format!("{}****", &key[..3]),
        _ => "****".to_string(),
    }
}

/// 对一行文本做脱敏。日志、错误串、诊断包统统走这里。
pub fn redact(input: &str) -> String {
    // 顺序：形状 → 登记 → 形状。
    //
    // 为什么形状要跑两遍：登记表里存的可能是某个更长串的**前缀**（比如自带 key 校验时
    // 登记了 `sk-abc`，日志里出现的却是 `sk-abcdef`），单靠登记会留下一截尾巴；
    // 先做形状匹配就能整段打掉。反过来，形状认不出的密文（随机 base64 的数据库口令）
    // 由登记那一道兜住。两道都跑，谁也不漏。
    let mut out = mask_shapes(input);
    if let Ok(set) = secrets().read() {
        for s in set.iter() {
            if out.contains(s.as_str()) {
                out = out.replace(s.as_str(), &mask_key(s));
            }
        }
    }
    mask_shapes(&out)
}

fn mask_shapes(input: &str) -> String {
    let mut out = mask_prefixed(input, "hunt_tools_");
    out = mask_prefixed(&out, "sk-");
    out = mask_prefixed(&out, "hk_");
    out = mask_bearer(&out);
    out = mask_email(&out);
    out = mask_phone_cn(&out);
    out = mask_ip_last_octet(&out);
    out
}

/// 对话内容的整段抹除（技术方案 §12.3 第三条）。
///
/// opencode / api 的日志里会把模型的输入输出原样打出来，形如
/// `... content: "帮我看看 600519 ..."` 或 `"text":"..."`。这些既不是我们要排查的信息，
/// 又是用户最不愿意外传的东西 —— **整段换成 `[redacted]`**，不做任何取舍。
///
/// 识别的形状：`content`、`text`、`message`、`prompt`、`completion` 这几个 key
/// 后面跟 `:` 或 `=`（中间允许有引号、空格），一直吃到行尾或下一个明显的字段边界。
/// 这里故意做得宽 —— 宁可多抹掉一些日志，也不能漏一句用户的话。
pub fn mask_content_fields(s: &str) -> String {
    const KEYS: [&str; 5] = ["content", "text", "message", "prompt", "completion"];
    let mut out = String::with_capacity(s.len());
    for (i, line) in s.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&mask_content_in_line(line, &KEYS));
    }
    out
}

fn mask_content_in_line(line: &str, keys: &[&str]) -> String {
    let lower = line.to_ascii_lowercase();
    let bytes = line.as_bytes();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < bytes.len() {
        let hit = keys.iter().find(|k| {
            lower[i..].starts_with(**k)
                && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric() && bytes[i - 1] != b'_')
        });
        if let Some(k) = hit {
            // key 之后允许 `"`、空格，然后必须是 `:` 或 `=`
            let mut j = i + k.len();
            while j < bytes.len() && matches!(bytes[j], b'"' | b'\'' | b' ') {
                j += 1;
            }
            if j < bytes.len() && matches!(bytes[j], b':' | b'=') {
                out.push_str(&line[i..i + k.len()]);
                out.push_str(&line[i + k.len()..=j]);
                out.push_str("[redacted]");
                return out; // 吃到行尾：值里可能有任意字符，切不干净就别切
            }
        }
        let ch_len = utf8_len(bytes[i]);
        out.push_str(&line[i..i + ch_len]);
        i += ch_len;
    }
    out
}

/// 把 `<前缀><一串 token 字符>` 换成 `<前缀>****`。
/// token 字符 = 字母数字 + `-` + `_`（覆盖 base64url 与常见 key 编码）。
fn mask_prefixed(s: &str, prefix: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if s[i..].starts_with(prefix) {
            let mut j = i + prefix.len();
            while j < bytes.len() && is_token_byte(bytes[j]) {
                j += 1;
            }
            // 至少要跟 8 个 token 字符才算 key，避免把 `sk-` 这种孤零零的词也打掉
            if j - i - prefix.len() >= 8 {
                out.push_str(prefix);
                out.push_str("****");
                i = j;
                continue;
            }
        }
        // 按字符推进，别把多字节的中文切坏
        let ch_len = utf8_len(bytes[i]);
        out.push_str(&s[i..i + ch_len]);
        i += ch_len;
    }
    out
}

fn is_token_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_'
}

fn utf8_len(b: u8) -> usize {
    match b {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        _ => 4,
    }
}

/// `Authorization: Bearer xxx` → `Bearer ****`
fn mask_bearer(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let lower = s.to_ascii_lowercase();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if lower[i..].starts_with("bearer ") {
            let mut j = i + "bearer ".len();
            while j < bytes.len() && (is_token_byte(bytes[j]) || bytes[j] == b'.') {
                j += 1;
            }
            if j > i + "bearer ".len() {
                out.push_str(&s[i..i + "bearer ".len()]);
                out.push_str("****");
                i = j;
                continue;
            }
        }
        let ch_len = utf8_len(bytes[i]);
        out.push_str(&s[i..i + ch_len]);
        i += ch_len;
    }
    out
}

/// `a@b.com` → `a***@b.com`（保留域名，便于判断是不是自家邮箱）
fn mask_email(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for (idx, part) in s.split(' ').enumerate() {
        if idx > 0 {
            out.push(' ');
        }
        match split_email(part) {
            Some((local, rest)) if !local.is_empty() => {
                out.push_str(&local[..1]);
                out.push_str("***");
                out.push_str(rest);
            }
            _ => out.push_str(part),
        }
    }
    out
}

fn split_email(part: &str) -> Option<(&str, &str)> {
    let at = part.find('@')?;
    let (local, rest) = part.split_at(at);
    if !rest[1..].contains('.') || local.is_empty() {
        return None;
    }
    if !local
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"._%+-".contains(&b))
    {
        return None;
    }
    Some((local, rest))
}

/// 11 位手机号 → 保留前 3 后 2
fn mask_phone_cn(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'1'
            && i + 11 <= bytes.len()
            && bytes[i..i + 11].iter().all(|b| b.is_ascii_digit())
            && (i == 0 || !bytes[i - 1].is_ascii_digit())
            && (i + 11 == bytes.len() || !bytes[i + 11].is_ascii_digit())
            && matches!(bytes[i + 1], b'3'..=b'9')
        {
            out.push_str(&s[i..i + 3]);
            out.push_str("******");
            out.push_str(&s[i + 9..i + 11]);
            i += 11;
            continue;
        }
        let ch_len = utf8_len(bytes[i]);
        out.push_str(&s[i..i + ch_len]);
        i += ch_len;
    }
    out
}

/// IPv4 末段打掉：`203.0.113.42` → `203.0.113.*`。
/// **本机地址 127.0.0.1 与 0.0.0.0 保留原样** —— 它们不是隐私，
/// 而且端口绑定是不是收回了本机正靠这个串来判断（红线 4 的自查靠它）。
fn mask_ip_last_octet(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() && (i == 0 || !is_ipish(bytes[i - 1])) {
            if let Some((end, keep, last_start)) = match_ipv4(bytes, i) {
                let text = &s[i..end];
                if text == "127.0.0.1" || text == "0.0.0.0" || text == "255.255.255.255" {
                    out.push_str(text);
                } else {
                    out.push_str(&s[i..last_start]);
                    out.push('*');
                    let _ = keep;
                }
                i = end;
                continue;
            }
        }
        let ch_len = utf8_len(bytes[i]);
        out.push_str(&s[i..i + ch_len]);
        i += ch_len;
    }
    out
}

fn is_ipish(b: u8) -> bool {
    b.is_ascii_digit() || b == b'.'
}

/// 从 `at` 开始匹配一个 IPv4，返回 (结束位置, 段数, 最后一段的起点)
fn match_ipv4(bytes: &[u8], at: usize) -> Option<(usize, usize, usize)> {
    let mut i = at;
    let mut seg = 0;
    let mut last_start = at;
    while seg < 4 {
        let start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        let len = i - start;
        if len == 0 || len > 3 {
            return None;
        }
        last_start = start;
        seg += 1;
        if seg < 4 {
            if i < bytes.len() && bytes[i] == b'.' {
                i += 1;
            } else {
                return None;
            }
        }
    }
    if i < bytes.len() && is_ipish(bytes[i]) {
        return None; // 后面还跟着数字或点，说明不是一个完整 IPv4
    }
    Some((i, seg, last_start))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 方案 §12.3 第三条：opencode / api 日志里的 `content:` / `text:` **整段**换成 `[redacted]`。
    #[test]
    fn 对话内容整段抹除() {
        let line = r#"2026-09-20 01:00:00 opencode INFO content: "帮我看看 600519 这两天怎么了，要不要加仓""#;
        let out = mask_content_fields(line);
        assert!(out.contains("[redacted]"), "{out}");
        assert!(!out.contains("600519"), "股票代码不该留下来：{out}");
        assert!(!out.contains("加仓"), "用户的话不该留下来：{out}");
        // 时间戳与来源要留着 —— 排查还得靠它们
        assert!(
            out.starts_with("2026-09-20 01:00:00 opencode INFO content:"),
            "{out}"
        );
    }

    #[test]
    fn json_形状的_text_字段也抹() {
        let line = r#"{"role":"assistant","text":"茅台今天收 1257.12，跌 0.78%"}"#;
        let out = mask_content_fields(line);
        assert!(!out.contains("1257.12"), "{out}");
        assert!(out.contains("[redacted]"), "{out}");
    }

    #[test]
    fn 逐行处理不会把整份日志吃掉() {
        let logs = "api-1 | INFO: GET /api/health 200 OK\n                    opencode-1 | content: 用户说的话\n                    api-1 | INFO: GET /api/setup/status 200 OK";
        let out = mask_content_fields(logs);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 3, "行数不该变：{out}");
        assert!(
            lines[0].contains("/api/health"),
            "第一行不该动：{}",
            lines[0]
        );
        assert!(lines[1].contains("[redacted]"), "第二行要抹：{}", lines[1]);
        assert!(!lines[1].contains("用户说的话"), "{}", lines[1]);
        assert!(
            lines[2].contains("/api/setup/status"),
            "第三行不该动：{}",
            lines[2]
        );
    }

    /// 不是「字段名」的地方不许乱抹 —— 比如日志里提到 `content-type` 这个词。
    #[test]
    fn 不是字段名的地方不动() {
        let line = "api-1 | INFO: response content-type=application/json size=42";
        assert_eq!(mask_content_fields(line), line);
        let line2 = "compose pull: extracting contents";
        assert_eq!(mask_content_fields(line2), line2);
    }

    /// 测试里用的都是**编造的假 key**，不是测试机上那把真的（红线 2）。
    const FAKE: &str = "hunt_tools_q6sKaaaaaaaaaaaaaaaaaaaaaaaaQMo2";

    #[test]
    fn hunter_key_被打掉() {
        let line = format!("LLM_API_KEY={FAKE} 写入 .env");
        let r = redact(&line);
        assert!(!r.contains("q6sK"), "{r}");
        assert!(r.contains("hunt_tools_****"), "{r}");
        assert!(r.contains("写入 .env"), "中文不能被切坏：{r}");
    }

    #[test]
    fn 登记过的密文即使形状不认识也会被打掉() {
        let weird = "Zm9vYmFyYmF6cXV1eDEyMzQ1Ng";
        assert!(redact(weird).contains(weird), "登记之前应当原样保留");
        register_secret(weird);
        let r = redact(&format!("POSTGRES_PASSWORD={weird}"));
        assert!(!r.contains(weird), "{r}");
    }

    #[test]
    fn 太短的串不登记免得把正常文本切碎() {
        register_secret("ok");
        assert_eq!(redact("everything is ok here"), "everything is ok here");
    }

    #[test]
    fn bearer_头被打掉() {
        let r = redact("Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.abc.def");
        assert!(!r.contains("eyJhbGciOiJIUzI1NiJ9"), "{r}");
        assert!(r.contains("Bearer ****"), "{r}");
    }

    #[test]
    fn 厂商_key_也打掉() {
        let r = redact("LLM_API_KEY=sk-0123456789abcdef0123");
        assert!(!r.contains("0123456789"), "{r}");
        assert_eq!(r, "LLM_API_KEY=sk-****");
    }

    /// 登记表是进程级的（生产里本来就该如此），同一个进程里的别的测试可能已经
    /// 登记过某个前缀。这条测试确认「形状再跑一遍」把那种残留尾巴也清掉了。
    #[test]
    fn 登记了前缀之后更长的同类串也不会留尾巴() {
        register_secret("sk-abcdefgh");
        let r = redact("key=sk-abcdefgh0123456789");
        assert!(!r.contains("0123456789"), "{r}");
        assert!(!r.contains("abcdefgh"), "{r}");
    }

    #[test]
    fn 邮箱与手机号() {
        assert_eq!(
            redact("联系 hangeaiagent@gmail.com 谢谢"),
            "联系 h***@gmail.com 谢谢"
        );
        assert_eq!(redact("手机 13812345678 备用"), "手机 138******78 备用");
    }

    #[test]
    fn 公网_ip_打末段但本机地址保留() {
        assert_eq!(
            redact("来自 203.0.113.42 的请求"),
            "来自 203.0.113.* 的请求"
        );
        // 红线 4 的自查要靠这个串，绝不能被打掉
        assert_eq!(redact("127.0.0.1:8101->8000"), "127.0.0.1:8101->8000");
        assert_eq!(redact("0.0.0.0:3101->3000"), "0.0.0.0:3101->3000");
    }

    #[test]
    fn 普通日志原样通过() {
        let line = "2026-09-19 21:30:01 [info] docker compose --project-name hunter up -d";
        assert_eq!(redact(line), line);
    }

    #[test]
    fn 版本号不会被误当成_ip() {
        assert_eq!(redact("Docker Engine 29.8.1"), "Docker Engine 29.8.1");
        assert_eq!(
            redact("compose v5.5.1 · sha256:4c1e"),
            "compose v5.5.1 · sha256:4c1e"
        );
    }

    #[test]
    fn 打码函数保留可辨识的前缀() {
        assert_eq!(mask_key(FAKE), "hunt_tools_****");
        assert_eq!(mask_key("sk-abcdefghijkl"), "sk-****");
    }
}
