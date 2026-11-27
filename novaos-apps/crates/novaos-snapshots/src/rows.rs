//! Logica di presentazione delle righe deployment: etichette, badge e
//! disponibilità delle azioni.
//!
//! Niente codice UI qui: tutto è unit-testabile con `cargo test`
//! senza GTK. La UI in `main.rs` si limita a tradurre queste strutture
//! in widget.

use novaos_common::ostree::{Deployment, pin_index_from_admin_status};

/// Badge mostrato su una riga deployment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Badge {
    /// Il deployment avviato ora.
    Booted,
    /// Applicato al prossimo reboot (staged).
    Staged,
    /// Pinnato (protetto dalla pulizia automatica).
    Pinned,
}

impl Badge {
    /// Etichetta italiana del badge.
    pub fn label(self) -> &'static str {
        match self {
            Badge::Booted => "Avviato",
            Badge::Staged => "In attesa di riavvio",
            Badge::Pinned => "Pinnato",
        }
    }
}

/// Vista pronta per la UI di un singolo deployment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowView {
    /// Etichetta principale: versione se presente, altrimenti checksum corto.
    pub title: String,
    /// Riga secondaria: data, checksum corto e origine (se presente).
    pub subtitle: String,
    /// Badge da mostrare, in ordine di importanza.
    pub badges: Vec<Badge>,
    /// `true` se ha senso il pulsante "Avvia" (mai per il booted).
    pub can_deploy: bool,
    /// Etichetta del pulsante pin: "Pin" oppure "Rimuovi pin".
    pub pin_label: &'static str,
}

/// Calcola la vista di un deployment.
pub fn row_view(deployment: &Deployment) -> RowView {
    let mut badges = Vec::new();
    if deployment.booted {
        badges.push(Badge::Booted);
    }
    if deployment.staged {
        badges.push(Badge::Staged);
    }
    if deployment.pinned {
        badges.push(Badge::Pinned);
    }
    let subtitle = match deployment.origin.as_deref() {
        Some(origin) if !origin.is_empty() => format!(
            "{} · {} · {origin}",
            deployment.date_string(),
            deployment.short_checksum()
        ),
        _ => format!(
            "{} · {}",
            deployment.date_string(),
            deployment.short_checksum()
        ),
    };
    RowView {
        title: deployment.label(),
        subtitle,
        badges,
        // "Avvia" non ha senso per il deployment già avviato.
        can_deploy: !deployment.booted,
        pin_label: if deployment.pinned {
            "Rimuovi pin"
        } else {
            "Pin"
        },
    }
}

/// `true` se il rollback ha un deployment precedente a cui tornare
/// (serve almeno un secondo deployment oltre a quello avviato).
pub fn can_rollback(deployments: &[Deployment]) -> bool {
    deployments.len() >= 2
}

/// Indice ostree di un deployment, per `ostree admin pin <indice>`.
///
/// Delega a `novaos-common`: parsa l'output di `ostree admin status`.
/// `None` significa "indice non trovato": la UI deve disabilitare il
/// pin con spiegazione e MAI indovinare l'indice.
pub fn pin_index(admin_status: &str, checksum: &str) -> Option<usize> {
    pin_index_from_admin_status(admin_status, checksum)
}

/// Messaggio mostrato quando `rpm-ostree status` non è disponibile
/// (rpm-ostree assente o errore): spiega che lo strumento funziona
/// solo su NovaOS / Fedora Atomic e riporta il dettaglio tecnico.
pub fn atomic_error_message(detail: &str) -> String {
    format!("Questo strumento funziona solo su NovaOS / Fedora Atomic.\n{detail}",)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deployment(
        checksum: &str,
        version: Option<&str>,
        booted: bool,
        pinned: bool,
        staged: bool,
    ) -> Deployment {
        Deployment {
            checksum: checksum.to_string(),
            osname: "fedora".to_string(),
            version: version.map(str::to_string),
            timestamp: 1_764_100_000,
            booted,
            pinned,
            staged,
            origin: Some("ghcr.io/giulianogramaglia69-boop/novaos-niri:45".to_string()),
        }
    }

    const CHECKSUM_A: &str = "8a7f6e5d4c3b2a19080706050403020100ffeeddccbbaa99887766554433221100";
    const CHECKSUM_B: &str = "3b2c1d0e9f8a7b6c5d4e3f2a1b0c9d8e7f6a5b4c3d2e1f0a9b8c7d6e5f4a3b2c1";

    #[test]
    fn booted_row_has_badge_and_no_deploy() {
        let view = row_view(&deployment(
            CHECKSUM_A,
            Some("45.20261126.0"),
            true,
            false,
            false,
        ));
        assert_eq!(view.title, "45.20261126.0");
        assert_eq!(view.badges, vec![Badge::Booted]);
        assert!(!view.can_deploy);
        assert_eq!(view.pin_label, "Pin");
        assert!(view.subtitle.contains("8a7f6e5d4c3b"));
        assert!(view.subtitle.contains("novaos-niri:45"));
    }

    #[test]
    fn staged_and_pinned_badges() {
        let view = row_view(&deployment(
            CHECKSUM_B,
            Some("45.20261125.0"),
            false,
            true,
            true,
        ));
        assert_eq!(view.badges, vec![Badge::Staged, Badge::Pinned]);
        assert!(view.can_deploy);
        assert_eq!(view.pin_label, "Rimuovi pin");
    }

    #[test]
    fn version_missing_falls_back_to_short_checksum() {
        let view = row_view(&deployment(CHECKSUM_A, None, false, false, false));
        assert_eq!(view.title, "8a7f6e5d4c3b");
    }

    #[test]
    fn subtitle_without_origin_has_no_trailing_separator() {
        let mut dep = deployment(CHECKSUM_A, Some("45.20261126.0"), false, false, false);
        dep.origin = None;
        let view = row_view(&dep);
        assert!(!view.subtitle.ends_with('·'));
        assert!(view.subtitle.contains("8a7f6e5d4c3b"));
    }

    #[test]
    fn rollback_needs_two_deployments() {
        assert!(!can_rollback(&[]));
        assert!(!can_rollback(&[deployment(
            CHECKSUM_A,
            Some("45.20261126.0"),
            true,
            false,
            false
        )]));
        assert!(can_rollback(&[
            deployment(CHECKSUM_A, Some("45.20261126.0"), true, false, false),
            deployment(CHECKSUM_B, Some("45.20261125.0"), false, false, false),
        ]));
    }

    #[test]
    fn pin_index_delegates_to_common() {
        let text = format!(
            "* fedora {CHECKSUM_A} (45.20261126.0)\n  fedora {CHECKSUM_B} (45.20261125.0) (pinned)\n"
        );
        assert_eq!(pin_index(&text, CHECKSUM_A), Some(0));
        assert_eq!(pin_index(&text, CHECKSUM_B), Some(1));
        assert_eq!(pin_index(&text, "00"), None);
    }

    #[test]
    fn atomic_error_message_keeps_detail() {
        let msg = atomic_error_message("rpm-ostree: comando non trovato");
        assert!(msg.contains("solo su NovaOS / Fedora Atomic"));
        assert!(msg.contains("rpm-ostree: comando non trovato"));
    }
}
