//! A real browser, driven over WebDriver HTTP (no websocket):
//! geckodriver + headless Firefox. geckodriver is downloaded on
//! first use into `~/.local/share/nova-ai/geckodriver/` (same lazy
//! pattern as Piper/Vosk); Firefox comes from the image.
//!
//! The session is process-wide (one Firefox for the whole app
//! lifetime): `navigate` / `find` / `click` / `type` / `text` /
//! `screenshot` reuse it, `close` kills it. Click and type change
//! web state, so the tool layer asks the user to confirm them first
//! (like any other mutating action).
//!
//! Honest limits: no JavaScript evaluation (the W3C endpoint for it
//! exists, but executing arbitrary page JS from the model is a
//! footgun — deliberately left out), no file uploads, and a page
//! that needs a login the user has not done in Firefox will not log
//! in by itself.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::Mutex;
use std::time::Duration;

use serde_json::{Value, json};

/// geckodriver release pinned here (verified 2026-11-26 on the
/// mozilla/geckodriver releases page).
pub const GECKODRIVER_VERSION: &str = "0.37.1";
const GECKODRIVER_URL: &str = "https://github.com/mozilla/geckodriver/releases/download/v0.37.1/geckodriver-v0.37.1-linux64.tar.gz";

/// W3C element reference key.
const ELEMENT_KEY: &str = "element-6066-11e4-a52e-4f735466cecf";

pub fn geckodriver_dir() -> PathBuf {
    crate::data_dir().join("geckodriver")
}

pub fn geckodriver_bin() -> PathBuf {
    geckodriver_dir().join("geckodriver")
}

pub fn geckodriver_present() -> bool {
    geckodriver_bin().is_file()
}

fn download_bytes(url: &str) -> Result<Vec<u8>, String> {
    let resp = crate::llm::LlmClient::http_agent()
        .get(url)
        .call()
        .map_err(|e| format!("download geckodriver fallito: {e}"))?;
    if resp.status().as_u16() != 200 {
        return Err(format!(
            "download geckodriver fallito: HTTP {}",
            resp.status().as_u16()
        ));
    }
    let mut reader = resp.into_body().into_reader();
    let mut out = Vec::new();
    reader
        .read_to_end(&mut out)
        .map_err(|e| format!("lettura download fallita: {e}"))?;
    Ok(out)
}

/// Download + extract geckodriver on first use. Returns the binary
/// path. A failed download leaves nothing behind.
pub fn ensure_geckodriver() -> Result<PathBuf, String> {
    let bin = geckodriver_bin();
    if bin.is_file() {
        return Ok(bin);
    }
    let base = geckodriver_dir();
    std::fs::create_dir_all(&base).map_err(|e| format!("impossibile creare {base:?}: {e}"))?;
    let bytes = download_bytes(GECKODRIVER_URL)?;
    let tmp = base.join(format!(".geckodriver.tmp-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).map_err(|e| format!("impossibile estrarre geckodriver: {e}"))?;
    let result = (|| -> Result<(), String> {
        let gz = flate2::read::GzDecoder::new(&bytes[..]);
        let mut archive = tar::Archive::new(gz);
        archive
            .unpack(&tmp)
            .map_err(|e| format!("archivio geckodriver non valido: {e}"))?;
        Ok(())
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(e);
    }
    let extracted = tmp.join("geckodriver");
    if !extracted.is_file() {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err("archivio geckodriver non contiene il binario atteso".to_string());
    }
    let dest = bin.clone();
    if dest.exists() {
        let _ = std::fs::remove_file(&dest);
    }
    std::fs::rename(&extracted, &dest)
        .map_err(|e| format!("installazione geckodriver fallita: {e}"))?;
    let _ = std::fs::remove_dir_all(&tmp);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755));
    }
    Ok(dest)
}

// ---------------------------------------------------------------------------
// WebDriver client (one session)
// ---------------------------------------------------------------------------

/// A WebDriver session against a geckodriver HTTP endpoint.
pub struct WebDriver {
    base_url: String,
    session_id: Option<String>,
    driver_child: Option<Child>,
}

