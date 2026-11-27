//! LLM client + agent loop tests against a local mock HTTP server.
//! No real API calls, no network beyond localhost.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;

use nova_ai_core::agent::Agent;
use nova_ai_core::config::Config;
use nova_ai_core::llm::{ChatMessage, LlmClient};
use nova_ai_core::providers::ProviderKind;

/// Start a mock server that answers each request with the next canned
/// JSON body (in order). Returns the base URL (http://127.0.0.1:PORT).
fn start_mock(responses: Vec<String>) -> String {
    let statused: Vec<(u16, String)> = responses
        .into_iter()
        .map(|b| {
            let status = if b.contains("\"error\"") && !b.contains("choices") {
                500
            } else {
                200
            };
            (status, b)
        })
        .collect();
    start_mock_statused(statused).0
}

/// Like `start_mock`, but each entry is (HTTP status, body). Also
/// returns the request bodies the mock saw, in order.
fn start_mock_statused(responses: Vec<(u16, String)>) -> (String, Arc<Mutex<Vec<String>>>) {
    let queue: Arc<Mutex<VecDeque<(u16, String)>>> = Arc::new(Mutex::new(responses.into()));
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let seen_clone = seen.clone();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = match stream {
                Ok(s) => s,
                Err(_) => continue,
            };
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
            // Read headers.
            let mut buf: Vec<u8> = Vec::new();
            let mut chunk = [0u8; 4096];
            let mut header_end = None;
            while header_end.is_none() {
                match stream.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        buf.extend_from_slice(&chunk[..n]);
                        if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
                            header_end = Some(pos + 4);
                        }
                        if buf.len() > 65536 {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            // Read the body, if any, and remember it.
            let mut body_bytes: Vec<u8> = Vec::new();
            let mut content_len = 0usize;
            if let Some(end) = header_end {
                let headers = String::from_utf8_lossy(&buf[..end]).to_string();
                for line in headers.lines() {
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        content_len = v.trim().parse().unwrap_or(0);
                    }
                }
                let already = buf.len() - end;
                body_bytes.extend_from_slice(&buf[end..]);
                let mut remaining = content_len.saturating_sub(already);
                while remaining > 0 {
                    match stream.read(&mut chunk) {
                        Ok(0) => break,
                        Ok(n) => {
                            body_bytes.extend_from_slice(&chunk[..n]);
                            remaining = remaining.saturating_sub(n);
                        }
                        Err(_) => break,
                    }
                }
            }
            seen_clone
                .lock()
                .unwrap()
                .push(String::from_utf8_lossy(&body_bytes).to_string());
            let (status, body) =
                queue.lock().unwrap().pop_front().unwrap_or_else(|| {
                    (500, r#"{"error":{"message":"mock esaurito"}}"#.to_string())
                });
            let reason = match status {
                200 => "OK",
                400 => "Bad Request",
                _ => "Internal Server Error",
            };
            let resp = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
        }
    });
    (format!("http://127.0.0.1:{port}"), seen)
}

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn chat_response(text: &str) -> String {
    serde_json::json!({
        "id": "chatcmpl-test",
        "choices": [{ "message": { "role": "assistant", "content": text }, "finish_reason": "stop" }],
        "usage": { "prompt_tokens": 1, "completion_tokens": 1 }
    })
    .to_string()
}

fn tool_call_response(id: &str, name: &str, args: &str) -> String {
    serde_json::json!({
        "id": "chatcmpl-test",
        "choices": [{ "message": {
            "role": "assistant",
            "content": "",
            "tool_calls": [{ "id": id, "type": "function",
                "function": { "name": name, "arguments": args } }]
        }, "finish_reason": "tool_calls" }]
    })
    .to_string()
}

fn test_client(base_url: &str) -> LlmClient {
    let mut cfg = Config::for_provider(ProviderKind::Custom);
    cfg.base_url = base_url.to_string();
    cfg.model = "test-model".to_string();
    cfg.api_key = Some("test-key".to_string());
    // Never touch the real keyring in tests.
    unsafe {
        std::env::set_var("NOVA_AI_NO_KEYRING", "1");
    }
    LlmClient::from_config(&cfg)
}

#[test]
fn chat_returns_text() {
    let url = start_mock(vec![chat_response("ciao, sono NovaAI")]);
    let client = test_client(&url);
    let r = client.chat(&[ChatMessage::user("ciao")], &[]).unwrap();
    assert_eq!(r.text, "ciao, sono NovaAI");
    assert!(r.tool_calls.is_empty());
}

#[test]
fn chat_parses_tool_calls() {
    let url = start_mock(vec![tool_call_response("call_1", "system_info", "{}")]);
    let client = test_client(&url);
    let r = client
        .chat(&[ChatMessage::user("info sistema")], &[])
        .unwrap();
    assert_eq!(r.tool_calls.len(), 1);
    assert_eq!(r.tool_calls[0].name, "system_info");
    assert_eq!(r.tool_calls[0].id, "call_1");
}

