//! 随机密钥生成。用操作系统的 CSPRNG（`getrandom`），不用任何伪随机数。
//!
//! 生成出来的值一律 [`crate::redact::register_secret`] 登记一次 ——
//! 数据库口令与 JWT_SECRET 同样不该出现在日志里。

use crate::err::{AppError, AppResult, Code};

/// n 字节随机数的 base64（标准字母表，带 `=` 填充）。
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
        let a = random_base64(48).unwrap();
        let b = random_base64(48).unwrap();
        assert_eq!(a.len(), 64, "48 字节 base64 后是 64 个字符");
        assert_ne!(a, b, "两次生成撞车说明用的不是 CSPRNG");
    }

    #[test]
    fn 生成的密钥会被登记进脱敏表() {
        let s = random_base64(24).unwrap();
        let r = crate::redact::redact(&format!("POSTGRES_PASSWORD={s}"));
        assert!(!r.contains(&s), "生成的口令必须自动进脱敏表：{r}");
    }
}
