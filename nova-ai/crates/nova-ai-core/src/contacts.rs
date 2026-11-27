//! Local contacts: a plain vCard file at
//! `~/.local/share/nova-ai/contacts.vcf` (one `FN` / `EMAIL` / `TEL`
//! per card). The user edits it with any editor; NovaAI only reads
//! it. No sync, no cloud — that is the point.

use std::fs;
use std::path::PathBuf;

/// One contact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contact {
    pub name: String,
    pub email: Option<String>,
    pub phone: Option<String>,
}

impl Contact {
    pub fn line(&self) -> String {
        let mut s = self.name.clone();
        if let Some(e) = &self.email {
            s.push_str(&format!(" — {e}"));
        }
        if let Some(p) = &self.phone {
            s.push_str(&format!(" — tel. {p}"));
        }
        s
    }
}

/// Parse vCard text. Only FN, EMAIL and TEL are read (first of each
/// per card); folded lines are unfolded; parameters (`TEL;TYPE=CELL`)
/// are ignored.
pub fn parse_vcf(text: &str) -> Vec<Contact> {
    let mut unfolded: Vec<String> = Vec::new();
    for line in text.lines() {
        if (line.starts_with(' ') || line.starts_with('\t')) && !unfolded.is_empty() {
            let last = unfolded.last_mut().unwrap();
            last.push_str(line.trim_start());
        } else {
            unfolded.push(line.trim_end_matches('\r').to_string());
        }
    }
    let mut contacts = Vec::new();
    let mut cur: Option<Contact> = None;
    for line in &unfolded {
        let (head, value) = match line.split_once(':') {
            Some((h, v)) => (h, v),
            None => continue,
        };
        let name = head.split(';').next().unwrap_or(head).to_ascii_uppercase();
        match name.as_str() {
            "BEGIN" if value.eq_ignore_ascii_case("VCARD") => {
                cur = Some(Contact {
                    name: String::new(),
                    email: None,
                    phone: None,
                });
            }
            "END" if value.eq_ignore_ascii_case("VCARD") => {
                if let Some(c) = cur.take()
                    && !c.name.trim().is_empty()
                {
                    contacts.push(c);
                }
            }
            "FN" => {
                if let Some(c) = cur.as_mut()
                    && c.name.is_empty()
                {
                    c.name = unescape_vcf(value);
                }
            }
            "EMAIL" => {
                if let Some(c) = cur.as_mut()
                    && c.email.is_none()
                {
                    c.email = Some(value.trim().to_string());
                }
            }
            "TEL" => {
                if let Some(c) = cur.as_mut()
                    && c.phone.is_none()
                {
                    c.phone = Some(value.trim().to_string());
                }
            }
            _ => {}
        }
    }
    contacts
}

fn unescape_vcf(s: &str) -> String {
    s.replace("\\,", ",")
        .replace("\\;", ";")
        .replace("\\\\", "\\")
        .replace("\\n", " ")
}

/// Default contacts file: `~/.local/share/nova-ai/contacts.vcf`.
pub fn default_path() -> PathBuf {
    crate::data_dir().join("contacts.vcf")
}

/// Load the contacts file. Missing file = empty list (with a flag so
/// the caller can tell the user where to put contacts).
pub fn load_from(path: &PathBuf) -> (Vec<Contact>, bool) {
    match fs::read_to_string(path) {
        Ok(text) => (parse_vcf(&text), true),
        Err(_) => (Vec::new(), false),
    }
}

/// Search by name (case-insensitive substring). Empty query lists
/// all contacts (at most 20).
pub fn search(contacts: &[Contact], query: &str) -> Vec<Contact> {
    let q = query.trim().to_lowercase();
    contacts
        .iter()
        .filter(|c| q.is_empty() || c.name.to_lowercase().contains(&q))
        .take(20)
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Fabiana Spoto\r\nEMAIL;TYPE=HOME:fabiana@example.it\r\nTEL;TYPE=CELL:+39 333 000 0000\r\nEND:VCARD\r\nBEGIN:VCARD\r\nVERSION:3.0\r\nFN:Antonino Ciulla\r\nEMAIL:ciulla@liceoleonardo.it\r\nEND:VCARD\r\nBEGIN:VCARD\r\nVERSION:3.0\r\nFN:Ilenia\r\nTEL:+39 333 111 1111\r\nEND:VCARD\r\n";

    #[test]
    fn parses_fixture() {
        let cs = parse_vcf(FIXTURE);
        assert_eq!(cs.len(), 3);
        assert_eq!(cs[0].name, "Fabiana Spoto");
        assert_eq!(cs[0].email.as_deref(), Some("fabiana@example.it"));
        assert_eq!(cs[0].phone.as_deref(), Some("+39 333 000 0000"));
        assert_eq!(cs[1].email.as_deref(), Some("ciulla@liceoleonardo.it"));
        assert!(cs[1].phone.is_none());
        assert!(cs[2].email.is_none());

        let hits = search(&cs, "fabiana");
        assert_eq!(hits.len(), 1);
        let hits = search(&cs, "FABIANA");
        assert_eq!(hits.len(), 1);
        let hits = search(&cs, "a");
        assert_eq!(hits.len(), 3);
        assert!(search(&cs, "inesistente").is_empty());
        assert_eq!(search(&cs, "").len(), 3);
        assert!(cs[0].line().contains("fabiana@example.it"));
    }

    #[test]
    fn missing_file_is_empty() {
        let (cs, exists) = load_from(&std::env::temp_dir().join("nova-ai-no-such.vcf"));
        assert!(cs.is_empty());
        assert!(!exists);
    }
}
