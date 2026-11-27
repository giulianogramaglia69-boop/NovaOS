//! The agent loop: user message -> LLM -> tool calls -> execute ->
//! results back to the LLM -> final answer. Max 8 tool iterations per
//! turn (20 when a plan is active). Mutating tools pause the loop with
//! a pending action; the UI approves or denies, then the loop resumes
//! with the tool result.
//!
//! Long conversations are compacted progressively: when the history
//! passes ~100k estimated tokens (chars/4), the older half is
//! summarised by the LLM and replaced with a system message; the
//! summary is also saved in the persistent memory.

use crate::attachments::Attachment;
use crate::config::Config;
use crate::llm::{ChatMessage, LlmClient, LlmError, build_user_message};
use crate::memory::Memory;
use crate::plans::PlanStore;
use crate::tools::{self, PendingAction, ToolOutcome};

/// Italian system prompt: NovaAI is the NovaOS assistant.
pub const SYSTEM_PROMPT: &str = "Sei NovaAI, l'assistente AI di NovaOS. \
Rispondi in italiano, in modo conciso e diretto. Puoi gestire il sistema: \
aprire applicazioni, installare e rimuovere app Flatpak, regolare volume, \
luminosità, Wi-Fi e tema, e dare informazioni sul sistema. In più puoi: \
eseguire comandi shell (solo lettura subito; quelli che modificano il sistema \
chiedono prima conferma, e i comandi catastrofici come 'rm -rf /' sono sempre \
rifiutati), leggere/scrivere/cercare file sotto la home, catturare screenshot \
(da guardare solo se il modello supporta la visione), gestire finestre su Niri \
e Hyprland, controllare aggiornamenti OS e servizi, profili energetici, \
Bluetooth e audio, gestire container Distrobox, installare pacchetti di \
sistema con rpm-ostree (layering: rallenta gli aggiornamenti, serve riavvio), \
cercare sul web e scaricare file in ~/Downloads. Hai memoria persistente: \
ricorda i fatti che l'utente ti chiede di ricordare (tool memory) e usali \
nelle prossime sessioni. Puoi creare piani multi-passo (tool plan) e portarli \
avanti anche su più turni, mostrando l'avanzamento. Puoi leggere il \
calendario (oggi, domani, una data, ricerca) e la rubrica dei contatti. Puoi \
usare un browser vero (Firefox headless) per aprire pagine, leggere il testo, \
cliccare e scrivere nei campi (clic e scrittura chiedono conferma, come ogni \
azione che cambia qualcosa). Usa gli strumenti quando serve invece di \
inventare risposte. Se un'azione modifica il sistema (installazioni, \
rimozioni, scritture, comandi shell mutanti, aggiornamenti), l'utente deve \
prima confermarla: spiega brevemente cosa farai e lascia che sia lui ad \
approvare. Non eseguire mai azioni da amministratore (root) e non modificare \
file di sistema. Se non sai fare qualcosa, dillo chiaramente.";

const MAX_TOOL_ITERATIONS: usize = 8;
/// Higher cap while a plan is active: a plan needs several steps.
const MAX_PLAN_ITERATIONS: usize = 20;
/// Estimated-token budget for the history (chars/4). Above this the
/// older half is summarised.
const CONTEXT_BUDGET_TOKENS: usize = 100_000;

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
    /// Persistent memory (None when disabled in the settings).
    memory: Option<Memory>,
    /// Plan store (always available; plans are the agent's own data).
    plans: PlanStore,
    config: Config,
}

impl Agent {
    pub fn new(cfg: &Config) -> Result<Agent, LlmError> {
        let client = LlmClient::from_config(cfg);
        if client.api_key.is_none() && client.provider != crate::providers::ProviderKind::Local {
            return Err(LlmError::NoKey);
        }
        let mut system = SYSTEM_PROMPT.to_string();
        let memory = if cfg.memory {
            let mem = Memory::load_default();
            system.push_str(&mem.system_prompt_block());
            Some(mem)
        } else {
            None
        };
        let plans = PlanStore::load_default();
        // Unfinished plans, so the model knows what was left open.
        for p in plans.unfinished().iter().take(3) {
            system.push_str(&format!("\n\nPiano incompiuto: {}", p.progress_line()));
        }
        Ok(Agent {
            client,
            history: vec![ChatMessage::system(&system)],
            pending: None,
            pending_tool_call: None,
            memory,
            plans,
            config: cfg.clone(),
        })
    }

