//! Voice input (v1): record the microphone with `pw-record` (PipeWire)
//! while the UI button is held, then transcribe the WAV via the
//! provider's Whisper endpoint (see LlmClient::transcribe).
//!
//! `pw-record` is the v1 mechanism: no extra native audio dependency
//! (cpal was considered and rejected for v1 — it needs ALSA/JACK dev
//! headers at build time and adds a native dependency for a feature
//! PipeWire already covers on NovaOS). TTS replies are OUT OF SCOPE
//! for v1.

use std::io::Read;
use std::process::{Child, Command, Stdio};

/// A running recording. Drop/stop it to finish.
pub struct Recording {
    child: Child,
    path: std::path::PathBuf,
}

impl Recording {
    /// Start recording to a temp WAV file (16 kHz, mono, 16-bit).
    /// Returns an error with a clear message when `pw-record` is
    /// missing (e.g. not on a PipeWire system).
    pub fn start() -> Result<Recording, String> {
        let mut path = std::env::temp_dir();
        path.push(format!("nova-ai-voice-{}.wav", std::process::id()));
        let child = Command::new("pw-record")
            .args([
                "--format",
                "s16",
                "--rate",
                "16000",
                "--channels",
                "1",
                path.to_str().unwrap_or("/tmp/nova-ai-voice.wav"),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    "pw-record non trovato: l'input vocale richiede PipeWire".to_string()
                } else {
                    format!("impossibile avviare la registrazione: {e}")
                }
            })?;
        Ok(Recording { child, path })
    }

    /// Stop the recording and read the WAV bytes. Returns the bytes;
    /// an empty/short file means nothing was recorded.
    pub fn stop(mut self) -> Result<Vec<u8>, String> {
        // SIGTERM via kill is enough: pw-record finalizes the WAV header.
        let _ = self.child.kill();
        let _ = self.child.wait();
        // Give the file a moment to flush.
        std::thread::sleep(std::time::Duration::from_millis(150));
        let mut f = std::fs::File::open(&self.path)
            .map_err(|e| format!("registrazione non leggibile: {e}"))?;
        let mut bytes = Vec::new();
        f.read_to_end(&mut bytes)
            .map_err(|e| format!("lettura registrazione fallita: {e}"))?;
        let _ = std::fs::remove_file(&self.path);
        if bytes.len() < 1000 {
            return Err("registrazione troppo breve (nessun audio?)".to_string());
        }
        Ok(bytes)
    }
}

/// Is voice input available at all on this system? (pw-record present)
pub fn voice_available() -> bool {
    Command::new("which")
        .arg("pw-record")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
