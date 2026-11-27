//! Verdetti dei controlli di aggiornamento: parsing puro, testabile
//! senza UI, senza comandi di sistema e senza rete.
//!
//! Fatti verificati 26 nov 2026 (non indovinare oltre):
//! - `rpm-ostree upgrade --check`: exit 77 = nessun aggiornamento
//!   disponibile (documentato nel man), 0 = aggiornamenti disponibili.
//! - `bootc upgrade --check`: uupd stabilisce "nessun update"
//!   cercando la stringa "No changes in:" nell'output; l'exit code
//!   di bootc NON è documentato e non viene usato qui.
//! - `flatpak remote-ls --updates` elenca gli aggiornamenti Flatpak
//!   senza applicarli (man flatpak-remote-ls).
//!
//! TODO(verifica-su-fedora): il formato esatto di
//! `flatpak remote-ls --updates` e l'output di `bootc upgrade --check`
//! quando ci sono aggiornamenti vanno confermati sul sistema reale;
//! se il parse non torna, i verdetti diventano "Non determinabile"
//! invece di indovinare.

use novaos_common::system::CmdOutput;

/// Verdetto di una sorgente di aggiornamenti.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Aggiornamenti disponibili.
    Available,
    /// Nessun aggiornamento disponibile.
    UpToDate,
    /// Non si è potuto stabilire (con il motivo, mostrato in UI).
    Unknown(String),
}

impl Verdict {
    /// Etichetta breve per la UI.
    pub fn label(&self) -> &'static str {
        match self {
            Verdict::Available => "Aggiornamenti disponibili",
            Verdict::UpToDate => "Sistema aggiornato",
            Verdict::Unknown(_) => "Non determinabile",
        }
    }

    /// Etichetta + dettaglio (il motivo, per `Unknown`).
    pub fn describe(&self) -> String {
        match self {
            Verdict::Available => "Aggiornamenti disponibili".to_string(),
            Verdict::UpToDate => "Sistema aggiornato".to_string(),
            Verdict::Unknown(reason) => format!("Non determinabile: {reason}"),
        }
    }

    pub fn is_available(&self) -> bool {
        matches!(self, Verdict::Available)
    }
}

/// Verdetto di `rpm-ostree upgrade --check` dall'output catturato.
/// Man rpm-ostree: exit 77 = nessun aggiornamento, 0 = disponibili.
pub fn verdict_rpm_ostree(out: &CmdOutput) -> Verdict {
    match out.code {
        Some(77) => Verdict::UpToDate,
        Some(0) => Verdict::Available,
        Some(code) => {
            let detail = first_non_empty(&[&out.stderr, &out.stdout]);
            Verdict::Unknown(format!(
                "codice di uscita {code}{}",
                detail.map_or(String::new(), |d| format!(": {d}"))
            ))
        }
        None => Verdict::Unknown("processo terminato da un segnale".to_string()),
    }
}

/// Verdetto di `bootc upgrade --check` dall'output combinato
/// (stdout + stderr). Solo la stringa "No changes in:" conta:
/// l'exit code di bootc non è documentato e viene ignorato.
pub fn verdict_bootc(combined: &str) -> Verdict {
    if combined.contains("No changes in:") {
        return Verdict::UpToDate;
    }
    let trimmed = combined.trim();
    if trimmed.is_empty() {
        return Verdict::Unknown("output vuoto".to_string());
    }
    if trimmed.contains("No image source specified") {
        return Verdict::Unknown(
            "nessuna sorgente immagine configurata (No image source specified)".to_string(),
        );
    }
    Verdict::Available
}

/// Parsea `flatpak remote-ls --updates`: una riga per app, ID = primo
/// token in notazione DNS inversa (almeno due punti, es.
/// `org.mozilla.firefox`). La riga di intestazione viene saltata.
/// Se il formato non torna, le righe non riconosciute vengono
/// ignorate invece di inventare ID.
pub fn parse_flatpak_updates(output: &str) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Riga di intestazione: "Name  Application ID  Version ...".
        if line.starts_with("Name") && line.contains("Application ID") {
            continue;
        }
        let tokens: Vec<&str> = line.split_whitespace().collect();
        let id = tokens
            .iter()
            .copied()
            .find(|t| looks_like_app_id(t))
            .or_else(|| tokens.get(1).copied());
        if let Some(id) = id {
            let id = id.to_string();
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
    }
    ids
}

/// `true` se il token assomiglia a un ID Flatpak (DNS inversa con
/// almeno due punti, solo caratteri ammessi negli ID).
fn looks_like_app_id(token: &str) -> bool {
    token.chars().filter(|c| *c == '.').count() >= 2
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

/// Prima riga non vuota tra le stringhe date (già trimmata).
fn first_non_empty<'a>(texts: &[&'a str]) -> Option<&'a str> {
    texts
        .iter()
        .copied()
        .filter_map(|t| {
            let t = t.trim();
            if t.is_empty() {
                None
            } else {
                Some(t.lines().next().unwrap_or(t))
            }
        })
        .next()
}

