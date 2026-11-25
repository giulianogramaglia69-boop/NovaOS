//! Configuration for NovaAI: provider, model, API key.
//!
//! The config file lives at `~/.config/nova-ai/config.toml` and holds the
//! provider, base URL and model. The API key is stored in the libsecret
//! keyring (service "NovaAI") first; only if the keyring is unavailable is
//! the key written into the config file, which is then created with mode
//! 0600. The key is never logged or printed.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::keyring;
use crate::providers::{ProviderKind, provider_defaults};

/// On-disk config (TOML). `api_key` is only populated from the file
/// fallback; the keyring is the primary store.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Config {
    /// "openai" | "groq" | "openrouter" | "custom"
    #[serde(default = "default_provider_name")]
    pub provider: String,
    /// Base URL of the OpenAI-compatible chat completions API.
    #[serde(default)]
    pub base_url: String,
    /// Model name.
    #[serde(default)]
    pub model: String,
    /// API key, only used when the keyring is unavailable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// "Ehi Nova" wake word enabled (opt-in, default OFF). The toggle
    /// persists here; the model itself is downloaded on first use.
    #[serde(default)]
    pub wake_word: bool,
}

fn default_provider_name() -> String {
    "groq".to_string()
}

impl Config {
    /// Defaults for a provider kind (base URL + model, no key).
    pub fn for_provider(kind: ProviderKind) -> Config {
        let (base_url, model) = provider_defaults(kind);
        Config {
            provider: kind.as_str().to_string(),
            base_url: base_url.to_string(),
            model: model.to_string(),
            api_key: None,
            wake_word: false,
        }
    }

    /// The provider kind parsed from `provider`; unknown names fall back
    /// to Custom (the base URL and model then come from the file).
    pub fn provider_kind(&self) -> ProviderKind {
        ProviderKind::parse(&self.provider)
    }

