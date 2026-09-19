//! 时间格式化。总控规则：**时间一律上海时间**。
//!
//! 没有引 chrono —— 启动器只需要「把 Unix 秒按 +08:00 印成 `YYYY-MM-DD HH:MM:SS`」这一件事，
//! 为它拖进一整套时区数据库不划算。中国自 1991 年起不再有夏令时，固定 +08:00 是正确的。

use std::time::{SystemTime, UNIX_EPOCH};

const SHANGHAI_OFFSET_SECS: i64 = 8 * 3600;

pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn now_shanghai() -> String {
    format_shanghai(now_unix())
}

pub fn format_shanghai(unix: i64) -> String {
    let (y, mo, d, h, mi, s) = civil_from_unix(unix + SHANGHAI_OFFSET_SECS);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}")
}

/// 只给时分秒，日志框里用（视觉稿第 2、3 张的日志行就是 `17:42:08` 这种）。
pub fn hms_shanghai(unix: i64) -> String {
    let (_, _, _, h, mi, s) = civil_from_unix(unix + SHANGHAI_OFFSET_SECS);
    format!("{h:02}:{mi:02}:{s:02}")
}

/// Howard Hinnant 的 civil_from_days 算法，适用于全部公历日期。
fn civil_from_unix(t: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (
        y,
        m,
        d,
        (secs / 3600) as u32,
        ((secs % 3600) / 60) as u32,
        (secs % 60) as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 按上海时间印出() {
        // 2026-09-19T05:14:41Z = 2026-09-19 13:14:41 +08:00
        assert_eq!(format_shanghai(1_789_794_881), "2026-09-19 13:14:41");
        assert_eq!(hms_shanghai(1_789_794_881), "13:14:41");
    }

    #[test]
    fn 跨日与闰年都对() {
        // 2024-02-29T23:59:59+08:00 = 2024-02-29T15:59:59Z
        assert_eq!(format_shanghai(1_709_222_399), "2024-02-29 23:59:59");
        // 再过一秒进 3 月 1 日
        assert_eq!(format_shanghai(1_709_222_400), "2024-03-01 00:00:00");
    }

    #[test]
    fn 现在的时间落在合理区间() {
        let s = now_shanghai();
        assert_eq!(s.len(), 19, "{s}");
        assert!(s.starts_with("20"), "{s}");
    }
}
