//! Providers: all speak the OpenAI-compatible chat completions API, so
//! there is exactly one client code path. Only the base URL, default
//! model, vision and speech-to-text support differ.

/// The four providers NovaAI knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    OpenAI,
    Groq,
    OpenRouter,
    Custom,
}

impl ProviderKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProviderKind::OpenAI => "openai",
            ProviderKind::Groq => "groq",
            ProviderKind::OpenRouter => "openrouter",
            ProviderKind::Custom => "custom",
        }
    }

    /// Parse a provider name; unknown names become Custom.
    pub fn parse(s: &str) -> ProviderKind {
        match s.to_ascii_lowercase().as_str() {
            "openai" => ProviderKind::OpenAI,
            "groq" => ProviderKind::Groq,
            "openrouter" => ProviderKind::OpenRouter,
            _ => ProviderKind::Custom,
        }
    }

    /// Human-readable name for the UI.
    pub fn display_name(&self) -> &'static str {
        match self {
            ProviderKind::OpenAI => "OpenAI",
            ProviderKind::Groq => "Groq",
            ProviderKind::OpenRouter => "OpenRouter",
            ProviderKind::Custom => "Custom",
        }
    }

    pub fn all() -> [ProviderKind; 4] {
        [
            ProviderKind::Groq,
            ProviderKind::OpenAI,
            ProviderKind::OpenRouter,
            ProviderKind::Custom,
        ]
    }
}

/// (base URL, default model) for a provider.
pub fn provider_defaults(kind: ProviderKind) -> (&'static str, &'static str) {
    match kind {
        ProviderKind::OpenAI => ("https://api.openai.com/v1", "gpt-4o-mini"),
        ProviderKind::Groq => ("https://api.groq.com/openai/v1", "llama-3.3-70b-versatile"),
        // Cheap default on OpenRouter; vision-capable.
        ProviderKind::OpenRouter => ("https://openrouter.ai/api/v1", "anthropic/claude-haiku-4.5"),
        ProviderKind::Custom => ("", ""),
    }
}

/// Does this provider/model accept images (`image_url` parts)?
/// OpenAI gpt-4o-mini and the Claude models do; Groq only on its
/// vision models (llama-3.2-*-vision-preview). Unknown models are
/// assumed text-only — an honest "no" beats a failed request.
pub fn supports_vision(kind: ProviderKind, model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    match kind {
        ProviderKind::OpenAI => m.starts_with("gpt-4o") || m.starts_with("gpt-5"),
        ProviderKind::Groq => m.contains("vision"),
        ProviderKind::OpenRouter => {
            m.contains("claude") || m.contains("gpt-4o") || m.contains("vision")
        }
        ProviderKind::Custom => m.contains("vision") || m.contains("gpt-4o"),
    }
}

/// Speech-to-text model for this provider, if it has one.
/// Returns the Whisper model name to call on `<base>/audio/transcriptions`.
pub fn stt_model(kind: ProviderKind) -> Option<&'static str> {
    match kind {
        ProviderKind::OpenAI => Some("whisper-1"),
        ProviderKind::Groq => Some("whisper-large-v3"),
        // OpenRouter and Custom have no standard STT endpoint in v1.
        ProviderKind::OpenRouter | ProviderKind::Custom => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_openai_compatible() {
        let (url, model) = provider_defaults(ProviderKind::Groq);
        assert!(url.ends_with("/v1"));
        assert_eq!(model, "llama-3.3-70b-versatile");
        let (url, _) = provider_defaults(ProviderKind::OpenAI);
        assert_eq!(url, "https://api.openai.com/v1");
    }

    #[test]
    fn vision_table() {
        assert!(supports_vision(ProviderKind::OpenAI, "gpt-4o-mini"));
        assert!(!supports_vision(
            ProviderKind::Groq,
            "llama-3.3-70b-versatile"
        ));
        assert!(supports_vision(
            ProviderKind::Groq,
            "llama-3.2-11b-vision-preview"
        ));
        assert!(supports_vision(
            ProviderKind::OpenRouter,
            "anthropic/claude-haiku-4.5"
        ));
        assert!(!supports_vision(ProviderKind::Custom, "llama-3.3-70b"));
    }

    #[test]
    fn stt_table() {
        assert_eq!(stt_model(ProviderKind::Groq), Some("whisper-large-v3"));
        assert_eq!(stt_model(ProviderKind::OpenAI), Some("whisper-1"));
        assert_eq!(stt_model(ProviderKind::OpenRouter), None);
        assert_eq!(stt_model(ProviderKind::Custom), None);
    }
}