    /// Default config file path: `~/.config/nova-ai/config.toml`.
    /// Tests can override it with the `NOVA_AI_CONFIG` env var.
    pub fn default_path() -> PathBuf {
        if let Ok(p) = std::env::var("NOVA_AI_CONFIG") {
            return PathBuf::from(p);
        }
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home)
            .join(".config")
            .join("nova-ai")
            .join("config.toml")
    }

    /// Load the config file; if it does not exist, return Groq defaults
    /// (the provider the user is most likely to start with: free tier,
    /// no card). Missing fields in an existing file are filled with the
    /// defaults of the file's provider.
    pub fn load() -> Result<Config, String> {
        Self::load_from(&Self::default_path())
    }

    pub fn load_from(path: &PathBuf) -> Result<Config, String> {
        match fs::read_to_string(path) {
            Ok(text) => {
                let mut cfg: Config =
                    toml::from_str(&text).map_err(|e| format!("config parse error: {e}"))?;
                cfg.fill_defaults();
                Ok(cfg)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Ok(Config::for_provider(ProviderKind::Groq))
            }
            Err(e) => Err(format!("cannot read config {}: {e}", path.display())),
        }
    }

    /// Fill empty base_url / model from the provider defaults.
    pub fn fill_defaults(&mut self) {
        let (base_url, model) = provider_defaults(self.provider_kind());
        if self.base_url.is_empty() {
            self.base_url = base_url.to_string();
        }
        if self.model.is_empty() {
            self.model = model.to_string();
        }
    }

    /// Save the config file (creates the parent dir, mode 0600 on the
    /// file). The key is only written here when the keyring is
    /// unavailable; callers should prefer `save_without_key` + keyring.
    pub fn save(&self) -> Result<(), String> {
        self.save_to(&Self::default_path())
    }

    pub fn save_to(&self, path: &PathBuf) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("cannot create config dir: {e}"))?;
        }
        let text =
            toml::to_string_pretty(self).map_err(|e| format!("config serialize error: {e}"))?;
        fs::write(path, text).map_err(|e| format!("cannot write config: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    /// Save the config without the API key in the file (keyring path).
    pub fn save_without_key(&self) -> Result<(), String> {
        let mut clone = self.clone();
        clone.api_key = None;
        clone.save()
    }

    /// Resolve the API key: libsecret keyring first, then the config
    /// file fallback. Returns None when no key is configured anywhere.
    /// Tests can disable the keyring lookup with NOVA_AI_NO_KEYRING=1.
    pub fn api_key(&self) -> Option<String> {
        if std::env::var("NOVA_AI_NO_KEYRING").is_err()
            && let Some(k) = keyring::lookup(&self.provider)
        {
            return Some(k);
        }
        self.api_key.clone()
    }

    /// Store the API key: keyring first; if that fails, into the config
    /// file (and save it). Returns where it ended up.
    pub fn store_key(&mut self, key: &str) -> Result<KeyLocation, String> {
        if keyring::store(&self.provider, key) {
            self.api_key = None;
            return Ok(KeyLocation::Keyring);
        }
        self.api_key = Some(key.to_string());
        self.save()?;
        Ok(KeyLocation::ConfigFile)
    }

    /// True when an API key is available from any store.
    pub fn has_key(&self) -> bool {
        self.api_key().is_some()
    }
}

/// Where the API key ended up being stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyLocation {
    Keyring,
    ConfigFile,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp_path(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("nova-ai-test-{name}-{}", std::process::id()));
        p
    }

    #[test]
    fn missing_file_gives_groq_defaults() {
        let p = tmp_path("missing.toml");
        let _ = fs::remove_file(&p);
        let cfg = Config::load_from(&p).unwrap();
        assert_eq!(cfg.provider, "groq");
        assert_eq!(cfg.base_url, "https://api.groq.com/openai/v1");
        assert_eq!(cfg.model, "llama-3.3-70b-versatile");
        assert!(cfg.api_key.is_none());
    }

    #[test]
    fn round_trip() {
        let p = tmp_path("roundtrip.toml");
        let mut cfg = Config::for_provider(ProviderKind::OpenRouter);
        cfg.api_key = Some("sk-test-key".to_string());
        cfg.save_to(&p).unwrap();
        let back = Config::load_from(&p).unwrap();
        assert_eq!(back, cfg);
        // mode 0600 on unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&p).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn partial_file_fills_defaults() {
        let p = tmp_path("partial.toml");
        let mut f = fs::File::create(&p).unwrap();
        writeln!(f, "provider = \"openai\"").unwrap();
        drop(f);
        let cfg = Config::load_from(&p).unwrap();
        assert_eq!(cfg.base_url, "https://api.openai.com/v1");
        assert_eq!(cfg.model, "gpt-4o-mini");
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn unknown_provider_becomes_custom() {
        let cfg = Config {
            provider: "weird".to_string(),
            base_url: "https://example.invalid/v1".to_string(),
            model: "m".to_string(),
            api_key: None,
            wake_word: false,
        };
        assert_eq!(cfg.provider_kind(), ProviderKind::Custom);
    }

    #[test]
    fn wake_word_defaults_off_and_persists() {
        // Old config files without the field still load (default OFF).
        let p = tmp_path("wake-old.toml");
        let mut f = fs::File::create(&p).unwrap();
        writeln!(f, "provider = \"groq\"").unwrap();
        drop(f);
        let cfg = Config::load_from(&p).unwrap();
        assert!(!cfg.wake_word);
        let _ = fs::remove_file(&p);

        // Round-trip with the toggle on.
        let p = tmp_path("wake-on.toml");
        let mut cfg = Config::for_provider(ProviderKind::Groq);
        cfg.wake_word = true;
        cfg.save_to(&p).unwrap();
        let back = Config::load_from(&p).unwrap();
        assert!(back.wake_word);
        assert_eq!(back, cfg);
        let _ = fs::remove_file(&p);
    }
}
