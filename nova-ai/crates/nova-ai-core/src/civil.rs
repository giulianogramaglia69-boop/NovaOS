//! Date math without a chrono dependency: conversions between a
//! civil date (year, month, day) and days since the Unix epoch,
//! using Howard Hinnant's algorithms, plus today's local date
//! (via the `date` utility, which knows the system timezone).

/// Days since 1970-01-01 -> (year, month, day).
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// (year, month, day) -> days since 1970-01-01.
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64; // [0, 399]
    let mp = if m > 2 { m - 3 } else { m + 9 } as u64; // [0, 11]
    let doy = (153 * mp + 2) / 5 + (d as u64) - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe as i64 - 719_468
}

/// Parse "YYYY-MM-DD" (or "YYYYMMDD") into (year, month, day).
pub fn parse_date(s: &str) -> Option<(i64, u32, u32)> {
    let s = s.trim();
    let (y, m, d) = if s.len() == 10 && s.as_bytes()[4] == b'-' && s.as_bytes()[7] == b'-' {
        (
            s[0..4].parse().ok()?,
            s[5..7].parse().ok()?,
            s[8..10].parse().ok()?,
        )
    } else if s.len() == 8 && s.bytes().all(|b| b.is_ascii_digit()) {
        (
            s[0..4].parse().ok()?,
            s[4..6].parse().ok()?,
            s[6..8].parse().ok()?,
        )
    } else {
        return None;
    };
    if !(1..=12).contains(&m) || d == 0 || d > 31 {
        return None;
    }
    Some((y, m, d))
}

/// Today's date in local time as (year, month, day). Runs `date
/// +%Y-%m-%d` (coreutils, always present on NovaOS); falls back to
/// the UTC date if the command is unavailable.
pub fn today_local() -> (i64, u32, u32) {
    if let Ok(out) = std::process::Command::new("date").arg("+%Y-%m-%d").output() {
        let s = String::from_utf8_lossy(&out.stdout);
        if let Some(d) = parse_date(s.trim()) {
            return d;
        }
    }
    let days = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() / 86_400)
        .unwrap_or(0) as i64;
    civil_from_days(days)
}

/// Format (year, month, day) as "YYYY-MM-DD".
pub fn format_date(y: i64, m: u32, d: u32) -> String {
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        // 2026-11-26 -> epoch day 20783 (verified with `date`).
        assert_eq!(days_from_civil(2026, 11, 26), 20783);
        assert_eq!(civil_from_days(20783), (2026, 11, 26));
        assert_eq!(civil_from_days(20387), (2025, 10, 26));
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(days_from_civil(2000, 2, 29)), (2000, 2, 29));
        assert_eq!(
            civil_from_days(days_from_civil(2024, 12, 31)),
            (2024, 12, 31)
        );
    }

    #[test]
    fn parse_dates() {
        assert_eq!(parse_date("2026-11-26"), Some((2026, 11, 26)));
        assert_eq!(parse_date("20261126"), Some((2026, 11, 26)));
        assert_eq!(parse_date("2026-13-01"), None);
        assert_eq!(parse_date("ciao"), None);
    }
}
