//! API key storage via libsecret, using the `secret-tool` CLI (shipped
//! with libsecret on Fedora). No D-Bus crates needed: if `secret-tool`
//! is missing or the lookup fails, the caller falls back to the config
//! file. The key value is never printed or logged here.
//!
//! Service: "NovaAI", attribute: `provider=<provider-name>`.

use std::process::Command;

/// Look a key up in the keyring. Returns None on any failure.
pub fn lookup(provider: &str) -> Option<String> {
    if std::env::var("NOVA_AI_NO_KEYRING").is_ok() {
        return None;
    }
    let out = Command::new("secret-tool")
        .args(["lookup", "service", "NovaAI", "provider", provider])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let key = String::from_utf8(out.stdout).ok()?;
    let key = key.trim().to_string();
    if key.is_empty() { None } else { Some(key) }
}

/// Store a key in the keyring. Returns false on any failure (the caller
/// then falls back to the config file).
pub fn store(provider: &str, key: &str) -> bool {
    if std::env::var("NOVA_AI_NO_KEYRING").is_ok() {
        return false;
    }
    if key.trim().is_empty() {
        return false;
    }
    let mut child = match Command::new("secret-tool")
        .args([
            "store", "--label", "NovaAI", "service", "NovaAI", "provider", provider,
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    use std::io::Write;
    let ok = child
        .stdin
        .as_mut()
        .map(|s| s.write_all(key.as_bytes()).is_ok())
        .unwrap_or(false);
    if !ok {
        let _ = child.kill();
        return false;
    }
    match child.wait() {
        Ok(s) => s.success(),
        Err(_) => false,
    }
}

/// Remove a key from the keyring (best effort).
pub fn remove(provider: &str) -> bool {
    let out = Command::new("secret-tool")
        .args(["clear", "service", "NovaAI", "provider", provider])
        .output();
    matches!(out, Ok(o) if o.status.success())
}
