//! Deployment rpm-ostree: parsing di `rpm-ostree status --json` e
//! mapping checksum → indice per `ostree admin pin`.
//!
//! Fatti verificati 26 nov 2026 (man rpm-ostree / Fedora Magazine /
//! Bazzite docs):
//! - `rpm-ostree status --json` ha per deployment: `checksum`, `osname`,
//!   `version`, `timestamp` (unix, uint64), `booted`, `pinned`, `staged`,
//!   `origin` (l'immagine OCI da cui proviene).
//! - `rpm-ostree deploy <checksum>` rende predefinito un deployment.
//! - `rpm-ostree rollback` torna al deployment precedente.
//! - NON esiste `rpm-ostree pin`: il pin è `ostree admin pin <indice>`
//!   (0 = primo deployment) e `ostree admin pin --unpin <indice>`.

use serde::Deserialize;

/// Un deployment rpm-ostree (= uno snapshot del sistema).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Deployment {
    /// Checksum ostree completo (64 hex).
    #[serde(default)]
    pub checksum: String,
    /// Nome OS, es. "fedora".
    #[serde(default)]
    pub osname: String,
    /// Versione, es. "45.20261126.0" (può mancare).
    #[serde(default)]
    pub version: Option<String>,
    /// Timestamp unix del commit.
    #[serde(default)]
    pub timestamp: u64,
    /// `true` se è il deployment avviato ora.
    #[serde(default)]
    pub booted: bool,
    /// `true` se è pinnato (protetto dalla pulizia automatica).
    #[serde(default)]
    pub pinned: bool,
    /// `true` se è staged (applicato al prossimo reboot).
    #[serde(default)]
    pub staged: bool,
    /// Origine, es. "ghcr.io/giulianogramaglia69-boop/novaos-niri:45".
    #[serde(default)]
    pub origin: Option<String>,
}

impl Deployment {
    /// Primi 12 caratteri del checksum (come li mostra rpm-ostree).
    pub fn short_checksum(&self) -> &str {
        &self.checksum[..self.checksum.len().min(12)]
    }

    /// Data/ora locali leggibili, es. "26 nov 2026, 18:32".
    pub fn date_string(&self) -> String {
        crate::dates::format_timestamp(self.timestamp)
    }

    /// Etichetta mostrata in lista: versione se c'è, altrimenti checksum.
    pub fn label(&self) -> String {
        self.version
            .clone()
            .unwrap_or_else(|| self.short_checksum().to_string())
    }
}

#[derive(Debug, Deserialize)]
struct StatusJson {
    #[serde(default)]
    deployments: Vec<Deployment>,
}

/// Parsea l'output di `rpm-ostree status --json` (testabile senza sistema).
pub fn status_from_json(text: &str) -> Result<Vec<Deployment>, String> {
    let parsed: StatusJson =
        serde_json::from_str(text).map_err(|e| format!("JSON di rpm-ostree non valido: {e}"))?;
    Ok(parsed.deployments)
}

/// Esegue `rpm-ostree status --json` e ritorna i deployment.
pub fn status() -> Result<Vec<Deployment>, String> {
    let out = crate::system::run(&["rpm-ostree", "status", "--json"])?;
    if !out.success() {
        return Err(format!(
            "rpm-ostree status fallito (codice {:?}): {}",
            out.code,
            out.stderr.trim()
        ));
    }
    status_from_json(&out.stdout)
}

/// Il deployment avviato ora, se presente.
pub fn booted(deployments: &[Deployment]) -> Option<&Deployment> {
    deployments.iter().find(|d| d.booted)
}

/// Il deployment staged (in attesa di reboot), se presente.
pub fn staged(deployments: &[Deployment]) -> Option<&Deployment> {
    deployments.iter().find(|d| d.staged && !d.booted)
}

