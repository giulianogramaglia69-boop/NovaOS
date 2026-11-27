//! /etc/os-release: parsing e rilevamento del desktop corrente.
//!
//! `build/00-image-info.sh` imposta `VARIANT_ID` al nome immagine,
//! es. `novaos-niri`: il DE corrente è il suffisso dopo `novaos-`.

use std::collections::HashMap;

/// Parsea un file os-release (`CHIAVE=valore`, valore anche tra virgolette).
pub fn parse(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().to_string();
        let mut value = value.trim().to_string();
        if value.len() >= 2 {
            let first = value.as_bytes()[0];
            let last = value.as_bytes()[value.len() - 1];
            if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
                value = value[1..value.len() - 1].to_string();
            }
        }
        map.insert(key, value);
    }
    map
}

/// Legge /etc/os-release.
pub fn read() -> Result<HashMap<String, String>, String> {
    let text = std::fs::read_to_string("/etc/os-release")
        .map_err(|e| format!("lettura di /etc/os-release fallita: {e}"))?;
    Ok(parse(&text))
}

/// Desktop corrente da una mappa os-release (`novaos-niri` → `niri`).
/// `None` se VARIANT_ID manca o non inizia con `novaos-`.
pub fn current_de(map: &HashMap<String, String>) -> Option<String> {
    let variant = map.get("VARIANT_ID")?;
    variant.strip_prefix("novaos-").map(str::to_string)
}

/// Desktop corrente dal sistema live.
pub fn current_de_live() -> Option<String> {
    read().ok().and_then(|m| current_de(&m))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_quoted_and_plain() {
        let m = parse(
            "NAME=Fedora Linux\nPRETTY_NAME=\"NovaOS Niri\"\nVARIANT_ID=novaos-niri\n# commento\n",
        );
        assert_eq!(m.get("NAME").map(String::as_str), Some("Fedora Linux"));
        assert_eq!(
            m.get("PRETTY_NAME").map(String::as_str),
            Some("NovaOS Niri")
        );
        assert_eq!(m.get("VARIANT_ID").map(String::as_str), Some("novaos-niri"));
    }

    #[test]
    fn current_de_variants() {
        let m = parse("VARIANT_ID=novaos-cosmic\n");
        assert_eq!(current_de(&m).as_deref(), Some("cosmic"));
        let m = parse("VARIANT_ID=silverblue\n");
        assert_eq!(current_de(&m), None);
        let m = parse("");
        assert_eq!(current_de(&m), None);
    }
}