impl WebDriver {
    /// Connect to an already-running driver (tests / externally
    /// managed geckodriver) and create a session.
    pub fn connect(base_url: &str) -> Result<WebDriver, String> {
        let mut wd = WebDriver {
            base_url: base_url.trim_end_matches('/').to_string(),
            session_id: None,
            driver_child: None,
        };
        wd.new_session()?;
        Ok(wd)
    }

    /// Start geckodriver on a free port (4444..4454), wait for it,
    /// and create a headless Firefox session.
    pub fn launch() -> Result<WebDriver, String> {
        let bin = ensure_geckodriver()?;
        for port in 4444u16..4454 {
            let child = Command::new(&bin)
                .arg("--port")
                .arg(port.to_string())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
            let mut child = match child {
                Ok(c) => c,
                Err(e) => return Err(format!("impossibile avviare geckodriver: {e}")),
            };
            let base_url = format!("http://127.0.0.1:{port}");
            if wait_for_driver(&base_url, 10) {
                let mut wd = WebDriver {
                    base_url,
                    session_id: None,
                    driver_child: Some(child),
                };
                // On failure wd drops here and its Drop kills the child.
                wd.new_session()?;
                return Ok(wd);
            }
            let _ = child.kill();
            let _ = child.wait();
        }
        Err("geckodriver non risponde su nessuna porta (4444-4453)".to_string())
    }

    fn agent() -> ureq::Agent {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(60)))
            .build();
        ureq::Agent::new_with_config(config)
    }

    fn post(&self, path: &str, body: Value) -> Result<Value, String> {
        let url = format!("{}{}", self.base_url, path);
        let resp = Self::agent()
            .post(&url)
            .header("Content-Type", "application/json")
            .send_json(&body)
            .map_err(|e| format!("errore WebDriver (POST {path}): {e}"))?;
        self.parse_response(resp)
    }

    fn get(&self, path: &str) -> Result<Value, String> {
        let url = format!("{}{}", self.base_url, path);
        let resp = Self::agent()
            .get(&url)
            .call()
            .map_err(|e| format!("errore WebDriver (GET {path}): {e}"))?;
        self.parse_response(resp)
    }

    fn delete(&self, path: &str) -> Result<Value, String> {
        let url = format!("{}{}", self.base_url, path);
        let resp = Self::agent()
            .delete(&url)
            .call()
            .map_err(|e| format!("errore WebDriver (DELETE {path}): {e}"))?;
        self.parse_response(resp)
    }

    fn parse_response(&self, resp: ureq::http::Response<ureq::Body>) -> Result<Value, String> {
        let status = resp.status().as_u16();
        let v: Value = resp.into_body().read_json().unwrap_or(Value::Null);
        if !(200..300).contains(&status) {
            let msg = v
                .get("value")
                .and_then(|x| x.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("errore WebDriver");
            return Err(format!("WebDriver HTTP {status}: {msg}"));
        }
        Ok(v.get("value").cloned().unwrap_or(Value::Null))
    }

    fn session_path(&self, suffix: &str) -> Result<String, String> {
        match &self.session_id {
            Some(id) => Ok(format!("/session/{id}{suffix}")),
            None => Err("nessuna sessione browser attiva".to_string()),
        }
    }

    fn new_session(&mut self) -> Result<(), String> {
        let v = self.post(
            "/session",
            json!({ "capabilities": { "alwaysMatch": { "moz:firefoxOptions": { "args": ["-headless"] } } } }),
        )?;
        let id = v
            .get("sessionId")
            .and_then(|s| s.as_str())
            .ok_or_else(|| "geckodriver non ha restituito un sessionId".to_string())?;
        self.session_id = Some(id.to_string());
        Ok(())
    }

    pub fn navigate(&mut self, url: &str) -> Result<(), String> {
        let path = self.session_path("/url")?;
        self.post(&path, json!({ "url": url }))?;
        Ok(())
    }

    pub fn title(&self) -> Result<String, String> {
        let path = self.session_path("/title")?;
        let v = self.get(&path)?;
        Ok(v.as_str().unwrap_or("").to_string())
    }

    /// Find one element by CSS selector. Returns the element id.
    pub fn find(&self, selector: &str) -> Result<String, String> {
        let path = self.session_path("/element")?;
        let v = self.post(&path, json!({ "using": "css selector", "value": selector }))?;
        v.get(ELEMENT_KEY)
            .or_else(|| v.get("ELEMENT"))
            .and_then(|e| e.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| format!("elemento non trovato: {selector}"))
    }

    pub fn click(&self, element_id: &str) -> Result<(), String> {
        let path = self.session_path(&format!("/element/{element_id}/click"))?;
        self.post(&path, json!({}))?;
        Ok(())
    }

    pub fn type_text(&self, element_id: &str, text: &str) -> Result<(), String> {
        let clear = self.session_path(&format!("/element/{element_id}/clear"))?;
        let _ = self.post(&clear, json!({}));
        let path = self.session_path(&format!("/element/{element_id}/value"))?;
        self.post(&path, json!({ "text": text }))?;
        Ok(())
    }

    pub fn element_text(&self, element_id: &str) -> Result<String, String> {
        let path = self.session_path(&format!("/element/{element_id}/text"))?;
        let v = self.get(&path)?;
        Ok(v.as_str().unwrap_or("").to_string())
    }

    /// Visible text of the page body (truncated).
    pub fn page_text(&self) -> Result<String, String> {
        let body = self.find("body")?;
        let t = self.element_text(&body)?;
        Ok(crate::tools::truncate_public(&t, 8000))
    }

    /// Screenshot as base64 PNG.
    pub fn screenshot_png_b64(&self) -> Result<String, String> {
        let path = self.session_path("/screenshot")?;
        let v = self.get(&path)?;
        v.as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| "screenshot WebDriver senza dati".to_string())
    }

    pub fn close(&mut self) -> Result<(), String> {
        if let Some(id) = self.session_id.take() {
            let _ = self.delete(&format!("/session/{id}"));
        }
        if let Some(mut child) = self.driver_child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        Ok(())
    }
}

