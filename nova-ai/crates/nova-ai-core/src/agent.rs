//! The agent loop: user message -> LLM -> tool calls -> execute ->
//! results back to the LLM -> final answer. Max 8 tool iterations per
//! turn. Mutating tools pause the loop with a pending action; the UI
//! approves or denies, then the loop resumes with the tool result.

use crate::attachments::Attachment;
use crate::config::Config;
use crate::llm::{ChatMessage, LlmClient, LlmError, build_user_message};
use crate::tools::{self, PendingAction, ToolOutcome};

/// Italian system prompt: NovaAI is the NovaOS assistant.
pub const SYSTEM_PROMPT: &str = "Sei NovaAI, l'assistente AI di NovaOS. \
Rispondi in italiano, in modo conciso e diretto. Puoi gestire il sistema: \
aprire applicazioni, installare e rimuovere app Flatpak, regolare volume, \
luminosità, Wi-Fi e tema, e dare informazioni sul sistema. Usa gli strumenti \
quando serve invece di inventare risposte. Se un'azione modifica il sistema \
(installazioni, rimozioni, impostazioni), l'utente deve prima confermarla: \
spiega brevemente cosa farai e lascia che sia lui ad approvare. Non eseguire \
mai azioni da amministratore (root) e non modificare file di sistema. Se non \
sai fare qualcosa, dillo chiaramente.";

const MAX_TOOL_ITERATIONS: usize = 8;

/// What a turn returns to the UI.
#[derive(Debug)]
pub enum ChatOutcome {
    /// Final answer text for the user.
    Answer(String),
    /// A mutating tool needs approval. The UI shows the description
    /// with Approve/Deny, then calls `confirm`.
    NeedsConfirmation(PendingAction),
    /// Something failed (network, provider, tool). The text is
    /// user-facing and honest.
    Error(String),
}

pub struct Agent {
    client: LlmClient,
    history: Vec<ChatMessage>,
    pending: Option<PendingAction>,
    /// Tool calls executed so far in the current turn (for the loop).
    pending_tool_call: Option<(String, PendingAction)>,
}

impl Agent {
    pub fn new(cfg: &Config) -> Result<Agent, LlmError> {
        let client = LlmClient::from_config(cfg);
        if client.api_key.is_none() {
            return Err(LlmError::NoKey);
        }
        Ok(Agent {
            client,
            history: vec![ChatMessage::system(SYSTEM_PROMPT)],
            pending: None,
            pending_tool_call: None,
        })
    }

    /// Rebuild the client after a config change (provider/model/key).
    pub fn reconfigure(&mut self, cfg: &Config) -> Result<(), LlmError> {
        let client = LlmClient::from_config(cfg);
        if client.api_key.is_none() {
            return Err(LlmError::NoKey);
        }
        self.client = client;
        Ok(())
    }

    pub fn vision_supported(&self) -> bool {
        self.client.supports_vision()
    }

    pub fn stt_model(&self) -> Option<&'static str> {
        self.client.stt_model()
    }

    pub fn model_label(&self) -> String {
        format!(
            "{}/{}",
            self.client.provider.display_name(),
            self.client.model
        )
    }

    /// True when a mutating tool is waiting for the user's approval.
    /// The wake-word turn checks this before starting a new turn, so a
    /// hands-free turn can never overwrite a pending confirmation.
    pub fn has_pending_action(&self) -> bool {
        self.pending_tool_call.is_some()
    }

    /// Send a user message (with optional attachments) and run the loop
    /// until the model answers or a confirmation is needed.
    pub fn send(&mut self, text: &str, attachments: &[Attachment]) -> ChatOutcome {
        let (msg, notes) = build_user_message(text, attachments, self.client.supports_vision());
        self.history.push(msg);
        let mut outcome = self.run_loop();
        if let ChatOutcome::Answer(ref mut a) = outcome {
            for note in notes {
                a.push_str(&format!("\n\n({note})"));
            }
        }
        outcome
    }

    /// The user approved (true) or denied (false) the pending action.
    /// On approval the action executes and the loop resumes with the
    /// tool result; on denial the model is told the user refused.
    pub fn confirm(&mut self, approve: bool) -> ChatOutcome {
        let Some((call_id, action)) = self.pending_tool_call.take() else {
            self.pending = None;
            return ChatOutcome::Error("nessuna azione in attesa di conferma".to_string());
        };
        self.pending = None;
        let result = if approve {
            match tools::execute_confirmed(&action) {
                Ok(s) => s,
                Err(e) => format!("Errore eseguendo l'azione: {e}"),
            }
        } else {
            "L'utente ha rifiutato l'azione.".to_string()
        };
        self.history
            .push(ChatMessage::tool_result(&call_id, &result));
        self.run_loop()
    }

    /// The LLM loop. Appends assistant messages (including tool calls)
    /// to history as it goes.
    fn run_loop(&mut self) -> ChatOutcome {
        let defs = tools::tool_definitions();
        for _ in 0..MAX_TOOL_ITERATIONS {
            let resp = match self.client.chat(&self.history, &defs) {
                Ok(r) => r,
                Err(e) => return ChatOutcome::Error(format!("Errore del provider: {e}")),
            };
            if resp.tool_calls.is_empty() {
                let answer = resp.text;
                self.history.push(ChatMessage::assistant(&answer));
                return ChatOutcome::Answer(answer);
            }
            // One tool call at a time (first call of the turn).
            let call = resp.tool_calls[0].clone();
            let mut assistant_msg = ChatMessage::assistant(&resp.text);
            assistant_msg.tool_calls = vec![crate::llm::ToolCall {
                id: call.id.clone(),
                name: call.name.clone(),
                arguments: call.arguments.clone(),
                arguments_raw: call.arguments_raw.clone(),
            }];
            self.history.push(assistant_msg);
            match tools::execute_tool(&call.name, &call.arguments) {
                Ok(ToolOutcome::Done(result)) => {
                    self.history
                        .push(ChatMessage::tool_result(&call.id, &result));
                }
                Ok(ToolOutcome::NeedsConfirmation(action)) => {
                    self.pending = Some(action.clone());
                    self.pending_tool_call = Some((call.id.clone(), action));
                    return ChatOutcome::NeedsConfirmation(self.pending.clone().unwrap());
                }
                Err(e) => {
                    self.history
                        .push(ChatMessage::tool_result(&call.id, &format!("Errore: {e}")));
                }
            }
        }
        ChatOutcome::Error(
            "troppe chiamate agli strumenti in un solo turno, mi fermo qui".to_string(),
        )
    }
}
