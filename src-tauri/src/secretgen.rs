//! 随机密钥生成。用操作系统的 CSPRNG（`getrandom`），不用任何伪随机数。
//!
//! 生成出来的值一律 [`crate::redact::register_secret`] 登记一次 ——
//! 数据库口令与 JWT_SECRET 同样不该出现在日志里。
//!
//! ## 为什么字母表里没有 `+` 与 `/`（I2 实测撞出来的）
//!
//! 原来这里用的是**标准 base64**。生成出来的 `POSTGRES_PASSWORD` 会被上游拼进一个
//! DSN：`postgresql://hunter:<口令>@postgres:5432/hunter`。口令里只要有一个 `/`，
//! URL 解析就在那里把 authority 截断了，于是 api 容器起不来，日志里一直刷：
//!
//! ```text
//! 迁移 | 数据库还没起来，2 秒后重试（第 29 次）：
//!       invalid integer value "u3s9Ckg" for connection option "port"
//! ```
//!
//! 这是 I2 的 GUI 回归真撞到的 —— `up -d` 直接失败（`container hunter-api-1 is unhealthy`），
//! 一次安装彻底装不起来。24 字节 base64 是 32 个字符，其中出现至少一个 `/` 的概率是
//! `1 - (63/64)^32 ≈ 39.6%`，再算上 `+`（`+` 在 URL 的 userinfo 里会被一些解析器
//! 当成空格）就是 `1 - (62/64)^32 ≈ 63.5%`。也就是说**每装三次就有一两次装不起来**，
//! 而且它是随机的 —— 上一次好好的，这一次就不行，最难查的那种故障。
//!
//! 修法是把字母表收成 `[A-Za-z0-9]`（62 个字符）。损失的熵可以忽略
//! （`log2(62)/log2(64) = 0.994`，32 个字符从 192 位降到 190 位），
//! 换来的是「生成出来的东西放进 URL、shell、YAML 里都不用转义」。
//!
//! 不去改上游的 DSN 拼接：总控规则要求本项目只读引用 hunter-community。
//! 已在成果文档的「需上游配合」一节记一笔（它那边该做 URL 编码），
//! 但启动器这边**不依赖上游改**也能彻底不踩这个坑。

use crate::err::{AppError, AppResult, Code};

/// 生成一串 `n` 字节熵的随机口令，字母表是 `[A-Za-z0-9]`。
///
/// 返回的字符数固定为 `n * 4 / 3` 向上取整后的那个长度 —— 与原来的 base64 一致，
/// 这样升级上来的机器和新装的机器口令长度看起来一样（口令本身是 sticky 的，不会被换掉）。
pub fn random_token(n: usize) -> AppResult<String> {
    const T: &[u8; 62] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let want = n.div_ceil(3) * 4;
    let mut out = String::with_capacity(want);
    // 拒绝采样：256 不是 62 的整数倍，直接取模会让前 8 个字符略微偏多。
    // 丢掉 >= 248 的字节（概率 8/256）之后分布就是均匀的。
    let mut buf = vec![0u8; want * 2];
    while out.len() < want {
        getrandom::fill(&mut buf)
            .map_err(|e| AppError::new(Code::ConfigWrite, format!("取随机数失败：{e}")))?;
        for b in &buf {
            if out.len() == want {
                break;
            }
            if *b < 248 {
                out.push(T[(*b % 62) as usize] as char);
            }
        }
    }
    crate::redact::register_secret(&out);
    Ok(out)
}

/// n 字节随机数的 base64（标准字母表，带 `=` 填充）。
///
/// ⚠ **不要再拿它生成会进 URL 的口令** —— 见模块头。留着是因为 [`b64`] 本身
/// 还有别处要用，而且那几条编码正确性的测试是有价值的。
#[allow(dead_code)]
pub fn random_base64(n: usize) -> AppResult<String> {
    let mut buf = vec![0u8; n];
    getrandom::fill(&mut buf)
        .map_err(|e| AppError::new(Code::ConfigWrite, format!("取随机数失败：{e}")))?;
    let s = b64(&buf);
    crate::redact::register_secret(&s);
    Ok(s)
}

/// 标准 base64。只有几十字节，自己写比拖一个 crate 划算。
fn b64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(T[(n >> 18 & 63) as usize] as char);
        out.push(T[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6 & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_编码正确() {
        assert_eq!(b64(b""), "");
        assert_eq!(b64(b"f"), "Zg==");
        assert_eq!(b64(b"fo"), "Zm8=");
        assert_eq!(b64(b"foo"), "Zm9v");
        assert_eq!(b64(b"foob"), "Zm9vYg==");
        assert_eq!(b64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn 生成的密钥长度对且两次不一样() {
        let a = random_token(48).unwrap();
        let b = random_token(48).unwrap();
        assert_eq!(
            a.len(),
            64,
            "48 字节的熵对应 64 个字符，与原来的 base64 长度一致"
        );
        assert_ne!(a, b, "两次生成撞车说明用的不是 CSPRNG");
        assert_eq!(random_token(24).unwrap().len(), 32);
        assert_eq!(random_token(18).unwrap().len(), 24);
    }

    /// I2 的 GUI 回归真撞到的那个 P0：口令里的 `/` 会把上游拼出来的
    /// `postgresql://hunter:<口令>@postgres:5432/hunter` 截断，api 容器起不来。
    /// 这条测试跑 200 次 —— 单次出现 `/` 的概率约 40%，200 次全躲过去的概率
    /// 小到可以当成零，所以它不是一条「碰运气」的测试。
    #[test]
    fn 生成的口令里绝不出现会破坏_url_的字符() {
        for _ in 0..200 {
            let s = random_token(24).unwrap();
            assert!(
                s.bytes().all(|b| b.is_ascii_alphanumeric()),
                "口令里出现了非字母数字字符：{}",
                s.bytes()
                    .filter(|b| !b.is_ascii_alphanumeric())
                    .map(|b| b as char)
                    .collect::<String>()
            );
            // 把它真拼进一个 DSN，主机与端口必须还认得出来
            let dsn = format!("postgresql://hunter:{s}@postgres:5432/hunter");
            let after_at = dsn.rsplit_once('@').unwrap().1;
            assert_eq!(after_at, "postgres:5432/hunter", "DSN 被口令截断了：{dsn}");
            assert_eq!(dsn.matches('@').count(), 1);
            assert_eq!(dsn.matches("//").count(), 1);
        }
    }

    #[test]
    fn 生成的密钥会被登记进脱敏表() {
        let s = random_token(24).unwrap();
        let r = crate::redact::redact(&format!("POSTGRES_PASSWORD={s}"));
        assert!(!r.contains(&s), "生成的口令必须自动进脱敏表：{r}");
    }

    /// 62 个字符的分布要均匀 —— 直接 `% 62` 会让前 8 个字母出现得多一点。
    /// 这条测试只是粗筛：2 万个字符里每个字符至少要出现一次。
    #[test]
    fn 字母表用满且没有明显偏斜() {
        use std::collections::HashSet;
        let mut seen = HashSet::new();
        for _ in 0..300 {
            for c in random_token(48).unwrap().chars() {
                seen.insert(c);
            }
        }
        assert_eq!(seen.len(), 62, "62 个字符应当都出现过，实际 {}", seen.len());
    }
}
