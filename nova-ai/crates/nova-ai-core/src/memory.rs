//! Persistent memory for NovaAI: facts the user asked to remember and
//! a summary of the last session. Stored as plain files under
//! `~/.local/share/nova-ai/memory/` so the user can read, edit or
//! delete them with any editor — no database, no embeddings, keyword
//! search only.
//!
//! Layout:
//! - `facts.jsonl`: one JSON object per line: `{"date": "YYYY-MM-DD",
//!   "text": "..."}` (append-only; `forget` rewrites the file).
//! - `summary.txt`: the summary of the last session (one block).

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// One remembered fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fact {
    /// Date the fact was recorded, YYYY-MM-DD.
    pub date: String,
    pub text: String,
}

/// The memory store. All paths are derived from `base_dir`
/// (`~/.local/share/nova-ai/memory` by default; tests pass their own).
pub struct Memory {
    base_dir: PathBuf,
}

impl Memory {
    /// Default location: `~/.local/share/nova-ai/memory`.
    /// Overridable with `NOVA_AI_DATA_DIR` (tests).
    pub fn default_dir() -> PathBuf {
        crate::data_dir().join("memory")
    }

    pub fn new(base_dir: PathBuf) -> Memory {
        Memory { base_dir }
    }

    pub fn load_default() -> Memory {
        Memory::new(Self::default_dir())
    }

    fn facts_path(&self) -> PathBuf {
        self.base_dir.join("facts.jsonl")
    }

    fn summary_path(&self) -> PathBuf {
        self.base_dir.join("summary.txt")
    }

    /// Today's date as YYYY-MM-DD, in local time.
    pub fn today() -> String {
        let (y, m, d) = crate::civil::today_local();
        crate::civil::format_date(y, m, d)
    }

    /// All facts, oldest first. Missing file = no facts.
    pub fn facts(&self) -> Vec<Fact> {
        let Ok(text) = fs::read_to_string(self.facts_path()) else {
            return Vec::new();
        };
        text.lines()
            .filter_map(|l| serde_json::from_str::<Fact>(l).ok())
            .collect()
    }

    /// Append a fact. Empty text is refused.
    pub fn remember(&self, text: &str) -> Result<Fact, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("niente da ricordare (testo vuoto)".to_string());
        }
        fs::create_dir_all(&self.base_dir)
            .map_err(|e| format!("impossibile creare la memoria: {e}"))?;
        let fact = Fact {
            date: Self::today(),
            text: text.to_string(),
        };
        let line = serde_json::to_string(&fact).map_err(|e| format!("{e}"))?;
        use std::io::Write;
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.facts_path())
            .map_err(|e| format!("impossibile scrivere la memoria: {e}"))?;
        writeln!(f, "{line}").map_err(|e| format!("impossibile scrivere la memoria: {e}"))?;
        Ok(fact)
    }

    /// Keyword search (case-insensitive substring) over facts, most
    /// recent first, at most `limit`.
    pub fn recall(&self, query: &str, limit: usize) -> Vec<Fact> {
        let q = query.trim().to_lowercase();
        let mut all = self.facts();
        all.reverse();
        if q.is_empty() {
            return all.into_iter().take(limit).collect();
        }
        all.into_iter()
            .filter(|f| f.text.to_lowercase().contains(&q))
            .take(limit)
            .collect()
    }

    /// Delete every fact matching `query` (case-insensitive
    /// substring). Returns how many were removed.
    pub fn forget(&self, query: &str) -> Result<usize, String> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return Err("dimmi cosa dimenticare".to_string());
        }
        let kept: Vec<Fact> = self
            .facts()
            .into_iter()
            .filter(|f| !f.text.to_lowercase().contains(&q))
            .collect();
        let removed = self.facts().len() - kept.len();
        let mut out = String::new();
        for f in &kept {
            out.push_str(&serde_json::to_string(f).map_err(|e| format!("{e}"))?);
            out.push('\n');
        }
        fs::create_dir_all(&self.base_dir)
            .map_err(|e| format!("impossibile creare la memoria: {e}"))?;
        fs::write(self.facts_path(), out)
            .map_err(|e| format!("impossibile scrivere la memoria: {e}"))?;
        Ok(removed)
    }

    /// The summary of the last session, if any.
    pub fn load_summary(&self) -> Option<String> {
        fs::read_to_string(self.summary_path())
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    /// Replace the last-session summary.
    pub fn save_summary(&self, text: &str) -> Result<(), String> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(());
        }
        fs::create_dir_all(&self.base_dir)
            .map_err(|e| format!("impossibile creare la memoria: {e}"))?;
        fs::write(self.summary_path(), text)
            .map_err(|e| format!("impossibile scrivere la memoria: {e}"))?;
        Ok(())
    }

    /// Text block injected into the agent's system prompt: the main
    /// facts (up to 20, oldest first so the model sees the history)
    /// plus the last session summary. Empty string when there is
    /// nothing to remember.
    pub fn system_prompt_block(&self) -> String {
        let facts = self.facts();
        let summary = self.load_summary();
        if facts.is_empty() && summary.is_none() {
            return String::new();
        }
        let mut out = String::from("\n\nMemoria (cose che l'utente ti ha chiesto di ricordare):");
        let shown: Vec<&Fact> = facts.iter().rev().take(20).collect();
        for f in shown.iter().rev() {
            out.push_str(&format!("\n- [{}] {}", f.date, f.text));
        }
        if let Some(s) = summary {
            out.push_str(&format!("\n\nRiassunto dell'ultima sessione: {s}"));
        }
        out.push_str("\nUsa questi fatti quando sono rilevanti; se l'utente ti corregge, aggiorna la memoria con il tool memory.");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("nova-ai-mem-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn remember_recall_forget_roundtrip() {
        let dir = tmp_dir("roundtrip");
        let mem = Memory::new(dir.clone());
        assert!(mem.facts().is_empty());
        assert!(mem.system_prompt_block().is_empty());

        mem.remember("Il cane si chiama Rex").unwrap();
        mem.remember("La password del Wi-Fi è cambiata").unwrap();
        mem.remember("A Giuliano piace il sushi").unwrap();

        let all = mem.recall("", 10);
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].text, "A Giuliano piace il sushi"); // most recent first

        let hits = mem.recall("cane", 10);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].text.contains("Rex"));

        let hits = mem.recall("CANE", 10); // case-insensitive
        assert_eq!(hits.len(), 1);

        let removed = mem.forget("sushi").unwrap();
        assert_eq!(removed, 1);
        assert_eq!(mem.facts().len(), 2);
        assert!(mem.recall("sushi", 10).is_empty());

        // The block for the system prompt contains the facts.
        let block = mem.system_prompt_block();
        assert!(block.contains("Rex"), "{block}");
        assert!(block.contains("Wi-Fi"), "{block}");
        assert!(!block.contains("sushi"), "{block}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn summary_roundtrip() {
        let dir = tmp_dir("summary");
        let mem = Memory::new(dir.clone());
        assert!(mem.load_summary().is_none());
        mem.save_summary("Abbiamo parlato di NovaOS e dei piani.")
            .unwrap();
        assert_eq!(
            mem.load_summary().as_deref(),
            Some("Abbiamo parlato di NovaOS e dei piani.")
        );
        let block = mem.system_prompt_block();
        assert!(block.contains("NovaOS"), "{block}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn today_is_a_date() {
        let t = Memory::today();
        assert_eq!(t.len(), 10, "{t}");
        assert_eq!(&t[4..5], "-");
        assert_eq!(&t[7..8], "-");
    }
}
