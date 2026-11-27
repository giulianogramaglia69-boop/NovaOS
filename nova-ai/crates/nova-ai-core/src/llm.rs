//! OpenAI-compatible chat completions client (one code path for all
//! providers) + Whisper transcription. Blocking HTTP via ureq; the UI
//! calls this from a worker thread. The API key is sent as a Bearer
//! token and never logged.

use serde_json::{Value, json};

use crate::attachments::Attachment;
use crate::config::Config;
use crate::providers::{ProviderKind, stt_model, supports_vision};

/// One chat message. `images` holds base64 data URLs (already
/// downscaled by the caller) attached to a user message.
#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: String, // "system" | "user" | "assistant" | "tool"
    pub text: String,
    pub images: Vec<String>, // data:image/...;base64,... URLs
    pub tool_call_id: Option<String>,
    pub tool_calls: Vec<ToolCall>,
}

impl ChatMessage {
    pub fn system(text: &str) -> Self {
        ChatMessage {
            role: "system".to_string(),
            text: text.to_string(),
            images: vec![],
            tool_call_id: None,
            tool_calls: vec![],
        }
    }

    pub fn user(text: &str) -> Self {
        ChatMessage {
            role: "user".to_string(),
            text: text.to_string(),
            images: vec![],
            tool_call_id: None,
            tool_calls: vec![],
        }
    }

    pub fn assistant(text: &str) -> Self {
        ChatMessage {
            role: "assistant".to_string(),
            text: text.to_string(),
            images: vec![],
            tool_call_id: None,
            tool_calls: vec![],
        }
    }

    pub fn tool_result(tool_call_id: &str, text: &str) -> Self {
        ChatMessage {
            role: "tool".to_string(),
            text: text.to_string(),
            images: vec![],
            tool_call_id: Some(tool_call_id.to_string()),
            tool_calls: vec![],
        }
    }

    /// Serialize to the OpenAI chat format.
    fn to_json(&self) -> Value {
        if self.role == "tool" {
            return json!({
                "role": "tool",
                "tool_call_id": self.tool_call_id,
                "content": self.text,
            });
        }
        if !self.tool_calls.is_empty() {
            let calls: Vec<Value> = self
                .tool_calls
                .iter()
                .map(|c| {
                    json!({
                        "id": c.id,
                        "type": "function",
                        "function": { "name": c.name, "arguments": c.arguments_raw },
                    })
                })
                .collect();
            let mut obj = json!({
                "role": "assistant",
                "tool_calls": calls,
            });
            if !self.text.is_empty() {
                obj["content"] = Value::String(self.text.clone());
            }
            return obj;
        }
        if !self.images.is_empty() {
            let mut parts: Vec<Value> = vec![json!({ "type": "text", "text": self.text })];
            for img in &self.images {
                parts.push(json!({ "type": "image_url", "image_url": { "url": img } }));
            }
            return json!({ "role": self.role, "content": parts });
        }
        json!({ "role": self.role, "content": self.text })
    }
}

/// A function call requested by the model.
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// Parsed arguments (object, may be empty).
    pub arguments: Value,
    /// Raw arguments string as returned by the model.
    pub arguments_raw: String,
}

/// What the model returned for one turn.
#[derive(Debug, Clone)]
pub struct LlmResponse {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
}

/// Definition of one tool, in OpenAI function-calling format.
#[derive(Debug, Clone)]
pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub parameters: Value,
}

impl ToolDef {
    pub fn to_json(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": self.name,
                "description": self.description,
                "parameters": self.parameters,
            },
        })
    }
}

/// Errors from the LLM layer. Messages are user-facing (Italian where
/// the user sees them); the HTTP status is kept for diagnosis.
#[derive(Debug)]
pub enum LlmError {
    NoKey,
    Http(u16, String),
    Transport(String),
    Parse(String),
}

impl std::fmt::Display for LlmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LlmError::NoKey => write!(f, "nessuna chiave API configurata"),
            LlmError::Http(code, body) => {
                write!(f, "il provider ha risposto con errore HTTP {code}: {body}")
            }
            LlmError::Transport(e) => write!(f, "errore di rete: {e}"),
            LlmError::Parse(e) => write!(f, "risposta del provider non valida: {e}"),
        }
    }
}

