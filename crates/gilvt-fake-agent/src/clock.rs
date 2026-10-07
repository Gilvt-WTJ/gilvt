//! Timestamps and ids in the formats the real CLIs write (no date or uuid crate).

use std::io::Read;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// `2026-09-25T02:24:03.230Z`: what both CLIs write in records.
pub fn iso_utc(t: SystemTime) -> String {
    let ms = millis(t);
    let (secs, milli) = (ms.div_euclid(1000), ms.rem_euclid(1000));
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{milli:03}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// Local calendar time (year, month, day, hour, minute, second): Codex names its day directories and
/// rollout files after it.
pub fn local_parts(t: SystemTime) -> (i32, u32, u32, u32, u32, u32) {
    let secs = millis(t).div_euclid(1000) as libc::time_t;
    // SAFETY: `localtime_r` only writes the `tm` we pass; a zeroed `tm` is a valid value.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let ok = unsafe { !libc::localtime_r(&secs, &mut tm).is_null() };
    if !ok {
        let (y, m, d) = civil_from_days((secs as i64).div_euclid(86_400));
        return (y as i32, m as u32, d as u32, 0, 0, 0);
    }
    (tm.tm_year + 1900, tm.tm_mon as u32 + 1, tm.tm_mday as u32, tm.tm_hour as u32, tm.tm_min as u32, tm.tm_sec as u32)
}

/// Unix seconds (Codex `started_at` / `completed_at`).
pub fn unix_secs(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO).as_secs()
}

fn millis(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_millis() as i64,
        Err(e) => -(e.duration().as_millis() as i64),
    }
}

/// Civil-from-days (Howard Hinnant): days since 1970-01-01 → (year, month, day).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// `n` random bytes from `/dev/urandom`, or (if that fails) from the clock, pid and a counter.
pub fn random_bytes(n: usize) -> Vec<u8> {
    let mut buf = vec![0u8; n];
    if std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut buf)).is_ok() {
        return buf;
    }
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut x = (millis(SystemTime::now()) as u64) ^ (u64::from(std::process::id()) << 32);
    x ^= COUNTER.fetch_add(1, Ordering::Relaxed).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    for b in &mut buf {
        // xorshift64*
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        *b = (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 56) as u8;
    }
    buf
}

fn hyphenate(b: &[u8]) -> String {
    let hex: String = b.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

/// A random (version 4) UUID: Claude session ids, record uuids, prompt ids.
pub fn uuid_v4() -> String {
    let mut b = random_bytes(16);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    hyphenate(&b)
}

/// A time-ordered (version 7) UUID: Codex session and turn ids.
pub fn uuid_v7(t: SystemTime) -> String {
    let mut b = random_bytes(16);
    let ms = millis(t).max(0) as u64;
    b[..6].copy_from_slice(&ms.to_be_bytes()[2..]);
    b[6] = (b[6] & 0x0f) | 0x70;
    b[8] = (b[8] & 0x3f) | 0x80;
    hyphenate(&b)
}

/// `prefix` + `n` random chars from Anthropic's id alphabet (`msg_011Cf…`, `toolu_01…`, `req_…`).
pub fn anthropic_id(prefix: &str, n: usize) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let body: String = random_bytes(n).iter().map(|b| ALPHABET[usize::from(*b) % ALPHABET.len()] as char).collect();
    format!("{prefix}01{body}")
}

/// A UUID string (8-4-4-4-12 hex digits).
pub fn is_uuid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == 5
        && parts.iter().zip([8, 4, 4, 4, 12]).all(|(p, n)| p.len() == n && p.bytes().all(|b| b.is_ascii_hexdigit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_timestamps() {
        let t = UNIX_EPOCH + Duration::from_millis(1_790_303_043_230);
        assert_eq!(iso_utc(t), "2026-09-25T02:24:03.230Z");
        assert_eq!(iso_utc(UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso_utc(UNIX_EPOCH + Duration::from_secs(951_782_400)), "2000-02-29T00:00:00.000Z");
    }

    #[test]
    fn local_time_is_a_calendar_time() {
        let (y, m, d, h, mi, s) = local_parts(UNIX_EPOCH + Duration::from_secs(1_790_303_043));
        assert!((2026..=2026).contains(&y) && m == 9 && (24..=25).contains(&d), "{y}-{m}-{d}");
        assert!(h < 24 && mi < 60 && s < 60);
    }

    #[test]
    fn uuids() {
        let v4 = uuid_v4();
        assert!(is_uuid(&v4), "{v4}");
        assert_eq!(&v4[14..15], "4");
        assert_ne!(uuid_v4(), v4);
        let t = UNIX_EPOCH + Duration::from_millis(1_790_297_716_464);
        let v7 = uuid_v7(t);
        assert!(is_uuid(&v7) && &v7[14..15] == "7", "{v7}");
        assert!(v7.starts_with("01a0d60f-"), "time prefix like codex's: {v7}");
        assert!(!is_uuid("not-a-uuid") && !is_uuid("0000000-0000-4000-8000-000000000000"));
    }

    #[test]
    fn anthropic_ids() {
        let id = anthropic_id("toolu_", 22);
        assert!(id.starts_with("toolu_01") && id.len() == "toolu_01".len() + 22);
        assert!(id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'));
    }
}