    /// Rebuild the client after a config change (provider/model/key).
    pub fn reconfigure(&mut self, cfg: &Config) -> Result<(), LlmError> {
        let client = LlmClient::from_config(cfg);
        if client.api_key.is_none() && client.provider != crate::providers::ProviderKind::Local {
            return Err(LlmError::NoKey);
        }
        self.client = client;
        self.config = cfg.clone();
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

    /// One line per unfinished plan, for the "resume?" note the UI
    /// shows when the chat opens.
    pub fn unfinished_plan_notes(&self) -> Vec<String> {
        self.plans
            .unfinished()
            .iter()
            .take(3)
            .map(|p| {
                format!(
                    "Hai un piano incompiuto: \"{}\" ({}/{} passi fatti). Di' \"riprendilo\" per continuare.",
                    p.title,
                    p.done_count(),
                    p.steps.len()
                )
            })
            .collect()
    }

    /// Estimated tokens of the history (chars/4, images counted by
    /// their base64 size).
    fn estimated_tokens(&self) -> usize {
        let chars: usize = self
            .history
            .iter()
            .map(|m| {
                m.text.chars().count()
                    + m.images.iter().map(|i| i.chars().count()).sum::<usize>()
                    + m.tool_calls
                        .iter()
                        .map(|c| c.arguments_raw.chars().count())
                        .sum::<usize>()
            })
            .sum();
        chars / 4
    }

    /// Render one history message as plain text for summarisation
    /// (images become a marker, never base64).
    fn message_text(m: &ChatMessage) -> String {
        let mut s = format!("{}: {}", m.role, m.text);
        if !m.images.is_empty() {
            s.push_str(" [immagine allegata]");
        }
        if !m.tool_calls.is_empty() {
            for c in &m.tool_calls {
                s.push_str(&format!(" [chiamata a {}]", c.name));
            }
        }
        // Keep each message short: the summary prompt must stay small.
        if s.chars().count() > 2000 {
            s = s.chars().take(2000).collect();
            s.push('…');
        }
        s
    }

    /// Summarise the older half of the history when it passes the
    /// budget, replacing it with a system message. The summary is
    /// also saved in the persistent memory. On LLM failure the old
    /// messages are dropped without a summary (honest fallback: the
    /// turn still works, the context is just shorter).
    fn maybe_compact(&mut self) {
        if self.estimated_tokens() <= CONTEXT_BUDGET_TOKENS {
            return;
        }
        // Keep the system message + the newest half; summarise the rest.
        let keep_from = self.history.len() / 2 + 1;
        let old: Vec<ChatMessage> = self.history.drain(1..keep_from).collect();
        let transcript = old
            .iter()
            .map(Self::message_text)
            .collect::<Vec<_>>()
            .join("\n");
        let summary = self
            .client
            .chat(
                &[
                    ChatMessage::system(
                        "Riassumi questa conversazione in italiano in al massimo 15 righe: \
                         conserva fatti importanti, decisioni prese, cose da fare e preferenze \
                         dell'utente. Non inventare niente.",
                    ),
                    ChatMessage::user(&transcript),
                ],
                &[],
            )
            .map(|r| r.text.trim().to_string())
            .unwrap_or_else(|_| {
                "Conversazione precedente (riassunto non disponibile, messaggi rimossi per far spazio)."
                    .to_string()
            });
        if let Some(mem) = &self.memory {
            let _ = mem.save_summary(&summary);
        }
        self.history.insert(
            1,
            ChatMessage::system(&format!("Riassunto della conversazione finora: {summary}")),
        );
    }

    /// Summarise the whole session (minus the system prompt) and save
    /// it as the last-session summary. Called by the UI when the chat
    /// window closes; does nothing for very short sessions or when
    /// memory is disabled.
    pub fn save_session_summary(&mut self) {
        if self.memory.is_none() || self.history.len() <= 4 {
            return;
        }
        let transcript = self.history[1..]
            .iter()
            .map(Self::message_text)
            .collect::<Vec<_>>()
            .join("\n");
        if let Ok(r) = self.client.chat(
            &[
                ChatMessage::system(
                    "Riassumi questa sessione in italiano in al massimo 10 righe: \
                     cosa abbiamo fatto, cosa è stato deciso, cosa resta da fare. \
                     Non inventare niente.",
                ),
                ChatMessage::user(&transcript),
            ],
            &[],
        ) && let Some(mem) = &self.memory
        {
            let _ = mem.save_summary(r.text.trim());
        }
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
        let max_iter = if self.plans.active().is_some() {
            MAX_PLAN_ITERATIONS
        } else {
            MAX_TOOL_ITERATIONS
        };
        for _ in 0..max_iter {
            self.maybe_compact();
            let resp = match self.client.chat(&self.history, &defs) {
                Ok(r) => r,
                Err(e) => return ChatOutcome::Error(format!("Errore del provider: {e}")),
            };
            if resp.tool_calls.is_empty() {
                let mut answer = resp.text;
                // Active plan: show progress with the answer.
                if let Some(p) = self.plans.active() {
                    answer.push_str(&format!("\n\n{}", p.progress_line()));
                }
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
            let ctx = tools::ToolContext {
                vision_supported: self.client.supports_vision(),
                calendar_url: self.config.calendar_url.clone(),
                calendar_file: self.config.calendar_file.clone(),
                memory_enabled: self.config.memory,
                data_dir: None,
            };
            match tools::execute_tool(&call.name, &call.arguments, &ctx) {
                Ok(ToolOutcome::Done(result)) => {
                    self.history
                        .push(ChatMessage::tool_result(&call.id, &result));
                }
                Ok(ToolOutcome::DoneWithImage {
                    text,
                    image_data_url,
                }) => {
                    // Tool result first (keeps OpenAI tool-message
                    // ordering valid), then the image as a user
                    // message the model can actually see.
                    self.history.push(ChatMessage::tool_result(&call.id, &text));
                    let mut img_msg = ChatMessage::user("Ecco lo screenshot appena catturato:");
                    img_msg.images = vec![image_data_url];
                    self.history.push(img_msg);
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