impl std::error::Error for LlmError {}

/// The client. Holds a snapshot of the config; rebuild it after the
/// user changes provider/model/key.
#[derive(Debug, Clone)]
pub struct LlmClient {
    pub provider: ProviderKind,
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
}

/// Parse a textual tool call from a plain-text answer (fallback for
/// providers that reject the native `tools` parameter). The model is
/// instructed to answer with a single fenced JSON block:
/// ```json
/// {"tool": "shell", "arguments": {"command": "ls"}}
/// ```
/// Returns the call (id `text-call-N`) when a well-formed block is
/// found, else None. The block is stripped from the returned text.
pub fn parse_text_tool_call(text: &str) -> Option<(ToolCall, String)> {
    let start = text.find("```json").or_else(|| text.find("```"))?;
    let after_fence = &text[start..];
    let body_start = after_fence.find('\n').map(|i| i + 1)?;
    let body = &after_fence[body_start..];
    let end = body.find("```")?;
    let block = body[..end].trim();
    let v: Value = serde_json::from_str(block).ok()?;
    let name = v.get("tool")?.as_str()?.to_string();
    if name.is_empty() {
        return None;
    }
    let arguments = v
        .get("arguments")
        .cloned()
        .unwrap_or(Value::Object(Default::default()));
    let raw = serde_json::to_string(&arguments).unwrap_or_else(|_| "{}".to_string());
    let call = ToolCall {
        id: "text-call-1".to_string(),
        name,
        arguments,
        arguments_raw: raw,
    };
    let mut rest = String::new();
    rest.push_str(text[..start].trim());
    let tail = body[end + 3..].trim();
    if !tail.is_empty() {
        if !rest.is_empty() {
            rest.push('\n');
        }
        rest.push_str(tail);
    }
    Some((call, rest))
}

/// True when a provider error message suggests the `tools` parameter
/// itself is rejected (custom / minimal endpoints). Used to decide
/// the one-shot retry without tools.
pub fn looks_like_tools_unsupported(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    (m.contains("tool") || m.contains("function"))
        && (m.contains("unsupported")
            || m.contains("not supported")
            || m.contains("unknown")
            || m.contains("invalid"))
}

/// Instruction appended to the system prompt when native function
/// calling is unavailable: the model must answer with a fenced JSON
/// tool call instead.
pub const TEXT_TOOL_INSTRUCTION: &str = "Questo provider non supporta le chiamate a funzione native: quando vuoi usare uno strumento, rispondi SOLO con un blocco di codice json contenente {\"tool\": \"nome_strumento\", \"arguments\": {...}} e nient'altro. Per rispondere senza strumenti, scrivi solo il testo.";

impl LlmClient {
    pub fn from_config(cfg: &Config) -> Self {
        LlmClient {
            provider: cfg.provider_kind(),
            base_url: cfg.base_url.trim_end_matches('/').to_string(),
            model: cfg.model.clone(),
            api_key: cfg.api_key(),
        }
    }

    pub fn supports_vision(&self) -> bool {
        supports_vision(self.provider, &self.model)
    }

