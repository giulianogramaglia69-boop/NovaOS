//! Personal calendar, read-only, from an ICS feed: either a URL
//! (e.g. the private Google Calendar iCalendar link) or a local
//! `.ics` file, configured in the settings. The feed is cached in
//! `~/.local/share/nova-ai/calendar.ics` and refreshed at most once
//! every 15 minutes.
//!
//! The parser is handwritten and small: VEVENT with DTSTART / DTEND /
//! SUMMARY / LOCATION. Times are wall-clock as written in the ICS
//! (TZID ignored, trailing `Z` stripped): Google's private ICS links
//! export events in the calendar's own timezone, so for a user in
//! Europe/Rome that is exactly right. UTC (`Z`) events from other
//! sources may be off by the UTC offset — documented in the README.
//!
//! No email, no write access: this tool only reads what is already
//! in the calendar.

use std::fs;
use std::path::PathBuf;

use crate::civil;

/// Refresh interval for the cached feed.
pub const CACHE_MAX_AGE_SECS: u64 = 15 * 60;

/// One calendar event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalEvent {
    /// Start date (year, month, day).
    pub start: (i64, u32, u32),
    /// Start time "HH:MM", None for all-day events.
    pub start_time: Option<String>,
    /// End date (exclusive for all-day events, per RFC 5545).
    pub end: Option<(i64, u32, u32)>,
    pub summary: String,
    pub location: String,
}

impl CalEvent {
    /// Does this event happen on `date` (year, month, day)?
    pub fn on_date(&self, date: (i64, u32, u32)) -> bool {
        let d = civil::days_from_civil(date.0, date.1, date.2);
        let s = civil::days_from_civil(self.start.0, self.start.1, self.start.2);
        if self.start_time.is_some() {
            // Timed event: it happens on its start day. DTEND is the
            // end *time* the same day, not an exclusive date bound.
            return d == s;
        }
        // All-day event: [start, end) per RFC 5545 (DTEND exclusive).
        match self.end {
            Some(e) => {
                let en = civil::days_from_civil(e.0, e.1, e.2);
                d >= s && d < en
            }
            None => d == s,
        }
    }

    /// One line for the chat: "09:00 Lezione di inglese (aula 3)" or
    /// "Tutto il giorno: Compleanno di mamma".
    pub fn line(&self) -> String {
        match &self.start_time {
            Some(t) if self.location.is_empty() => format!("{t} {}", self.summary),
            Some(t) => format!("{t} {} ({})", self.summary, self.location),
            None if self.location.is_empty() => {
                format!("Tutto il giorno: {}", self.summary)
            }
            None => format!("Tutto il giorno: {} ({})", self.summary, self.location),
        }
    }
}

/// Unfold folded ICS lines (a line starting with a space or tab
/// continues the previous one) and split into (name, params, value)
/// triples.
fn parse_properties(text: &str) -> Vec<(String, String)> {
    let mut unfolded: Vec<String> = Vec::new();
    for line in text.lines() {
        if (line.starts_with(' ') || line.starts_with('\t')) && !unfolded.is_empty() {
            let last = unfolded.last_mut().unwrap();
            last.push_str(line.trim_start());
        } else {
            unfolded.push(line.trim_end_matches('\r').to_string());
        }
    }
    unfolded
        .into_iter()
        .filter_map(|l| {
            let (head, value) = l.split_once(':')?;
            Some((head.to_string(), value.to_string()))
        })
        .collect()
}

/// Parse an ICS datetime value: `20261127` (all-day) or
/// `20261127T090000` / `20261127T090000Z`. Returns (date, time).
fn parse_dt(value: &str) -> Option<((i64, u32, u32), Option<String>)> {
    let v = value.trim().trim_end_matches('Z');
    if v.len() == 8 {
        let date = civil::parse_date(v)?;
        Some((date, None))
    } else if v.len() == 15 && v.as_bytes()[8] == b'T' {
        let date = civil::parse_date(&v[..8])?;
        let hh: u32 = v[9..11].parse().ok()?;
        let mm: u32 = v[11..13].parse().ok()?;
        if hh > 23 || mm > 59 {
            return None;
        }
        Some((date, Some(format!("{hh:02}:{mm:02}"))))
    } else {
        None
    }
}

