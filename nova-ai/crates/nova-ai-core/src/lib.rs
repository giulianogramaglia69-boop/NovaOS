//! nova-ai-core: LLM client, agent loop, tools, config, providers.
//! No GUI dependency — this crate compiles and tests anywhere Rust does.

pub mod agent;
pub mod attachments;
pub mod config;
pub mod keyring;
pub mod llm;
pub mod providers;
pub mod tools;
pub mod voice;
pub mod wakeword;

pub use agent::{Agent, ChatOutcome};
pub use config::Config;
pub use llm::{ChatMessage, LlmClient, LlmError, LlmResponse, ToolCall, ToolDef};
