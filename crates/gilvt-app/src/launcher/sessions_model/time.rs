//! 「10 分钟前 / 昨天 22:41 / 9 月 21 日」, 「14:30:12 / 昨天 22:41」 and 「18 MB」.

use std::time::{Duration, SystemTime};

/// A moment in local time ([`local`] converts with `localtime_r`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalTime {
    pub year: i32,
    /// 1–12.
    pub month: u32,
    /// 1–31.
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

/// A moment in local time.
pub fn local(t: SystemTime) -> LocalTime {
    let secs = t.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs()) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&secs, &mut tm) }.is_null() {
        return LocalTime { year: 1970, month: 1, day: 1, hour: 0, minute: 0, second: 0 };
    }
    LocalTime {
        year: tm.tm_year + 1900,
        month: (tm.tm_mon + 1) as u32,
        day: tm.tm_mday as u32,
        hour: tm.tm_hour as u32,
        minute: tm.tm_min as u32,
        second: tm.tm_sec as u32,
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date (H. Hinnant's `days_from_civil`).
fn day_number(t: &LocalTime) -> i64 {
    let y = t.year as i64 - (t.month <= 2) as i64;
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = t.month as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + t.day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// When a session was last active, seen at `now`, `ago` earlier: 刚刚, N 分钟前, N 小时前 (the same day),
/// 昨天 HH:MM, M 月 D 日 (this year), YYYY 年 M 月 D 日.
pub fn when_label(t: LocalTime, now: LocalTime, ago: Duration) -> String {
    let secs = ago.as_secs();
    let days = day_number(&now) - day_number(&t);
    match () {
        _ if secs < 60 => "刚刚".into(),
        _ if secs < 3600 => format!("{} 分钟前", secs / 60),
        _ if days <= 0 => format!("{} 小时前", secs / 3600),
        _ if days == 1 => format!("昨天 {:02}:{:02}", t.hour, t.minute),
        _ if t.year == now.year => format!("{} 月 {} 日", t.month, t.day),
        _ => format!("{} 年 {} 月 {} 日", t.year, t.month, t.day),
    }
}

/// When something started, seen at `now`: 「14:30」 (「14:30:12」 with `seconds`) the same day, else minutes
/// with the day in front: 昨天 14:30, 9 月 21 日 14:30 (this year), 2025 年 12 月 30 日 14:30.
pub fn clock_label(t: LocalTime, now: LocalTime, seconds: bool) -> String {
    let hm = format!("{:02}:{:02}", t.hour, t.minute);
    match day_number(&now) - day_number(&t) {
        days if days <= 0 && seconds => format!("{hm}:{:02}", t.second),
        days if days <= 0 => hm,
        1 => format!("昨天 {hm}"),
        _ if t.year == now.year => format!("{} 月 {} 日 {hm}", t.month, t.day),
        _ => format!("{} 年 {} 月 {} 日 {hm}", t.year, t.month, t.day),
    }
}

/// Bytes as Finder counts them (1 MB = 10⁶ B): 「0.4 MB」, 「18 MB」, 「1.6 GB」.
pub fn size_label(bytes: u64) -> String {
    let mb = bytes as f64 / 1e6;
    match () {
        _ if bytes == 0 => "0 MB".into(),
        _ if mb < 0.1 => "< 0.1 MB".into(),
        _ if mb < 9.95 => format!("{mb:.1} MB"),
        _ if mb < 999.5 => format!("{mb:.0} MB"),
        _ => format!("{:.1} GB", mb / 1e3),
    }
}