/// Parse ICS text into events. Unknown/malformed events are skipped.
pub fn parse_ics(text: &str) -> Vec<CalEvent> {
    let props = parse_properties(text);
    let mut events = Vec::new();
    let mut cur: Option<CalEvent> = None;
    for (head, value) in &props {
        let name = head.split(';').next().unwrap_or(head);
        match name {
            "BEGIN" if value == "VEVENT" => {
                cur = Some(CalEvent {
                    start: (1970, 1, 1),
                    start_time: None,
                    end: None,
                    summary: String::new(),
                    location: String::new(),
                });
            }
            "END" if value == "VEVENT" => {
                if let Some(mut ev) = cur.take()
                    && !ev.summary.is_empty()
                {
                    // All-day events without DTEND end the next day.
                    if ev.start_time.is_none() && ev.end.is_none() {
                        let d = civil::days_from_civil(ev.start.0, ev.start.1, ev.start.2);
                        ev.end = Some(civil::civil_from_days(d + 1));
                    }
                    events.push(ev);
                }
            }
            _ => {
                if let Some(ev) = cur.as_mut() {
                    match name {
                        "DTSTART" => {
                            if let Some((d, t)) = parse_dt(value) {
                                ev.start = d;
                                ev.start_time = t;
                            }
                        }
                        "DTEND" => {
                            if let Some((d, _)) = parse_dt(value) {
                                ev.end = Some(d);
                            }
                        }
                        "SUMMARY" => ev.summary = unescape_ics(value),
                        "LOCATION" => ev.location = unescape_ics(value),
                        _ => {}
                    }
                }
            }
        }
    }
    events.sort_by_key(|e| (e.start, e.start_time.clone()));
    events
}

/// ICS text escapes: `\,` `\;` `\\` `\n`.
fn unescape_ics(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') | Some('N') => out.push('\n'),
                Some(other) => out.push(other),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The calendar: source config + on-disk cache.
pub struct Calendar {
    /// Remote ICS URL (private Google Calendar link), may be empty.
    pub url: String,
    /// Local .ics file path, may be empty.
    pub file: String,
    /// Cache path (default `~/.local/share/nova-ai/calendar.ics`).
    pub cache_path: PathBuf,
}

impl Calendar {
    pub fn default_cache_path() -> PathBuf {
        crate::data_dir().join("calendar.ics")
    }

    pub fn new(url: String, file: String, cache_path: PathBuf) -> Calendar {
        Calendar {
            url,
            file,
            cache_path,
        }
    }

    pub fn from_config(url: &str, file: &str) -> Calendar {
        Calendar::new(
            url.to_string(),
            file.to_string(),
            Self::default_cache_path(),
        )
    }

    /// True when at least one source is configured.
    pub fn configured(&self) -> bool {
        !self.url.trim().is_empty() || !self.file.trim().is_empty()
    }

    /// Load the ICS text: local file wins when set, else the cache
    /// (refreshed from the URL when older than 15 minutes).
    /// Returns (ics text, note) — the note explains a stale cache.
    pub fn load(&self) -> Result<(String, Option<String>), String> {
        if !self.file.trim().is_empty() {
            let text = fs::read_to_string(self.file.trim())
                .map_err(|e| format!("impossibile leggere il file ICS {}: {e}", self.file))?;
            return Ok((text, None));
        }
        if self.url.trim().is_empty() {
            return Err("nessun calendario configurato: imposta l'URL del calendario o un file .ics nelle impostazioni".to_string());
        }
        let stale = match fs::metadata(&self.cache_path) {
            Ok(m) => m
                .modified()
                .ok()
                .and_then(|t| t.elapsed().ok())
                .map(|e| e.as_secs() > CACHE_MAX_AGE_SECS)
                .unwrap_or(true),
            Err(_) => true,
        };
        if stale {
            match self.refresh() {
                Ok(()) => {}
                Err(e) => {
                    // Fall back to the stale cache, honestly.
                    if let Ok(text) = fs::read_to_string(&self.cache_path) {
                        return Ok((
                            text,
                            Some(format!(
                                "aggiornamento calendario fallito ({e}): uso la copia in cache"
                            )),
                        ));
                    }
                    return Err(format!("impossibile scaricare il calendario: {e}"));
                }
            }
        }
        let text = fs::read_to_string(&self.cache_path)
            .map_err(|e| format!("impossibile leggere la cache del calendario: {e}"))?;
        Ok((text, None))
    }

    /// Download the URL feed into the cache.
    pub fn refresh(&self) -> Result<(), String> {
        let url = self.url.trim();
        if url.is_empty() {
            return Err("nessun URL calendario configurato".to_string());
        }
        let resp = crate::llm::LlmClient::http_agent()
            .get(url)
            .call()
            .map_err(|e| format!("download calendario fallito: {e}"))?;
        if resp.status().as_u16() != 200 {
            return Err(format!(
                "il server del calendario ha risposto HTTP {}",
                resp.status().as_u16()
            ));
        }
        let mut body = resp.into_body().into_reader();
        let mut text = String::new();
        use std::io::Read;
        body.read_to_string(&mut text)
            .map_err(|e| format!("lettura calendario fallita: {e}"))?;
        if let Some(parent) = self.cache_path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("impossibile creare la cache: {e}"))?;
        }
        let tmp = self
            .cache_path
            .with_extension(format!("tmp-{}", std::process::id()));
        fs::write(&tmp, &text).map_err(|e| format!("scrittura cache fallita: {e}"))?;
        fs::rename(&tmp, &self.cache_path).map_err(|e| format!("scrittura cache fallita: {e}"))?;
        Ok(())
    }

    /// All events, sorted.
    pub fn events(&self) -> Result<(Vec<CalEvent>, Option<String>), String> {
        let (text, note) = self.load()?;
        Ok((parse_ics(&text), note))
    }

    /// Events on one date.
    pub fn events_on(
        &self,
        date: (i64, u32, u32),
    ) -> Result<(Vec<CalEvent>, Option<String>), String> {
        let (events, note) = self.events()?;
        Ok((
            events.into_iter().filter(|e| e.on_date(date)).collect(),
            note,
        ))
    }

    /// Events today (local date).
    pub fn events_today(&self) -> Result<(Vec<CalEvent>, Option<String>), String> {
        self.events_on(civil::today_local())
    }

    /// Events tomorrow (local date).
    pub fn events_tomorrow(&self) -> Result<(Vec<CalEvent>, Option<String>), String> {
        let t = civil::today_local();
        let d = civil::days_from_civil(t.0, t.1, t.2) + 1;
        self.events_on(civil::civil_from_days(d))
    }

    /// Search summary/location (case-insensitive substring), most
    /// recent first, at most 20.
    pub fn search(&self, query: &str) -> Result<(Vec<CalEvent>, Option<String>), String> {
        let q = query.trim().to_lowercase();
        let (events, note) = self.events()?;
        let mut hits: Vec<CalEvent> = events
            .into_iter()
            .filter(|e| {
                q.is_empty()
                    || e.summary.to_lowercase().contains(&q)
                    || e.location.to_lowercase().contains(&q)
            })
            .collect();
        hits.reverse();
        hits.truncate(20);
        Ok((hits, note))
    }
}

