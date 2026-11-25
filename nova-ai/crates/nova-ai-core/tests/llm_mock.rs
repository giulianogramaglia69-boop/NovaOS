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
    let queue: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(responses.into()));
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
            // Read the body, if any.
            let mut content_len = 0usize;
            if let Some(end) = header_end {
                let headers = String::from_utf8_lossy(&buf[..end]).to_string();
                for line in headers.lines() {
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        content_len = v.trim().parse().unwrap_or(0);
                    }
                }
                let already = buf.len() - end;
                let mut remaining = content_len.saturating_sub(already);
                while remaining > 0 {
                    match stream.read(&mut chunk) {
                        Ok(0) => break,
                        Ok(n) => {
                            remaining = remaining.saturating_sub(n);
                        }
                        Err(_) => break,
                    }
                }
            }
            let body = queue
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| r#"{"error":{"message":"mock esaurito"}}"#.to_string());
            let status = if body.contains("\"error\"") && !body.contains("choices") {
                "500 Internal Server Error"
            } else {
                "200 OK"
            };
            let resp = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
        }
    });
    format!("http://127.0.0.1:{port}")
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
