//! Persistent multi-step plans: the agent can break a goal into
//! steps, work through them over several turns, and resume after a
//! restart. Stored as one JSON file per plan under
//! `~/.local/share/nova-ai/plans/<id>.json`.
//!
//! A plan has a title, steps with a done flag, and an updated
//! timestamp (used to pick the "active" plan: the most recently
//! updated unfinished one). Plans never mutate the system by
//! themselves — the tools the plan's steps call still go through
//! the normal confirmation flow.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// One step of a plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanStep {
    pub text: String,
    #[serde(default)]
    pub done: bool,
}

/// A plan on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    /// Short id: milliseconds since epoch + a counter suffix.
    pub id: String,
    pub title: String,
    pub steps: Vec<PlanStep>,
    /// Unix seconds of the last update (for "most recent first").
    pub updated: u64,
}

impl Plan {
    /// Short progress line for the chat, e.g.
    /// `Piano "Pulire foto" (2/4): ✓ Esporta ✓ Controlla ○ Cancella ○ Svuota cestino`.
    pub fn progress_line(&self) -> String {
        let done = self.steps.iter().filter(|s| s.done).count();
        let total = self.steps.len();
        let mut line = format!("Piano \"{}\" ({done}/{total}):", self.title);
        for s in &self.steps {
            line.push_str(if s.done { " ✓ " } else { " ○ " });
            line.push_str(&s.text);
        }
        line
    }

    pub fn is_done(&self) -> bool {
        !self.steps.is_empty() && self.steps.iter().all(|s| s.done)
    }

    pub fn done_count(&self) -> usize {
        self.steps.iter().filter(|s| s.done).count()
    }
}

/// The plans store (one directory).
pub struct PlanStore {
    base_dir: PathBuf,
}

impl PlanStore {
    /// Default location: `~/.local/share/nova-ai/plans`.
    pub fn default_dir() -> PathBuf {
        crate::data_dir().join("plans")
    }

    pub fn new(base_dir: PathBuf) -> PlanStore {
        PlanStore { base_dir }
    }

    pub fn load_default() -> PlanStore {
        PlanStore::new(Self::default_dir())
    }

    fn path_for(&self, id: &str) -> PathBuf {
        // The id is ours (timestamp-based); reject path tricks anyway.
        let safe: String = id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        self.base_dir.join(format!("{safe}.json"))
    }

    fn now_secs() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    fn new_id() -> String {
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        format!("{ms}")
    }

    /// Create a plan. Empty title or no steps is refused.
    pub fn create(&self, title: &str, steps: &[String]) -> Result<Plan, String> {
        let title = title.trim();
        if title.is_empty() {
            return Err("il piano ha bisogno di un titolo".to_string());
        }
        let steps: Vec<PlanStep> = steps
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| PlanStep {
                text: s.to_string(),
                done: false,
            })
            .collect();
        if steps.is_empty() {
            return Err("il piano ha bisogno di almeno un passo".to_string());
        }
        let plan = Plan {
            id: Self::new_id(),
            title: title.to_string(),
            steps,
            updated: Self::now_secs(),
        };
        self.save(&plan)?;
        Ok(plan)
    }

    pub fn save(&self, plan: &Plan) -> Result<(), String> {
        fs::create_dir_all(&self.base_dir)
            .map_err(|e| format!("impossibile creare i piani: {e}"))?;
        let text = serde_json::to_string_pretty(plan).map_err(|e| format!("{e}"))?;
        fs::write(self.path_for(&plan.id), text)
            .map_err(|e| format!("impossibile salvare il piano: {e}"))?;
        Ok(())
    }

    pub fn load(&self, id: &str) -> Result<Plan, String> {
        let text =
            fs::read_to_string(self.path_for(id)).map_err(|_| format!("piano {id} non trovato"))?;
        serde_json::from_str(&text).map_err(|e| format!("piano non leggibile: {e}"))
    }

    /// All plans, most recently updated first.
    pub fn list(&self) -> Vec<Plan> {
        let Ok(rd) = fs::read_dir(&self.base_dir) else {
            return Vec::new();
        };
        let mut plans: Vec<Plan> = rd
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| {
                fs::read_to_string(e.path())
                    .ok()
                    .and_then(|t| serde_json::from_str(&t).ok())
            })
            .collect();
        plans.sort_by_key(|p| std::cmp::Reverse(p.updated));
        plans
    }

    /// Unfinished plans, most recently updated first.
    pub fn unfinished(&self) -> Vec<Plan> {
        self.list().into_iter().filter(|p| !p.is_done()).collect()
    }

    /// The active plan: the most recently updated unfinished one.
    pub fn active(&self) -> Option<Plan> {
        self.unfinished().into_iter().next()
    }

    /// Mark one step done / not done (0-based index). Returns the
    /// updated plan.
    pub fn set_step(&self, id: &str, index: usize, done: bool) -> Result<Plan, String> {
        let mut plan = self.load(id)?;
        let n = plan.steps.len();
        let step = plan
            .steps
            .get_mut(index)
            .ok_or_else(|| format!("il piano ha {n} passi, indice {index} non valido"))?;
        step.done = done;
        plan.updated = Self::now_secs();
        self.save(&plan)?;
        Ok(plan)
    }

    /// Mark the whole plan done (all steps).
    pub fn complete(&self, id: &str) -> Result<Plan, String> {
        let mut plan = self.load(id)?;
        for s in &mut plan.steps {
            s.done = true;
        }
        plan.updated = Self::now_secs();
        self.save(&plan)?;
        Ok(plan)
    }

    /// Delete a plan file.
    pub fn delete(&self, id: &str) -> Result<(), String> {
        fs::remove_file(self.path_for(id))
            .map_err(|e| format!("impossibile cancellare il piano: {e}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("nova-ai-plan-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn create_list_update_complete() {
        let dir = tmp_dir("flow");
        let store = PlanStore::new(dir.clone());
        assert!(store.list().is_empty());
        assert!(store.active().is_none());

        let plan = store
            .create(
                "Organizzare foto",
                &[
                    "Esporta".to_string(),
                    "Controlla".to_string(),
                    "Cancella".to_string(),
                ],
            )
            .unwrap();
        assert_eq!(plan.done_count(), 0);
        assert!(!plan.is_done());
        assert!(plan.progress_line().contains("0/3"));

        let active = store.active().unwrap();
        assert_eq!(active.id, plan.id);

        let plan = store.set_step(&plan.id, 0, true).unwrap();
        assert_eq!(plan.done_count(), 1);
        assert!(plan.progress_line().contains("1/3"));
        assert!(plan.progress_line().contains("✓ Esporta"));

        let plan = store.complete(&plan.id).unwrap();
        assert!(plan.is_done());
        assert!(store.active().is_none());
        assert_eq!(store.list().len(), 1);

        store.delete(&plan.id).unwrap();
        assert!(store.list().is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_empty() {
        let dir = tmp_dir("empty");
        let store = PlanStore::new(dir.clone());
        assert!(store.create("", &["x".to_string()]).is_err());
        assert!(store.create("titolo", &[]).is_err());
        assert!(store.set_step("inesistente", 0, true).is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}