/// Format a list of events for the chat. `label` is "oggi", "domani"
/// or the date.
pub fn format_events(label: &str, events: &[CalEvent]) -> String {
    if events.is_empty() {
        return format!("Niente in programma {label}.");
    }
    let mut out = format!("In programma {label}:");
    for e in events {
        out.push_str(&format!("\n- {}", e.line()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//IT\r\nBEGIN:VEVENT\r\nUID:1@test\r\nDTSTART;TZID=Europe/Rome:20261127T090000\r\nDTEND;TZID=Europe/Rome:20261127T100000\r\nSUMMARY:Lezione di inglese\r\nLOCATION:Aula 3\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:2@test\r\nDTSTART;VALUE=DATE:20261128\r\nDTEND;VALUE=DATE:20261129\r\nSUMMARY:Compleanno di mamma\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:3@test\r\nDTSTART:20261127T183000Z\r\nDTEND:20261127T193000Z\r\nSUMMARY:Palestra\\, gambe\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:4@test\r\nDTSTART;VALUE=DATE:20261130\r\nDTEND;VALUE=DATE:20261202\r\nSUMMARY:Gita scolastica\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    #[test]
    fn parses_fixture() {
        let evs = parse_ics(FIXTURE);
        assert_eq!(evs.len(), 4);
        let lezione = &evs[0];
        assert_eq!(lezione.summary, "Lezione di inglese");
        assert_eq!(lezione.location, "Aula 3");
        assert_eq!(lezione.start, (2026, 11, 27));
        assert_eq!(lezione.start_time.as_deref(), Some("09:00"));
        assert_eq!(lezione.line(), "09:00 Lezione di inglese (Aula 3)");

        let compleanno = evs.iter().find(|e| e.summary.contains("mamma")).unwrap();
        assert_eq!(compleanno.start_time, None);
        assert!(compleanno.on_date((2026, 11, 28)));
        assert!(!compleanno.on_date((2026, 11, 29))); // DTEND exclusive
        assert_eq!(compleanno.line(), "Tutto il giorno: Compleanno di mamma");

        let palestra = evs.iter().find(|e| e.summary.contains("Palestra")).unwrap();
        assert_eq!(palestra.summary, "Palestra, gambe"); // unescaped \,
        assert_eq!(palestra.start_time.as_deref(), Some("18:30"));

        let gita = evs.iter().find(|e| e.summary.contains("Gita")).unwrap();
        assert!(gita.on_date((2026, 11, 30)));
        assert!(gita.on_date((2026, 12, 1)));
        assert!(!gita.on_date((2026, 12, 2)));
    }

    #[test]
    fn calendar_from_file() {
        let dir = std::env::temp_dir().join(format!("nova-ai-cal-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let ics = dir.join("test.ics");
        fs::write(&ics, FIXTURE).unwrap();
        let cal = Calendar::new(
            String::new(),
            ics.to_string_lossy().to_string(),
            dir.join("cache.ics"),
        );
        let (today, _) = cal.events_on((2026, 11, 27)).unwrap();
        assert_eq!(today.len(), 2); // lezione + palestra
        let (hits, _) = cal.search("mamma").unwrap();
        assert_eq!(hits.len(), 1);
        assert!(format_events("oggi", &today).contains("Lezione di inglese"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unconfigured_is_honest() {
        let cal = Calendar::new(
            String::new(),
            String::new(),
            std::env::temp_dir().join("nova-ai-cal-none.ics"),
        );
        assert!(!cal.configured());
        assert!(cal.events().is_err());
    }
}