    /// Whisper model for voice transcription, if this provider has one.
    pub fn stt_model(&self) -> Option<&'static str> {
        stt_model(self.provider)
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    /// ureq agent that returns error statuses as responses instead of
    /// Err, so the provider's own error message can be surfaced.
    /// Also used by the wake-word model downloader (wakeword.rs).
    pub(crate) fn http_agent() -> ureq::Agent {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(std::time::Duration::from_secs(120)))
            .build();
        ureq::Agent::new_with_config(config)
    }

    /// One chat completions call. `tools` may be empty (plain chat).
    /// When the provider rejects the `tools` parameter (HTTP 400
    /// mentioning tools/functions), the call is retried once without
    /// tools and a textual tool call in the answer is parsed instead
    /// (see [`parse_text_tool_call`]).
    pub fn chat(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolDef],
    ) -> Result<LlmResponse, LlmError> {
        let send_tools = !tools.is_empty();
        match self.chat_inner(messages, tools, send_tools) {
            Ok(r) => Ok(r),
            Err(LlmError::Http(400, msg)) if send_tools && looks_like_tools_unsupported(&msg) => {
                // Fallback: same messages + textual-tool instruction,
                // no `tools` parameter at all.
                let mut fallback: Vec<ChatMessage> = messages.to_vec();
                if let Some(sys) = fallback.iter_mut().find(|m| m.role == "system") {
                    sys.text.push_str("\n\n");
                    sys.text.push_str(TEXT_TOOL_INSTRUCTION);
                } else {
                    fallback.insert(0, ChatMessage::system(TEXT_TOOL_INSTRUCTION));
                }
                let resp = self.chat_inner(&fallback, &[], false)?;
                if let Some((call, rest)) = parse_text_tool_call(&resp.text) {
                    Ok(LlmResponse {
                        text: rest,
                        tool_calls: vec![call],
                    })
                } else {
                    Ok(resp)
                }
            }
            Err(e) => Err(e),
        }
    }

    fn chat_inner(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolDef],
        send_tools: bool,
    ) -> Result<LlmResponse, LlmError> {
        // Local (Ollama) needs no API key; every other provider does.
        let key = match &self.api_key {
            Some(k) => Some(k.clone()),
            None if self.provider == ProviderKind::Local => None,
            None => return Err(LlmError::NoKey),
        };
        if self.base_url.is_empty() {
            return Err(LlmError::Transport(
                "base URL del provider mancante (provider Custom senza URL?)".to_string(),
            ));
        }
        let mut body = json!({
            "model": self.model,
            "messages": messages.iter().map(|m| m.to_json()).collect::<Vec<_>>(),
        });
        if send_tools {
            body["tools"] = json!(tools.iter().map(|t| t.to_json()).collect::<Vec<_>>());
            body["tool_choice"] = Value::String("auto".to_string());
        }
        let mut req = Self::http_agent()
            .post(&self.endpoint("/chat/completions"))
            .header("Content-Type", "application/json");
        if let Some(k) = &key {
            req = req.header("Authorization", &format!("Bearer {k}"));
        }
        if self.provider == ProviderKind::OpenRouter {
            req = req
                .header("HTTP-Referer", "https://novaos.local/")
                .header("X-Title", "NovaAI");
        }
        let resp = req.send_json(&body).map_err(map_ureq_error)?;
        let status = resp.status().as_u16();
        let v: Value = resp
            .into_body()
            .read_json()
            .map_err(|e| LlmError::Parse(format!("{e}")))?;
        if !(200..300).contains(&status) {
            return Err(LlmError::Http(status, extract_provider_message(&v)));
        }
        parse_chat_response(&v)
    }

    /// Minimal "say hi" call to verify the key works. Returns the
    /// model's short reply on success.
    pub fn test_connection(&self) -> Result<String, LlmError> {
        let msgs = vec![
            ChatMessage::system("You are a test probe. Reply with exactly: ok"),
            ChatMessage::user("hi"),
        ];
        let r = self.chat(&msgs, &[])?;
        Ok(r.text.trim().to_string())
    }

    /// Transcribe a WAV file (16 kHz mono) via the provider's Whisper
    /// endpoint. Returns the plain transcript text.
    pub fn transcribe(&self, wav: &[u8]) -> Result<String, LlmError> {
        let key = self.api_key.clone().ok_or(LlmError::NoKey)?;
        let model = self.stt_model().ok_or_else(|| {
            LlmError::Transport("questo provider non ha trascrizione vocale".to_string())
        })?;
        let boundary = "----novaai-wav-boundary";
        let mut body: Vec<u8> = Vec::new();
        let push = |body: &mut Vec<u8>, s: &str| body.extend_from_slice(s.as_bytes());
        push(&mut body, &format!("--{boundary}\r\n"));
        push(
            &mut body,
            "Content-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\n",
        );
        push(&mut body, "Content-Type: audio/wav\r\n\r\n");
        body.extend_from_slice(wav);
        push(&mut body, "\r\n");
        push(&mut body, &format!("--{boundary}\r\n"));
        push(
            &mut body,
            "Content-Disposition: form-data; name=\"model\"\r\n\r\n",
        );
        push(&mut body, model);
        push(&mut body, "\r\n");
        push(&mut body, &format!("--{boundary}--\r\n"));

        let resp = Self::http_agent()
            .post(&self.endpoint("/audio/transcriptions"))
            .header("Authorization", &format!("Bearer {key}"))
            .header(
                "Content-Type",
                &format!("multipart/form-data; boundary={boundary}"),
            )
            .send(&body)
            .map_err(map_ureq_error)?;
        let status = resp.status().as_u16();
        let v: Value = resp
            .into_body()
            .read_json()
            .map_err(|e| LlmError::Parse(format!("{e}")))?;
        if !(200..300).contains(&status) {
            return Err(LlmError::Http(status, extract_provider_message(&v)));
        }
        v.get("text")
            .and_then(|t| t.as_str())
            .map(|t| t.trim().to_string())
            .ok_or_else(|| {
                LlmError::Parse("campo 'text' mancante nella risposta Whisper".to_string())
            })
    }
}