/// Esito del controllo Flatpak per un ambito (`--system` / `--user`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlatpakCheck {
    /// ID delle app con aggiornamenti disponibili.
    pub apps: Vec<String>,
    /// Errore del comando (`None` se il comando è riuscito).
    pub error: Option<String>,
}

impl FlatpakCheck {
    pub fn verdict(&self) -> Verdict {
        if let Some(error) = &self.error {
            Verdict::Unknown(error.clone())
        } else if self.apps.is_empty() {
            Verdict::UpToDate
        } else {
            Verdict::Available
        }
    }

    /// Dettaglio leggibile: lista app, "nessun aggiornamento", o errore.
    pub fn detail(&self) -> String {
        if let Some(error) = &self.error {
            error.clone()
        } else if self.apps.is_empty() {
            "nessun aggiornamento".to_string()
        } else {
            format!(
                "{} aggiornamento/i: {}",
                self.apps.len(),
                self.apps.join(", ")
            )
        }
    }
}

/// Esito completo del controllo aggiornamenti (una voce per sorgente).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckReport {
    pub rpm_ostree: Verdict,
    pub bootc: Verdict,
    pub flatpak_system: FlatpakCheck,
    pub flatpak_user: FlatpakCheck,
}

impl CheckReport {
    /// `true` se c'è un aggiornamento di sistema (rpm-ostree o bootc).
    pub fn system_available(&self) -> bool {
        self.rpm_ostree.is_available() || self.bootc.is_available()
    }

    /// `true` se c'è almeno un aggiornamento Flatpak.
    pub fn flatpak_available(&self) -> bool {
        self.flatpak_system.verdict().is_available() || self.flatpak_user.verdict().is_available()
    }

    /// Verdetto combinato: disponibili se almeno una sorgente ne ha,
    /// altrimenti non determinabile se almeno una sorgente è ignota,
    /// altrimenti aggiornato.
    pub fn combined(&self) -> Verdict {
        let verdicts = [
            self.rpm_ostree.clone(),
            self.bootc.clone(),
            self.flatpak_system.verdict(),
            self.flatpak_user.verdict(),
        ];
        if verdicts.iter().any(Verdict::is_available) {
            return Verdict::Available;
        }
        if let Some(unknown) = verdicts.iter().find(|v| matches!(v, Verdict::Unknown(_))) {
            return unknown.clone();
        }
        Verdict::UpToDate
    }