#[test]
fn http_error_is_honest() {
    let url = start_mock(vec![
        r#"{"error":{"message":"invalid api key"}}"#.to_string(),
    ]);
    let client = test_client(&url);
    let err = client.chat(&[ChatMessage::user("ciao")], &[]).unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("500") || msg.contains("invalid api key"),
        "{msg}"
    );
}

#[test]
fn agent_loop_runs_read_only_tool() {
    // Turn 1: model calls system_info. Turn 2: final answer.
    let url = start_mock(vec![
        tool_call_response("call_1", "system_info", "{}"),
        chat_response("Il sistema è NovaOS, tutto a posto."),
    ]);
    let mut cfg = Config::for_provider(ProviderKind::Custom);
    cfg.base_url = url;
    cfg.model = "test-model".to_string();
    cfg.api_key = Some("test-key".to_string());
    unsafe {
        std::env::set_var("NOVA_AI_NO_KEYRING", "1");
    }
    let mut agent = Agent::new(&cfg).unwrap();
    match agent.send("dammi info sul sistema", &[]) {
        nova_ai_core::ChatOutcome::Answer(a) => {
            assert!(a.contains("NovaOS"), "{a}");
        }
        other => panic!("expected Answer, got {other:?}"),
    }
}

#[test]
fn agent_loop_pauses_for_confirmation() {
    // Turn 1: model calls set_volume. The loop must stop and ask.
    let url = start_mock(vec![tool_call_response(
        "call_9",
        "set_volume",
        r#"{"level": 42}"#,
    )]);
    let mut cfg = Config::for_provider(ProviderKind::Custom);
    cfg.base_url = url;
    cfg.model = "test-model".to_string();
    cfg.api_key = Some("test-key".to_string());
    unsafe {
        std::env::set_var("NOVA_AI_NO_KEYRING", "1");
    }
    let mut agent = Agent::new(&cfg).unwrap();
    match agent.send("metti il volume a 42", &[]) {
        nova_ai_core::ChatOutcome::NeedsConfirmation(action) => {
            assert_eq!(action.tool, "set_volume");
            assert_eq!(action.args["level"], 42);
        }
        other => panic!("expected NeedsConfirmation, got {other:?}"),
    }
    // Deny: the loop resumes and the model is told the user refused.
    // (No second mock response queued -> the mock returns an error body;
    // the agent reports a provider error honestly.)
    match agent.confirm(false) {
        nova_ai_core::ChatOutcome::Error(_) => {}
        nova_ai_core::ChatOutcome::Answer(_) => {}
        other => panic!("unexpected outcome after deny: {other:?}"),
    }
}

#[test]
fn agent_without_key_errors() {
    let mut cfg = Config::for_provider(ProviderKind::Groq);
    cfg.api_key = None;
    unsafe {
        std::env::set_var("NOVA_AI_NO_KEYRING", "1");
    }
    assert!(Agent::new(&cfg).is_err());
}

#[test]
fn chat_falls_back_to_text_tool_call_when_tools_rejected() {
    // 1st request (with tools) -> 400 "tools not supported".
    // The client retries without tools; the model answers with a
    // fenced JSON tool call, which the agent executes (system_info).
    // 3rd request -> final answer.
    let (url, _seen) = start_mock_statused(vec![
        (
            400,
            r#"{"error":{"message":"tools parameter is not supported by this endpoint"}}"#
                .to_string(),
        ),
        (
            200,
            chat_response("```json\n{\"tool\": \"system_info\", \"arguments\": {}}\n```"),
        ),
        (200, chat_response("Il sistema è NovaOS, tutto a posto.")),
    ]);
    let mut cfg = Config::for_provider(ProviderKind::Custom);
    cfg.base_url = url;
    cfg.model = "test-model".to_string();
    cfg.api_key = Some("test-key".to_string());
    unsafe {
        std::env::set_var("NOVA_AI_NO_KEYRING", "1");
    }
    let mut agent = Agent::new(&cfg).unwrap();
    match agent.send("dammi info sul sistema", &[]) {
        nova_ai_core::ChatOutcome::Answer(a) => {
            assert!(a.contains("NovaOS"), "{a}");
        }
        other => panic!("expected Answer, got {other:?}"),
    }
}