/// Parse an OpenAI-style chat completions response.
fn parse_chat_response(v: &Value) -> Result<LlmResponse, LlmError> {
    let choice = v
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
        .ok_or_else(|| LlmError::Parse("nessuna choice nella risposta".to_string()))?;
    let msg = choice
        .get("message")
        .ok_or_else(|| LlmError::Parse("campo 'message' mancante".to_string()))?;
    let text = msg
        .get("content")
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();
    let mut tool_calls = Vec::new();
    if let Some(calls) = msg.get("tool_calls").and_then(|c| c.as_array()) {
        for c in calls {
            let f = match c.get("function") {
                Some(f) => f,
                None => continue,
            };
            let name = f
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string();
            let raw = f
                .get("arguments")
                .and_then(|a| a.as_str())
                .unwrap_or("{}")
                .to_string();
            let arguments: Value =
                serde_json::from_str(&raw).unwrap_or(Value::Object(Default::default()));
            tool_calls.push(ToolCall {
                id: c
                    .get("id")
                    .and_then(|i| i.as_str())
                    .unwrap_or("")
                    .to_string(),
                name,
                arguments,
                arguments_raw: raw,
            });
        }
    }
    Ok(LlmResponse { text, tool_calls })
}

fn map_ureq_error(e: ureq::Error) -> LlmError {
    match e {
        ureq::Error::Timeout(_) => LlmError::Transport("timeout".to_string()),
        other => LlmError::Transport(format!("{other}")),
    }
}

/// The provider's own error message when present (`{"error": {"message":
/// ...}}`), else the raw body truncated.
fn extract_provider_message(v: &Value) -> String {
    if let Some(m) = v
        .get("error")
        .and_then(|e| e.get("message"))
        .and_then(|m| m.as_str())
    {
        return m.to_string();
    }
    let s = v.to_string();
    s.chars().take(300).collect()
}

/// Build the user message for a turn, attaching text files as context
/// and images (only when the model supports vision).
pub fn build_user_message(
    text: &str,
    attachments: &[Attachment],
    vision: bool,
) -> (ChatMessage, Vec<String>) {
    let mut msg = ChatMessage::user(text);
    let mut notes: Vec<String> = Vec::new();
    let mut full_text = text.to_string();
    for att in attachments {
        match att {
            Attachment::TextFile { name, content } => {
                full_text.push_str(&format!("\n\n[allegato: {name}]\n{content}"));
            }
            Attachment::Image { name, data_url } => {
                if vision {
                    msg.images.push(data_url.clone());
                } else {
                    notes.push(format!(
                        "immagine '{name}' non inviata: il modello corrente non supporta la visione"
                    ));
                }
            }
            Attachment::Other { name } => {
                notes.push(format!("allegato '{name}' non letto (tipo non supportato)"));
            }
        }
    }
    msg.text = full_text;
    (msg, notes)
}