    /// Dettaglio del verdetto combinato: da dove arrivano gli
    /// aggiornamenti, oppure i motivi di indeterminatezza.
    pub fn combined_detail(&self) -> String {
        let mut sources: Vec<&str> = Vec::new();
        if self.rpm_ostree.is_available() {
            sources.push("rpm-ostree");
        }
        if self.bootc.is_available() {
            sources.push("bootc");
        }
        if self.flatpak_system.verdict().is_available() {
            sources.push("Flatpak (sistema)");
        }
        if self.flatpak_user.verdict().is_available() {
            sources.push("Flatpak (utente)");
        }
        if !sources.is_empty() {
            return format!("Aggiornamenti da: {}", sources.join(", "));
        }
        let mut reasons: Vec<String> = Vec::new();
        for verdict in [
            &self.rpm_ostree,
            &self.bootc,
            &self.flatpak_system.verdict(),
            &self.flatpak_user.verdict(),
        ] {
            if let Verdict::Unknown(reason) = verdict {
                reasons.push(reason.clone());
            }
        }
        if !reasons.is_empty() {
            return format!("Non determinabile: {}", reasons.join("; "));
        }
        "Tutto aggiornato.".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd_output(code: Option<i32>, stdout: &str, stderr: &str) -> CmdOutput {
        CmdOutput {
            code,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
        }
    }

    #[test]
    fn rpm_ostree_77_is_up_to_date() {
        assert_eq!(
            verdict_rpm_ostree(&cmd_output(Some(77), "", "")),
            Verdict::UpToDate
        );
    }

    #[test]
    fn rpm_ostree_0_is_available() {
        assert_eq!(
            verdict_rpm_ostree(&cmd_output(Some(0), "Update available", "")),
            Verdict::Available
        );
    }

    #[test]
    fn rpm_ostree_other_code_is_unknown() {
        let verdict = verdict_rpm_ostree(&cmd_output(Some(1), "", "errore strano"));
        assert!(matches!(verdict, Verdict::Unknown(_)));
        assert!(verdict.describe().contains("codice di uscita 1"));
        assert!(verdict.describe().contains("errore strano"));
    }

    #[test]
    fn rpm_ostree_signal_is_unknown() {
        assert!(matches!(
            verdict_rpm_ostree(&cmd_output(None, "", "")),
            Verdict::Unknown(_)
        ));
    }

    #[test]
    fn bootc_no_changes_is_up_to_date() {
        assert_eq!(
            verdict_bootc("No changes in: ghcr.io/giulianogramaglia69-boop/novaos-niri:45\n"),
            Verdict::UpToDate
        );
    }

    #[test]
    fn bootc_without_marker_is_available() {
        // La stringa manca: uupd lo leggerebbe come "update presente".
        // L'exit code non conta: anche con 0, senza marker è Available.
        assert_eq!(
            verdict_bootc("Image updated to a newer version\n"),
            Verdict::Available
        );
    }

    #[test]
    fn bootc_empty_output_is_unknown() {
        assert!(matches!(verdict_bootc("  \n"), Verdict::Unknown(_)));
    }

    #[test]
    fn bootc_no_image_source_is_unknown() {
        let verdict = verdict_bootc("Error: No image source specified\n");
        assert!(matches!(verdict, Verdict::Unknown(_)));
        assert!(verdict.describe().contains("sorgente immagine"));
    }

    #[test]
    fn flatpak_parses_remote_ls() {
        let output = "Name                   Application ID                  Version Branch Remote\n\
                      Firefox                org.mozilla.firefox             146.0   stable flathub\n\
                      GNOME Calculator       org.gnome.Calculator            48.0    stable flathub\n";
        assert_eq!(
            parse_flatpak_updates(output),
            vec![
                "org.mozilla.firefox".to_string(),
                "org.gnome.Calculator".to_string()
            ]
        );
    }

    #[test]
    fn flatpak_header_only_is_empty() {
        let output =
            "Name                   Application ID                  Version Branch Remote\n";
        assert!(parse_flatpak_updates(output).is_empty());
    }

    #[test]
    fn flatpak_empty_output_is_empty() {
        assert!(parse_flatpak_updates("").is_empty());
        assert!(parse_flatpak_updates("\n\n").is_empty());
    }

    #[test]
    fn flatpak_dedupes_ids() {
        let output = "Firefox org.mozilla.firefox 146.0 stable flathub\n\
                      Firefox org.mozilla.firefox 146.0 stable flathub\n";
        assert_eq!(
            parse_flatpak_updates(output),
            vec!["org.mozilla.firefox".to_string()]
        );
    }

    #[test]
    fn flatpak_check_verdicts() {
        let empty = FlatpakCheck {
            apps: vec![],
            error: None,
        };
        assert_eq!(empty.verdict(), Verdict::UpToDate);
        assert_eq!(empty.detail(), "nessun aggiornamento");

        let some = FlatpakCheck {
            apps: vec!["org.mozilla.firefox".to_string()],
            error: None,
        };
        assert_eq!(some.verdict(), Verdict::Available);
        assert!(some.detail().contains("org.mozilla.firefox"));

        let failed = FlatpakCheck {
            apps: vec![],
            error: Some("flatpak non installato".to_string()),
        };
        assert_eq!(
            failed.verdict(),
            Verdict::Unknown("flatpak non installato".to_string())
        );
    }

    fn report_all_up_to_date() -> CheckReport {
        CheckReport {
            rpm_ostree: Verdict::UpToDate,
            bootc: Verdict::UpToDate,
            flatpak_system: FlatpakCheck {
                apps: vec![],
                error: None,
            },
            flatpak_user: FlatpakCheck {
                apps: vec![],
                error: None,
            },
        }
    }

    #[test]
    fn combined_all_up_to_date() {
        let report = report_all_up_to_date();
        assert_eq!(report.combined(), Verdict::UpToDate);
        assert!(!report.system_available());
        assert!(!report.flatpak_available());
        assert_eq!(report.combined_detail(), "Tutto aggiornato.");
    }

    #[test]
    fn combined_available_wins() {
        let mut report = report_all_up_to_date();
        report.flatpak_user.apps = vec!["org.mozilla.firefox".to_string()];
        assert_eq!(report.combined(), Verdict::Available);
        assert!(report.flatpak_available());
        assert!(!report.system_available());
        assert!(report.combined_detail().contains("Flatpak (utente)"));
    }

    #[test]
    fn combined_unknown_when_nothing_available() {
        let mut report = report_all_up_to_date();
        report.bootc = Verdict::Unknown("nessuna sorgente immagine".to_string());
        assert_eq!(
            report.combined(),
            Verdict::Unknown("nessuna sorgente immagine".to_string())
        );
        assert!(report.combined_detail().contains("sorgente immagine"));
    }

    #[test]
    fn verdict_labels() {
        assert_eq!(Verdict::Available.label(), "Aggiornamenti disponibili");
        assert_eq!(Verdict::UpToDate.label(), "Sistema aggiornato");
        assert_eq!(Verdict::Unknown("x".into()).label(), "Non determinabile");
        assert!(
            Verdict::Unknown("motivo".into())
                .describe()
                .contains("motivo")
        );
    }
}
