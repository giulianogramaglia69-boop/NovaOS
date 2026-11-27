//! Notifiche desktop via `notify-send` (best effort: se il comando
//! manca o fallisce, non è un errore per il chiamante).

/// Mostra una notifica; ritorna `true` se `notify-send` è partito bene.
pub fn notify(title: &str, body: &str) -> bool {
    match crate::system::run(&["notify-send", title, body]) {
        Ok(out) => out.success(),
        Err(_) => false,
    }
}
