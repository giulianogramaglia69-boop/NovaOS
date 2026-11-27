//! Epoch → data/ora locali in italiano, in puro Rust (niente chrono:
//! non è nelle dipendenze del progetto e non vale una dipendenza nuova
//! per una formattazione).
//!
//! Conversione giorni → calendario con l'algoritmo civile di
//! Howard Hinnant (dominio pubblico). L'offset locale viene da
//! `date +%z` (sempre presente su Fedora); se il comando manca,
//! si usa UTC.

const MESI: [&str; 12] = [
    "gen", "feb", "mar", "apr", "mag", "giu", "lug", "ago", "set", "ott", "nov", "dic",
];

/// (anno, mese 1–12, giorno 1–31) da giorni trascorsi dal 1970-01-01.
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

/// Offset locale in secondi (da `date +%z`, es. +0100 → 3600).
/// 0 se il comando non è disponibile o non si riesce a parsare.
pub fn local_offset_seconds() -> i64 {
    parse_offset().unwrap_or(0)
}

fn parse_offset() -> Option<i64> {
    let out = std::process::Command::new("date")
        .arg("+%z")
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let text = text.trim();
    if text.len() != 5 {
        return None;
    }
    let (sign, digits) = match text.as_bytes()[0] {
        b'+' => (1i64, &text[1..]),
        b'-' => (-1i64, &text[1..]),
        _ => return None,
    };
    let hours: i64 = digits.get(..2)?.parse().ok()?;
    let minutes: i64 = digits.get(2..)?.parse().ok()?;
    Some(sign * (hours * 3600 + minutes * 60))
}

fn format_with_offset(epoch_secs: u64, offset_secs: i64) -> String {
    let local = epoch_secs as i64 + offset_secs;
    let days = local.div_euclid(86_400);
    let secs_of_day = local.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = secs_of_day / 3600;
    let minute = (secs_of_day % 3600) / 60;
    format!(
        "{day} {} {year}, {hour:02}:{minute:02}",
        MESI[(month - 1) as usize]
    )
}

/// "26 nov 2026, 18:32" in UTC.
pub fn format_timestamp_utc(epoch_secs: u64) -> String {
    format_with_offset(epoch_secs, 0)
}

/// "26 nov 2026, 18:32" in ora locale.
pub fn format_timestamp(epoch_secs: u64) -> String {
    format_with_offset(epoch_secs, local_offset_seconds())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_zero() {
        assert_eq!(format_timestamp_utc(0), "1 gen 1970, 00:00");
    }

    #[test]
    fn known_dates_utc() {
        // Valori verificati con `date -u -d @<ts>` sulla VM.
        assert_eq!(format_timestamp_utc(1_764_100_000), "25 nov 2025, 19:46");
        assert_eq!(format_timestamp_utc(1_732_560_000), "25 nov 2024, 18:40");
        assert_eq!(format_timestamp_utc(1_700_000_000), "14 nov 2023, 22:13");
    }

    #[test]
    fn offset_applied() {
        // Valori deterministici: l'offset è un parametro, non il fuso
        // della macchina che esegue i test.
        assert_eq!(
            format_with_offset(1_764_100_000, 3600),
            "25 nov 2025, 20:46"
        );
        assert_eq!(
            format_with_offset(1_764_100_000, -18_000),
            "25 nov 2025, 14:46"
        );
        assert_eq!(
            format_with_offset(1_764_100_000, 14_400),
            "25 nov 2025, 23:46"
        );
        // Attraversamento della mezzanotte: +5h → giorno dopo.
        assert_eq!(
            format_with_offset(1_764_100_000, 18_000),
            "26 nov 2025, 00:46"
        );
    }

    #[test]
    fn leap_day() {
        assert_eq!(format_timestamp_utc(951_782_400), "29 feb 2000, 00:00");
    }

    #[test]
    fn civil_roundtrip_edges() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        assert_eq!(civil_from_days(11_000), (2000, 2, 13));
    }
}
