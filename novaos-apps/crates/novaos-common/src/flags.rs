//! Flag persistenti per app: primo avvio visto, pacchetto installato.
//!
//! I flag vivono in `~/.local/share/novaos/<app>/<nome>` (file vuoti:
//! esiste = fatto). `XDG_DATA_HOME` / `XDG_CONFIG_HOME` rispettati.
//! I flag di "visto" del primo avvio vivono invece in
//! `~/.config/novaos/<app>/<nome>` (config, non dati).

use std::path::PathBuf;

/// `~/.local/share/novaos/<app>` (o `$XDG_DATA_HOME/novaos/<app>`).
pub fn data_dir(app: &str) -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from(".local/share"));
    base.join("novaos").join(app)
}

/// `~/.config/novaos/<app>` (o `$XDG_CONFIG_HOME/novaos/<app>`).
pub fn config_dir(app: &str) -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"));
    base.join("novaos").join(app)
}

fn flag_file(dir: PathBuf, name: &str) -> PathBuf {
    dir.join(format!("{name}.done"))
}

/// `true` se il flag dati `<app>/<nome>` esiste.
pub fn is_done(app: &str, name: &str) -> bool {
    flag_file(data_dir(app), name).is_file()
}

/// Crea il flag dati `<app>/<nome>` (crea le directory se servono).
pub fn mark_done(app: &str, name: &str) -> std::io::Result<()> {
    let dir = data_dir(app);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(flag_file(dir, name), b"")
}

/// Rimuove il flag dati `<app>/<nome>` (ok se non esiste).
pub fn clear_done(app: &str, name: &str) {
    let _ = std::fs::remove_file(flag_file(data_dir(app), name));
}

/// `true` se il flag di config `<app>/<nome>` esiste (es. primo avvio visto).
pub fn is_seen(app: &str, name: &str) -> bool {
    flag_file(config_dir(app), name).is_file()
}

/// Crea il flag di config `<app>/<nome>`.
pub fn mark_seen(app: &str, name: &str) -> std::io::Result<()> {
    let dir = config_dir(app);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(flag_file(dir, name), b"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn done_flag_roundtrip() {
        // Isola il test con XDG_DATA_HOME temporanea.
        let tmp = std::env::temp_dir().join(format!("novaos-flags-test-{}", std::process::id()));
        // `set_var` è unsafe in edition 2024 (env condiviso tra thread).
        // Qui è un test single-thread: l'uso è sicuro.
        unsafe {
            std::env::set_var("XDG_DATA_HOME", &tmp);
        }
        let _ = std::fs::remove_dir_all(&tmp);
        assert!(!is_done("setup-test", "codec"));
        mark_done("setup-test", "codec").unwrap();
        assert!(is_done("setup-test", "codec"));
        clear_done("setup-test", "codec");
        assert!(!is_done("setup-test", "codec"));
        let _ = std::fs::remove_dir_all(&tmp);
        unsafe {
            std::env::remove_var("XDG_DATA_HOME");
        }
    }
}
