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

/// 文件名安全的上海时间戳：`20260921-183045`。
///
/// 定长，所以**按名字排序 = 按时间排序**（事件流归档就靠这一点决定删哪几份）。
pub fn file_stamp_unix(unix: i64) -> String {
    let (y, mo, d, h, mi, s) = civil_from_unix(unix + SHANGHAI_OFFSET_SECS);
    format!("{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}")
}

pub fn file_stamp_now() -> String {
    file_stamp_unix(now_unix())
}

/// 从 [`SystemTime`] 来一份（文件的修改时间）。1970 之前的时间当成 0。
pub fn file_stamp(t: SystemTime) -> String {
    let unix = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    file_stamp_unix(unix)
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

/// `YYYY-MM-DD HH:MM:SS`（上海时间）→ Unix 秒。**[`format_shanghai`] 的逆运算**。
///
/// I13 才需要它：「上一次成功的自动备份是什么时候」存在配置里的是一串人看的时间，
/// 而「距今多少小时」这个判断要拿它跟现在比。认不出来的格式返回 `None` ——
/// 配置文件被手改成别的样子时，宁可不判也不猜（红线 1）。
pub fn unix_from_shanghai(s: &str) -> Option<i64> {
    let s = s.trim();
    let (d, t) = s.split_once(' ')?;
    let mut dp = d.split('-');
    let y: i64 = dp.next()?.parse().ok()?;
    let mo: u32 = dp.next()?.parse().ok()?;
    let da: u32 = dp.next()?.parse().ok()?;
    let mut tp = t.split(':');
    let h: i64 = tp.next()?.parse().ok()?;
    let mi: i64 = tp.next()?.parse().ok()?;
    let se: i64 = tp.next().unwrap_or("0").parse().ok()?;
    if !(1..=12).contains(&mo) || !(1..=31).contains(&da) || h > 23 || mi > 59 || se > 60 {
        return None;
    }
    let days = days_from_civil(y, mo, da);
    Some(days * 86_400 + h * 3600 + mi * 60 + se - SHANGHAI_OFFSET_SECS)
}

/// 距离那个时刻过去了多少小时（负数说明那个时间在未来）。认不出来返回 `None`。
pub fn hours_since_shanghai(s: &str) -> Option<f64> {
    let t = unix_from_shanghai(s)?;
    Some((now_unix() - t) as f64 / 3600.0)
}

/// Howard Hinnant 的 days_from_civil，[`civil_from_unix`] 的逆。
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = m as i64;
    let d = d as i64;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 上海时间来回换算是同一个数() {
        for t in [1_789_794_881_i64, 1_709_222_399, 1_709_222_400, 0] {
            let s = format_shanghai(t);
            assert_eq!(unix_from_shanghai(&s), Some(t), "{s}");
        }
    }

    #[test]
    fn 认不出来的时间返回_none_不猜() {
        assert_eq!(unix_from_shanghai(""), None);
        assert_eq!(unix_from_shanghai("昨天"), None);
        assert_eq!(unix_from_shanghai("2026-09-23"), None);
        assert_eq!(unix_from_shanghai("2026-13-01 00:00:00"), None, "13 月");
        assert_eq!(unix_from_shanghai("2026-09-23 25:00:00"), None, "25 点");
    }

    #[test]
    fn 距今多少小时() {
        let one_hour_ago = format_shanghai(now_unix() - 3600);
        let h = hours_since_shanghai(&one_hour_ago).expect("认得出来");
        assert!((h - 1.0).abs() < 0.1, "{h}");
        assert_eq!(hours_since_shanghai("不是时间"), None);
    }

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
