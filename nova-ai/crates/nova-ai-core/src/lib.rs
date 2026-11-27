//! nova-ai-core: LLM client, agent loop, tools, config, providers.
//! No GUI dependency — this crate compiles and tests anywhere Rust does.

pub mod agent;
pub mod attachments;
pub mod browser;
pub mod calendar;
pub mod civil;
pub mod config;
pub mod contacts;
pub mod keyring;
pub mod llm;
pub mod memory;
pub mod plans;
pub mod providers;
pub mod tools;
pub mod tts;
pub mod voice;
pub mod wakeword;
pub mod watchers;

pub use agent::{Agent, ChatOutcome};
pub use config::Config;
pub use llm::{ChatMessage, LlmClient, LlmError, LlmResponse, ToolCall, ToolDef};

/// The agent's data directory: `~/.local/share/nova-ai`.
/// Tests can override it with `NOVA_AI_DATA_DIR`.
pub fn data_dir() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("NOVA_AI_DATA_DIR") {
        return std::path::PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    std::path::PathBuf::from(home)
        .join(".local")
        .join("share")
        .join("nova-ai")
}