impl Drop for WebDriver {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

fn wait_for_driver(base_url: &str, secs: u64) -> bool {
    let agent = WebDriver::agent();
    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    while std::time::Instant::now() < deadline {
        if let Ok(resp) = agent.get(&format!("{base_url}/status")).call()
            && resp.status().as_u16() == 200
        {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

// ---------------------------------------------------------------------------
// Process-wide session (what the tool layer uses)
// ---------------------------------------------------------------------------

static SESSION: Mutex<Option<WebDriver>> = Mutex::new(None);
/// Test override: when set, the shared session connects here instead
/// of spawning geckodriver (mock WebDriver server).
static TEST_BASE: Mutex<Option<String>> = Mutex::new(None);

pub fn set_test_base(url: Option<String>) {
    *TEST_BASE.lock().unwrap_or_else(|e| e.into_inner()) = url;
}

fn ensure_session() -> Result<(), String> {
    let mut guard = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_some() {
        return Ok(());
    }
    let test_base = TEST_BASE.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let wd = match test_base {
        Some(base) => WebDriver::connect(&base)?,
        None => WebDriver::launch()?,
    };
    *guard = Some(wd);
    Ok(())
}

fn with_session<T>(f: impl FnOnce(&mut WebDriver) -> Result<T, String>) -> Result<T, String> {
    ensure_session()?;
    let mut guard = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    let wd = guard
        .as_mut()
        .ok_or_else(|| "nessuna sessione browser".to_string())?;
    f(wd)
}

pub fn browser_navigate(url: &str) -> Result<String, String> {
    with_session(|wd| {
        wd.navigate(url)?;
        let title = wd.title().unwrap_or_default();
        if title.is_empty() {
            Ok(format!("Aperto {url}"))
        } else {
            Ok(format!("Aperto {url} — titolo: {title}"))
        }
    })
}

pub fn browser_find(selector: &str) -> Result<String, String> {
    with_session(|wd| {
        let el = wd.find(selector)?;
        Ok(format!("Elemento trovato: {selector} (id {el})"))
    })
}

pub fn browser_click(selector: &str) -> Result<String, String> {
    with_session(|wd| {
        let el = wd.find(selector)?;
        wd.click(&el)?;
        Ok(format!("Cliccato {selector}"))
    })
}

pub fn browser_type(selector: &str, text: &str) -> Result<String, String> {
    with_session(|wd| {
        let el = wd.find(selector)?;
        wd.type_text(&el, text)?;
        Ok(format!("Scritto in {selector}"))
    })
}

pub fn browser_text() -> Result<String, String> {
    with_session(|wd| wd.page_text())
}

pub fn browser_screenshot_b64() -> Result<String, String> {
    with_session(|wd| wd.screenshot_png_b64())
}

pub fn browser_close() -> Result<String, String> {
    let mut guard = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(mut wd) = guard.take() {
        wd.close()?;
        Ok("Browser chiuso.".to_string())
    } else {
        Ok("Nessun browser aperto.".to_string())
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    //! Mock WebDriver server for tests (also used by tools.rs tests).
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// Serializes the tests that use the process-wide TEST_BASE
    /// override: they each set it to their own mock server and clear
    /// it at the end, so two of them running in parallel would make
    /// one of them fall back to a real geckodriver download.
    pub static TEST_BASE_LOCK: Mutex<()> = Mutex::new(());
    /// Start a mock WebDriver server; returns its base URL.
    pub fn start_mock_wd() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = match stream {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
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
                                buf.extend_from_slice(&chunk[..n]);
                                remaining = remaining.saturating_sub(n);
                            }
                            Err(_) => break,
                        }
                    }
                }
                let head = String::from_utf8_lossy(&buf).to_string();
                let mut lines = head.lines();
                let request_line = lines.next().unwrap_or("");
                let mut parts = request_line.split_whitespace();
                let method = parts.next().unwrap_or("");
                let path = parts.next().unwrap_or("");
                let body = mock_wd_response(method, path);
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
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

    fn mock_wd_response(method: &str, path: &str) -> String {
        let value: Value = if method == "POST" && path == "/session" {
            json!({ "sessionId": "mock-session", "capabilities": {} })
        } else if method == "POST" && path.ends_with("/url") {
            json!(null)
        } else if method == "GET" && path.ends_with("/title") {
            json!("Pagina di prova")
        } else if method == "POST" && path.ends_with("/element") {
            json!({ ELEMENT_KEY: "mock-el-1" })
        } else if method == "POST"
            && (path.contains("/click") || path.contains("/clear") || path.contains("/value"))
        {
            json!(null)
        } else if method == "GET" && path.ends_with("/text") {
            json!("Ciao dal mock WebDriver")
        } else if method == "GET" && path.ends_with("/screenshot") {
            json!(
                "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8Xw8AAwMCAO+ip1sAAAAASUVORK5CYII="
            )
        } else {
            json!(null)
        };
        json!({ "value": value }).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn webdriver_flow_against_mock() {
        let base = test_support::start_mock_wd();
        let mut wd = WebDriver::connect(&base).unwrap();
        assert_eq!(wd.session_id.as_deref(), Some("mock-session"));
        wd.navigate("https://example.org").unwrap();
        assert_eq!(wd.title().unwrap(), "Pagina di prova");
        let el = wd.find("input[name=q]").unwrap();
        assert_eq!(el, "mock-el-1");
        wd.click(&el).unwrap();
        wd.type_text(&el, "ciao").unwrap();
        assert_eq!(wd.page_text().unwrap(), "Ciao dal mock WebDriver");
        let png = wd.screenshot_png_b64().unwrap();
        assert!(png.starts_with("iVBORw0KGgo"));
        wd.close().unwrap();
        let _ = Arc::new(());
    }

    #[test]
    fn shared_session_uses_test_base() {
        let _lock = test_support::TEST_BASE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let base = test_support::start_mock_wd();
        set_test_base(Some(base));
        let out = browser_navigate("https://example.org").unwrap();
        assert!(out.contains("Pagina di prova"), "{out}");
        let text = browser_text().unwrap();
        assert!(text.contains("mock WebDriver"), "{text}");
        let shot = browser_screenshot_b64().unwrap();
        assert!(shot.starts_with("iVBORw0KGgo"));
        assert_eq!(browser_close().unwrap(), "Browser chiuso.");
        set_test_base(None);
    }
}