#[test]
fn agent_loop_screenshot_sends_image() {
    // Fake `grim` in PATH: writes a minimal 1x1 PNG to its last arg.
    let dir = std::env::temp_dir().join(format!("nova-ai-grim-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let png_b64 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8Xw8AAwMCAO+ip1sAAAAASUVORK5CYII=";
    let script = dir.join("grim");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nfor last; do true; done\nprintf '%s' '{png_b64}' | base64 -d > \"$last\"\n"
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755));
    }
    let old_path = std::env::var("PATH").unwrap_or_default();
    unsafe {
        std::env::set_var("PATH", format!("{}:{old_path}", dir.display()));
    }
    let (url, seen) = start_mock_statused(vec![
        (200, tool_call_response("call_shot", "screenshot", "{}")),
        (
            200,
            chat_response("Vedo un desktop con una finestra aperta."),
        ),
    ]);
    let mut cfg = Config::for_provider(ProviderKind::Custom);
    cfg.base_url = url;
    // Model name with "vision" so the agent enables the image path.
    cfg.model = "test-vision-model".to_string();
    cfg.api_key = Some("test-key".to_string());
    unsafe {
        std::env::set_var("NOVA_AI_NO_KEYRING", "1");
    }
    let mut agent = Agent::new(&cfg).unwrap();
    let outcome = agent.send("cosa vedi sullo schermo?", &[]);
    unsafe {
        std::env::set_var("PATH", &old_path);
    }
    let _ = std::fs::remove_dir_all(&dir);
    match outcome {
        nova_ai_core::ChatOutcome::Answer(a) => {
            assert!(a.contains("desktop"), "{a}");
        }
        other => panic!("expected Answer, got {other:?}"),
    }
    // The second request must carry the screenshot as image_url.
    let seen = seen.lock().unwrap();
    assert!(seen.len() >= 2, "expected 2 requests, got {}", seen.len());
    assert!(
        seen[1].contains("image_url"),
        "second request has no image: {}",
        seen[1]
    );
    assert!(
        seen[1].contains("data:image/png;base64"),
        "second request has no png data url"
    );
}

#[test]
fn agent_compacts_long_history_with_summary() {
    // A single huge user message pushes the history over the 100k
    // token budget: the agent must first ask the mock for a summary,
    // then answer with the summary message in the history.
    let big = "x".repeat(500_000);
    // Isolate the memory store: compaction saves the summary there.
    let mem_dir = std::env::temp_dir().join(format!("nova-ai-compact-{}", std::process::id()));
    unsafe {
        std::env::set_var("NOVA_AI_DATA_DIR", &mem_dir);
    }
    let (url, seen) = start_mock_statused(vec![
        (
            200,
            chat_response("Riassunto di prova: l'utente ha incollato un testo lungo."),
        ),
        (200, chat_response("Ricevuto, è un testo molto lungo.")),
    ]);
    let mut cfg = Config::for_provider(ProviderKind::Custom);
    cfg.base_url = url;
    cfg.model = "test-model".to_string();
    cfg.api_key = Some("test-key".to_string());
    unsafe {
        std::env::set_var("NOVA_AI_NO_KEYRING", "1");
    }
    let mut agent = Agent::new(&cfg).unwrap();
    match agent.send(&big, &[]) {
        nova_ai_core::ChatOutcome::Answer(a) => {
            assert!(a.contains("molto lungo"), "{a}");
        }
        other => panic!("expected Answer, got {other:?}"),
    }
    let seen = seen.lock().unwrap();
    assert!(seen.len() >= 2, "expected 2 requests, got {}", seen.len());
    // The second request (the real turn) carries the summary message.
    assert!(
        seen[1].contains("Riassunto della conversazione finora"),
        "main request has no summary: {}",
        &seen[1][..seen[1].len().min(500)]
    );
    assert!(
        seen[1].contains("Riassunto di prova"),
        "main request lost the summary text"
    );
    // The summary was also saved in the (isolated) memory store.
    let saved = std::fs::read_to_string(mem_dir.join("memory").join("summary.txt")).unwrap();
    assert!(saved.contains("Riassunto di prova"), "{saved}");
    unsafe {
        std::env::remove_var("NOVA_AI_DATA_DIR");
    }
    let _ = std::fs::remove_dir_all(&mem_dir);
}

#[test]
fn agent_with_local_provider_needs_no_key() {
    // Local (Ollama) works without an API key: the client must not
    // demand one, and must not send an Authorization header.
    let (url, seen) = start_mock_statused(vec![(200, chat_response("ok locale"))]);
    let mut cfg = Config::for_provider(ProviderKind::Local);
    cfg.base_url = url;
    cfg.model = "qwen2.5:7b".to_string();
    cfg.api_key = None;
    unsafe {
        std::env::set_var("NOVA_AI_NO_KEYRING", "1");
    }
    let mut agent = Agent::new(&cfg).unwrap();
    match agent.send("ciao", &[]) {
        nova_ai_core::ChatOutcome::Answer(a) => {
            assert!(a.contains("locale"), "{a}");
        }
        other => panic!("expected Answer, got {other:?}"),
    }
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert!(
        !seen[0].contains("Authorization"),
        "local request sent an Authorization header"
    );
}