/// Indice ostree di un deployment, per `ostree admin pin <indice>`.
///
/// Parsea l'output di `ostree admin status`: una riga per deployment,
/// nella forma `* <osname> <checksum> (<versione>)` con `*` sul booted
/// (e un eventuale suffisso `(pinned)`). L'indice è l'ordine delle righe
/// (0 = primo deployment elencato).
///
/// TODO(verifica-su-fedora): il formato esatto di `ostree admin status`
/// va confermato sul sistema reale; se il parse non trova il checksum,
/// il pin va disabilitato in UI con spiegazione (mai indovinare l'indice).
pub fn pin_index_from_admin_status(text: &str, checksum: &str) -> Option<usize> {
    for (idx, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Riga tipo: "* fedora 8a7f... (45.20261126.0)" oppure
        // "  fedora 3b2c... (45.20261125.0) (pinned)".
        let line = line.strip_prefix('*').unwrap_or(line).trim();
        let mut parts = line.split_whitespace();
        let _osname = parts.next()?;
        let listed = parts.next()?;
        if listed.starts_with(checksum) || checksum.starts_with(listed) {
            return Some(idx);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_JSON: &str = r#"{
        "deployments": [
            {
                "checksum": "8a7f6e5d4c3b2a19080706050403020100ffeeddccbbaa99887766554433221100",
                "osname": "fedora",
                "version": "45.20261126.0",
                "timestamp": 1764100000,
                "booted": true,
                "pinned": false,
                "staged": false,
                "origin": "ghcr.io/giulianogramaglia69-boop/novaos-niri:45"
            },
            {
                "checksum": "3b2c1d0e9f8a7b6c5d4e3f2a1b0c9d8e7f6a5b4c3d2e1f0a9b8c7d6e5f4a3b2c1",
                "osname": "fedora",
                "version": "45.20261125.0",
                "timestamp": 1732560000,
                "booted": false,
                "pinned": true,
                "staged": false,
                "origin": "ghcr.io/giulianogramaglia69-boop/novaos-niri:45"
            }
        ]
    }"#;

    #[test]
    fn parses_sample_json() {
        let deps = status_from_json(SAMPLE_JSON).unwrap();
        assert_eq!(deps.len(), 2);
        assert!(deps[0].booted);
        assert!(!deps[0].pinned);
        assert_eq!(deps[0].version.as_deref(), Some("45.20261126.0"));
        assert_eq!(deps[0].short_checksum(), "8a7f6e5d4c3b");
        // L'ora è locale (dipende dal fuso orario della macchina che esegue
        // i test: il valore esatto UTC è coperto in dates.rs). Qui si
        // verifica solo che date_string() formatti il timestamp del
        // deployment, non un valore assoluto.
        assert_eq!(
            deps[0].date_string(),
            crate::dates::format_timestamp(1_764_100_000)
        );
        assert!(deps[1].pinned);
        assert_eq!(booted(&deps).unwrap().checksum, deps[0].checksum);
    }

    #[test]
    fn invalid_json_errors() {
        assert!(status_from_json("non json").is_err());
        assert!(status_from_json("{}").is_ok()); // nessun deployment: lista vuota
        assert!(status_from_json("{}").unwrap().is_empty());
    }

    #[test]
    fn pin_index_parses_admin_status() {
        let text = "* fedora 8a7f6e5d4c3b2a19080706050403020100ffeeddccbbaa99887766554433221100 (45.20261126.0)\n  fedora 3b2c1d0e9f8a7b6c5d4e3f2a1b0c9d8e7f6a5b4c3d2e1f0a9b8c7d6e5f4a3b2c1 (45.20261125.0) (pinned)\n";
        assert_eq!(
            pin_index_from_admin_status(
                text,
                "8a7f6e5d4c3b2a19080706050403020100ffeeddccbbaa99887766554433221100"
            ),
            Some(0)
        );
        assert_eq!(
            pin_index_from_admin_status(
                text,
                "3b2c1d0e9f8a7b6c5d4e3f2a1b0c9d8e7f6a5b4c3d2e1f0a9b8c7d6e5f4a3b2c1"
            ),
            Some(1)
        );
        assert_eq!(
            pin_index_from_admin_status(
                text,
                "0000000000000000000000000000000000000000000000000000000000000000"
            ),
            None
        );
    }

    #[test]
    fn short_checksum_short_input() {
        let d = Deployment {
            checksum: "abc".to_string(),
            osname: "fedora".to_string(),
            version: None,
            timestamp: 0,
            booted: false,
            pinned: false,
            staged: false,
            origin: None,
        };
        assert_eq!(d.short_checksum(), "abc");
        assert_eq!(d.label(), "abc");
    }
}
