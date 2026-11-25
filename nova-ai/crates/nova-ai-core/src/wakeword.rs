//! Wake word "Ehi Nova": offline detection with Vosk.
//!
//! Nothing leaves the device for detection: the small Italian Vosk model
//! runs locally on the microphone stream, with a constrained grammar
//! (`["ehi nova"]`) so CPU stays low. The flow is:
//!
//! 1. The user opts in (default OFF) in setup or settings.
//! 2. On first use the model (~47 MB) is downloaded to
//!    `~/.local/share/nova-ai/models/vosk-model-small-it-0.22/`.
//! 3. A listener thread captures the mic (cpal, 16 kHz mono) and feeds
//!    a [`WakeDetector`]; on detection it switches to capturing the
//!    following command (max 10 s), then pauses until the UI resumes it.
//!
//! The detection state machine ([`WakeDetector`]) and the model
//! download are plain Rust and always compiled; only the Vosk
//! recognizer and the cpal listener live behind the `wakeword` cargo
//! feature (vosk-sys links a system `libvosk.so`, cpal needs ALSA
//! headers — both may be absent on a build machine). Without the
//! feature the rest of NovaAI compiles and runs, just without wake
//! word.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// The wake phrase (Italian).
pub const WAKE_PHRASE: &str = "ehi nova";
/// Vosk model used for the wake word.
pub const MODEL_NAME: &str = "vosk-model-small-it-0.22";
/// Where the model zip comes from (verified: HTTP 200, ~47 MB).
pub const MODEL_URL: &str = "https://alphacephei.com/vosk/models/vosk-model-small-it-0.22.zip";
/// Marker file inside a complete model directory.
pub const MODEL_FILE: &str = "am/final.mdl";
/// Audio rate Vosk wants (Hz, mono, 16-bit).
pub const SAMPLE_RATE: u32 = 16_000;
/// Max command length captured after the wake phrase (seconds).
pub const COMMAND_MAX_SECS: u32 = 10;
/// Same, in samples.
pub const COMMAND_MAX_SAMPLES: usize = (SAMPLE_RATE as usize) * (COMMAND_MAX_SECS as usize);

/// Grammar for the recognizer: only the wake phrase. A constrained
/// grammar keeps the small model fast and avoids false positives from
/// ordinary speech.
pub fn grammar_json() -> String {
    format!("[\"{WAKE_PHRASE}\"]")
}

/// Base directory for downloaded models:
/// `~/.local/share/nova-ai/models`. Tests can override it with
/// `NOVA_AI_MODELS_DIR`.
pub fn models_base_dir() -> PathBuf {
    if let Ok(p) = std::env::var("NOVA_AI_MODELS_DIR") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".local")
        .join("share")
        .join("nova-ai")
        .join("models")
}

/// Full path of the wake-word model directory.
pub fn model_dir() -> PathBuf {
    models_base_dir().join(MODEL_NAME)
}

pub fn model_dir_in(base: &Path) -> PathBuf {
    base.join(MODEL_NAME)
}

/// True when the model directory holds a complete model.
pub fn model_present() -> bool {
    model_present_in(&model_dir())
}

pub fn model_present_in(dir: &Path) -> bool {
    dir.join(MODEL_FILE).is_file()
}

// ---------------------------------------------------------------------------
// Model download
// ---------------------------------------------------------------------------

/// Download the model zip, unzip it under the models directory and
/// return the model dir. `progress(downloaded, total)` is called as
/// bytes arrive (total is 0 when the server hides Content-Length).
/// The model is extracted to a temp dir first and only moved into
/// place once complete, so a failed download never leaves a broken
/// model behind. Never commits anything to the repo: the model lives
/// in the user's data dir, not in the source tree.
pub fn download_model(progress: impl FnMut(u64, u64)) -> Result<PathBuf, String> {
    download_model_from(MODEL_URL, &models_base_dir(), progress)
}

pub fn download_model_from(
    url: &str,
    base: &Path,
    mut progress: impl FnMut(u64, u64),
) -> Result<PathBuf, String> {
    fs::create_dir_all(base).map_err(|e| format!("impossibile creare {base:?}: {e}"))?;
    let resp = crate::llm::LlmClient::http_agent()
        .get(url)
        .call()
        .map_err(|e| match e {
            ureq::Error::Timeout(_) => "timeout durante il download del modello".to_string(),
            ureq::Error::Io(e) if e.kind() == std::io::ErrorKind::NotFound => {
                "download fallito: controlla la connessione (sei offline?)".to_string()
            }
            other => format!("download del modello fallito: {other}"),
        })?;
    if resp.status().as_u16() != 200 {
        return Err(format!(
            "download del modello fallito: HTTP {}",
            resp.status().as_u16()
        ));
    }
    let total: u64 = resp
        .headers()
        .get("Content-Length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    let mut zip_path = base.to_path_buf();
    zip_path.push(format!(".{MODEL_NAME}.zip-{}", std::process::id()));
    let mut zip_file =
        fs::File::create(&zip_path).map_err(|e| format!("impossibile scrivere il modello: {e}"))?;
    let mut reader = resp.into_body().into_reader();
    let mut buf = [0u8; 32768];
    let mut downloaded: u64 = 0;
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("lettura download fallita: {e}"))?;
        if n == 0 {
            break;
        }
        zip_file
            .write_all(&buf[..n])
            .map_err(|e| format!("scrittura modello fallita: {e}"))?;
        downloaded += n as u64;
        progress(downloaded, total);
    }
    drop(zip_file);

    let out = extract_model(&zip_path, base);
    let _ = fs::remove_file(&zip_path);
    out
}

fn extract_model(zip_path: &Path, base: &Path) -> Result<PathBuf, String> {
    let file =
        fs::File::open(zip_path).map_err(|e| format!("impossibile aprire il modello: {e}"))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| format!("modello scaricato non valido: {e}"))?;
    let tmp = base.join(format!(".{MODEL_NAME}.tmp-{}", std::process::id()));
    fs::create_dir_all(&tmp).map_err(|e| format!("impossibile estrarre il modello: {e}"))?;
    let result = (|| -> Result<(), String> {
        for i in 0..archive.len() {
            let mut entry = archive
                .by_index(i)
                .map_err(|e| format!("modello scaricato non valido: {e}"))?;
            let Some(name) = entry.enclosed_name() else {
                continue;
            };
            let dest = tmp.join(name);
            if entry.is_dir() {
                fs::create_dir_all(&dest)
                    .map_err(|e| format!("impossibile estrarre il modello: {e}"))?;
            } else {
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|e| format!("impossibile estrarre il modello: {e}"))?;
                }
                let mut out = fs::File::create(&dest)
                    .map_err(|e| format!("impossibile estrarre il modello: {e}"))?;
                std::io::copy(&mut entry, &mut out)
                    .map_err(|e| format!("impossibile estrarre il modello: {e}"))?;
            }
        }
        Ok(())
    })();
    if let Err(e) = result {
        let _ = fs::remove_dir_all(&tmp);
        return Err(e);
    }
    let extracted = tmp.join(MODEL_NAME);
    if !model_present_in(&extracted) {
        let _ = fs::remove_dir_all(&tmp);
        return Err("modello scaricato incompleto (manca am/final.mdl)".to_string());
    }
    let dest = model_dir_in(base);
    let _ = fs::remove_dir_all(&dest);
    fs::rename(&extracted, &dest).map_err(|e| format!("impossibile installare il modello: {e}"))?;
    let _ = fs::remove_dir_all(&tmp);
    Ok(dest)
}

// ---------------------------------------------------------------------------
// Audio helpers
// ---------------------------------------------------------------------------

/// Encode mono 16-bit 16 kHz samples as a WAV file (44-byte header),
/// ready for the Whisper endpoint.
pub fn samples_to_wav(samples: &[i16]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    let push = |v: &mut Vec<u8>, s: &[u8]| v.extend_from_slice(s);
    push(&mut out, b"RIFF");
    push(&mut out, &(36 + data_len).to_le_bytes());
    push(&mut out, b"WAVE");
    push(&mut out, b"fmt ");
    push(&mut out, &16u32.to_le_bytes());
    push(&mut out, &1u16.to_le_bytes()); // PCM
    push(&mut out, &1u16.to_le_bytes()); // mono
    push(&mut out, &SAMPLE_RATE.to_le_bytes());
    push(&mut out, &(SAMPLE_RATE * 2).to_le_bytes()); // byte rate
    push(&mut out, &2u16.to_le_bytes()); // block align
    push(&mut out, &16u16.to_le_bytes()); // bits
    push(&mut out, b"data");
    push(&mut out, &data_len.to_le_bytes());
    for s in samples {
        push(&mut out, &s.to_le_bytes());
    }
    out
}

/// Convert captured audio to mono 16 kHz i16: downmix the channels
/// (average) and linear-resample when the source rate differs.
pub fn normalize_f32(raw: &[f32], channels: usize, from_rate: u32) -> Vec<i16> {
    if channels == 0 || raw.is_empty() {
        return Vec::new();
    }
    let frames = raw.len() / channels;
    let mut mono = Vec::with_capacity(frames);
    for f in 0..frames {
        let mut acc = 0.0f32;
        for c in 0..channels {
            acc += raw[f * channels + c];
        }
        mono.push(acc / channels as f32);
    }
    resample_mono(&mono, from_rate)
}

pub fn normalize_i16(raw: &[i16], channels: usize, from_rate: u32) -> Vec<i16> {
    let as_f32: Vec<f32> = raw.iter().map(|s| *s as f32 / 32768.0).collect();
    normalize_f32(&as_f32, channels, from_rate)
}

fn resample_mono(mono: &[f32], from_rate: u32) -> Vec<i16> {
    if from_rate == SAMPLE_RATE {
        return mono
            .iter()
            .map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i16)
            .collect();
    }
    if from_rate == 0 || mono.is_empty() {
        return Vec::new();
    }
    let ratio = from_rate as f64 / SAMPLE_RATE as f64;
    let out_len = (mono.len() as f64 / ratio) as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let pos = i as f64 * ratio;
        let idx = pos as usize;
        let frac = (pos - idx as f64) as f32;
        let a = mono[idx];
        let b = mono.get(idx + 1).copied().unwrap_or(a);
        let s = a + (b - a) * frac;
        out.push((s.clamp(-1.0, 1.0) * 32767.0) as i16);
    }
    out
}

// ---------------------------------------------------------------------------
// Detection state machine (no Vosk here: testable with a mock)
// ---------------------------------------------------------------------------

/// Events produced by the wake-word detector.
#[derive(Debug, Clone, PartialEq)]
pub enum WakeEvent {
    /// The phrase was heard; the command window starts now.
    Detected,
    /// The command audio (16 kHz mono, max 10 s) after the phrase.
    CommandReady(Vec<i16>),
    /// Something failed in the listener (mic gone, model unloadable…).
    Error(String),
}

/// Anything that can say whether a chunk contains the wake phrase.
/// The real implementation wraps a Vosk recognizer with the grammar
/// `["ehi nova"]`; tests use a mock.
pub trait WakeRecognizer {
    /// True when this chunk completes the wake phrase.
    fn accept(&mut self, samples: &[i16]) -> bool;
    /// Forget partial state (called when listening resumes).
    fn reset(&mut self) {}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DetectorState {
    Listening,
    Capturing,
    Paused,
}

/// Listens for the wake phrase, then captures the following command.
///
/// States: Listening → (phrase) → Capturing → (10 s of audio) →
/// Paused. While Paused it discards audio until `set_paused(false)`.
pub struct WakeDetector<R: WakeRecognizer> {
    recognizer: R,
    state: DetectorState,
    buffer: Vec<i16>,
}

impl<R: WakeRecognizer> WakeDetector<R> {
    pub fn new(recognizer: R) -> Self {
        WakeDetector {
            recognizer,
            state: DetectorState::Listening,
            buffer: Vec::new(),
        }
    }

    pub fn is_paused(&self) -> bool {
        self.state == DetectorState::Paused
    }

    /// Pause (discard audio) or resume listening. Resuming resets the
    /// recognizer so a stale partial cannot fire immediately.
    pub fn set_paused(&mut self, paused: bool) {
        match (paused, self.state) {
            (true, DetectorState::Listening) | (true, DetectorState::Capturing) => {
                self.state = DetectorState::Paused;
                self.buffer.clear();
            }
            (false, DetectorState::Paused) => {
                self.state = DetectorState::Listening;
                self.recognizer.reset();
                self.buffer.clear();
            }
            _ => {}
        }
    }

    /// Feed one chunk of 16 kHz mono audio. Returns an event when the
    /// wake phrase is detected or the command window is complete.
    pub fn feed(&mut self, samples: &[i16]) -> Option<WakeEvent> {
        match self.state {
            DetectorState::Paused => None,
            DetectorState::Listening => {
                if self.recognizer.accept(samples) {
                    self.state = DetectorState::Capturing;
                    self.buffer.clear();
                    Some(WakeEvent::Detected)
                } else {
                    None
                }
            }
            DetectorState::Capturing => {
                self.buffer.extend_from_slice(samples);
                if self.buffer.len() >= COMMAND_MAX_SAMPLES {
                    self.buffer.truncate(COMMAND_MAX_SAMPLES);
                    self.state = DetectorState::Paused;
                    Some(WakeEvent::CommandReady(std::mem::take(&mut self.buffer)))
                } else {
                    None
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Vosk recognizer + cpal listener (feature `wakeword` only)
// ---------------------------------------------------------------------------

#[cfg(feature = "wakeword")]
pub use listener::WakeWordHandle;

#[cfg(feature = "wakeword")]
mod listener {
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, mpsc};
    use std::thread::JoinHandle;
    use std::time::Duration;

    use super::{
        COMMAND_MAX_SAMPLES, SAMPLE_RATE, WAKE_PHRASE, WakeDetector, WakeEvent, WakeRecognizer,
        normalize_f32, normalize_i16,
    };

    /// Vosk recognizer with the wake-word grammar.
    pub struct VoskRecognizer {
        recognizer: vosk::Recognizer,
    }

    impl VoskRecognizer {
        pub fn new(model_dir: &Path) -> Result<Self, String> {
            let path = model_dir
                .to_str()
                .ok_or_else(|| format!("percorso modello non valido: {}", model_dir.display()))?;
            let model = vosk::Model::new(path).ok_or_else(|| {
                format!(
                    "impossibile caricare il modello Vosk in {}",
                    model_dir.display()
                )
            })?;
            let recognizer =
                vosk::Recognizer::new_with_grammar(&model, SAMPLE_RATE as f32, &[WAKE_PHRASE])
                    .ok_or_else(|| "impossibile creare il riconoscitore Vosk".to_string())?;
            Ok(Self { recognizer })
        }
    }

    impl WakeRecognizer for VoskRecognizer {
        fn accept(&mut self, samples: &[i16]) -> bool {
            let Ok(state) = self.recognizer.accept_waveform(samples) else {
                return false;
            };
            let text = match state {
                vosk::DecodingState::Finalized => self
                    .recognizer
                    .result()
                    .single()
                    .map(|r| r.text.to_string())
                    .unwrap_or_default(),
                vosk::DecodingState::Running => {
                    self.recognizer.partial_result().partial.to_string()
                }
                vosk::DecodingState::Failed => return false,
            };
            text.to_ascii_lowercase().contains(WAKE_PHRASE)
        }

        fn reset(&mut self) {
            self.recognizer.reset();
        }
    }

    /// A running wake-word listener. Owns the mic thread; `stop()`
    /// joins it. The event callback runs on the listener thread.
    pub struct WakeWordHandle {
        stop: Arc<AtomicBool>,
        paused: Arc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
    }

    impl WakeWordHandle {
        pub fn start<F>(model_dir: &Path, on_event: F) -> Result<Self, String>
        where
            F: Fn(WakeEvent) + Send + 'static,
        {
            let stop = Arc::new(AtomicBool::new(false));
            let paused = Arc::new(AtomicBool::new(false));
            let dir = model_dir.to_path_buf();
            let stop2 = stop.clone();
            let paused2 = paused.clone();
            let thread = std::thread::Builder::new()
                .name("nova-ai-wakeword".to_string())
                .spawn(move || {
                    if let Err(e) = run_listener(&dir, &on_event, &stop2, &paused2) {
                        on_event(WakeEvent::Error(e));
                    }
                })
                .map_err(|e| format!("impossibile avviare il thread di ascolto: {e}"))?;
            Ok(Self {
                stop,
                paused,
                thread: Some(thread),
            })
        }

        pub fn pause(&self) {
            self.paused.store(true, Ordering::Relaxed);
        }

        pub fn resume(&self) {
            self.paused.store(false, Ordering::Relaxed);
        }

        pub fn is_paused(&self) -> bool {
            self.paused.load(Ordering::Relaxed)
        }

        pub fn stop(mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }

    impl Drop for WakeWordHandle {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }

    fn run_listener(
        model_dir: &Path,
        on_event: &dyn Fn(WakeEvent),
        stop: &AtomicBool,
        paused: &AtomicBool,
    ) -> Result<(), String> {
        let recognizer = VoskRecognizer::new(model_dir)?;
        let mut detector = WakeDetector::new(recognizer);
        let (tx, rx) = mpsc::sync_channel::<Vec<i16>>(8);
        let stream_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let stream = build_input_stream(tx, stream_error.clone())?;
        {
            use cpal::traits::StreamTrait;
            stream
                .play()
                .map_err(|e| format!("impossibile avviare il microfono: {e}"))?;
        }
        loop {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            if let Some(e) = stream_error
                .lock()
                .map_err(|_| "lock microfono".to_string())?
                .take()
            {
                on_event(WakeEvent::Error(e));
                break;
            }
            detector.set_paused(paused.load(Ordering::Relaxed));
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(chunk) => {
                    if let Some(ev) = detector.feed(&chunk) {
                        on_event(ev);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        Ok(())
    }

    fn build_input_stream(
        tx: mpsc::SyncSender<Vec<i16>>,
        stream_error: Arc<Mutex<Option<String>>>,
    ) -> Result<cpal::Stream, String> {
        use cpal::traits::{DeviceTrait, HostTrait};
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| "nessun microfono trovato".to_string())?;
        // Preferred: exactly what Vosk wants (16 kHz, mono, i16).
        // PipeWire/ALSA resample in the graph, so this usually works.
        let preferred = cpal::StreamConfig {
            channels: 1,
            sample_rate: cpal::SampleRate(SAMPLE_RATE),
            buffer_size: cpal::BufferSize::Default,
        };
        let err1 = stream_error.clone();
        let tx1 = tx.clone();
        match device.build_input_stream(
            &preferred,
            move |data: &[i16], _: &cpal::InputCallbackInfo| {
                let _ = tx1.try_send(data.to_vec());
            },
            move |err| {
                if let Ok(mut slot) = err1.lock() {
                    *slot = Some(format!("errore microfono: {err}"));
                }
            },
            None,
        ) {
            Ok(stream) => Ok(stream),
            Err(_) => {
                // Fallback: the device's default config, normalized to
                // 16 kHz mono in software.
                let cfg = device
                    .default_input_config()
                    .map_err(|e| format!("config microfono non disponibile: {e}"))?;
                build_with_default_config(&device, &cfg, tx, stream_error)
            }
        }
    }

    fn build_with_default_config(
        device: &cpal::Device,
        cfg: &cpal::SupportedStreamConfig,
        tx: mpsc::SyncSender<Vec<i16>>,
        stream_error: Arc<Mutex<Option<String>>>,
    ) -> Result<cpal::Stream, String> {
        use cpal::traits::DeviceTrait;
        let config: cpal::StreamConfig = cfg.config();
        let channels = config.channels as usize;
        let rate = config.sample_rate.0;
        let err_cb = move |err| {
            if let Ok(mut slot) = stream_error.lock() {
                *slot = Some(format!("errore microfono: {err}"));
            }
        };
        match cfg.sample_format() {
            cpal::SampleFormat::I16 => {
                let tx = tx.clone();
                device
                    .build_input_stream(
                        &config,
                        move |data: &[i16], _: &cpal::InputCallbackInfo| {
                            let _ = tx.try_send(normalize_i16(data, channels, rate));
                        },
                        err_cb,
                        None,
                    )
                    .map_err(|e| format!("impossibile aprire il microfono: {e}"))
            }
            cpal::SampleFormat::F32 => {
                let tx = tx.clone();
                device
                    .build_input_stream(
                        &config,
                        move |data: &[f32], _: &cpal::InputCallbackInfo| {
                            let _ = tx.try_send(normalize_f32(data, channels, rate));
                        },
                        err_cb,
                        None,
                    )
                    .map_err(|e| format!("impossibile aprire il microfono: {e}"))
            }
            cpal::SampleFormat::U16 => {
                let tx = tx.clone();
                device
                    .build_input_stream(
                        &config,
                        move |data: &[u16], _: &cpal::InputCallbackInfo| {
                            let as_i16: Vec<i16> =
                                data.iter().map(|s| (*s as i32 - 32768) as i16).collect();
                            let _ = tx.try_send(normalize_i16(&as_i16, channels, rate));
                        },
                        err_cb,
                        None,
                    )
                    .map_err(|e| format!("impossibile aprire il microfono: {e}"))
            }
            other => Err(format!("formato microfono non supportato: {other:?}")),
        }
    }

    // Silence the unused warning when the feature is on but the
    // constant is only used in tests.
    #[allow(dead_code)]
    fn _use_command_max_samples() {
        let _ = COMMAND_MAX_SAMPLES;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockRecognizer {
        detect_after: usize,
        calls: usize,
    }

    impl MockRecognizer {
        fn new(detect_after: usize) -> Self {
            MockRecognizer {
                detect_after,
                calls: 0,
            }
        }
    }

    impl WakeRecognizer for MockRecognizer {
        fn accept(&mut self, _samples: &[i16]) -> bool {
            self.calls += 1;
            self.calls >= self.detect_after
        }
    }

    fn chunk(n: usize) -> Vec<i16> {
        vec![0; n]
    }

    #[test]
    fn grammar_is_only_the_wake_phrase() {
        assert_eq!(grammar_json(), "[\"ehi nova\"]");
        assert_eq!(WAKE_PHRASE, "ehi nova");
    }

    #[test]
    fn model_paths() {
        let base = PathBuf::from("/tmp/x/models");
        assert_eq!(
            model_dir_in(&base),
            PathBuf::from("/tmp/x/models/vosk-model-small-it-0.22")
        );
        assert!(!model_present_in(&PathBuf::from(
            "/tmp/does-not-exist-nova-ai"
        )));
    }

    #[test]
    fn detector_detects_then_captures_then_pauses() {
        let mut det = WakeDetector::new(MockRecognizer::new(3));
        assert_eq!(det.feed(&chunk(1024)), None);
        assert_eq!(det.feed(&chunk(1024)), None);
        assert_eq!(det.feed(&chunk(1024)), Some(WakeEvent::Detected));
        // Command window: 10 s = 160_000 samples.
        assert_eq!(det.feed(&chunk(100_000)), None);
        assert_eq!(det.feed(&chunk(59_999)), None);
        match det.feed(&chunk(1)) {
            Some(WakeEvent::CommandReady(samples)) => {
                assert_eq!(samples.len(), COMMAND_MAX_SAMPLES);
            }
            other => panic!("expected CommandReady, got {other:?}"),
        }
        // Auto-paused after the command: audio is discarded.
        assert_eq!(det.feed(&chunk(1024)), None);
        assert!(det.is_paused());
        // Resume: listening again, and the mock still fires.
        det.set_paused(false);
        assert!(!det.is_paused());
        assert_eq!(det.feed(&chunk(1024)), Some(WakeEvent::Detected));
    }

    #[test]
    fn detector_pause_discards_and_resumes_clean() {
        let mut det = WakeDetector::new(MockRecognizer::new(1));
        det.set_paused(true);
        assert_eq!(det.feed(&chunk(1024)), None);
        det.set_paused(false);
        assert_eq!(det.feed(&chunk(1024)), Some(WakeEvent::Detected));
    }

    #[test]
    fn wav_header_is_valid() {
        let wav = samples_to_wav(&[0, 1000, -1000, 32767]);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(wav.len(), 44 + 8);
        assert_eq!(i16::from_le_bytes([wav[44], wav[45]]), 0);
        assert_eq!(i16::from_le_bytes([wav[46], wav[47]]), 1000);
    }

    #[test]
    fn normalize_downmixes_and_resamples() {
        // Stereo 48 kHz -> mono 16 kHz: 3 s of input -> 1 s of output.
        let stereo: Vec<f32> = vec![0.5; 48_000 * 2];
        let out = normalize_f32(&stereo, 2, 48_000);
        assert_eq!(out.len(), 16_000);
        assert!(out.iter().all(|s| (*s - 16383).abs() < 2));
        // Same rate: passthrough.
        let mono = vec![0.0f32, 0.5, -0.5, 1.0];
        let out = normalize_f32(&mono, 1, 16_000);
        assert_eq!(out, vec![0, 16383, -16383, 32767]);
    }

    #[test]
    fn download_and_extract_from_mock_server() {
        use std::io::Cursor;
        use std::net::TcpListener;
        // Build a tiny zip with the expected layout.
        let mut zip_bytes = Vec::new();
        {
            let mut w = zip::ZipWriter::new(Cursor::new(&mut zip_bytes));
            let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();
            w.start_file("vosk-model-small-it-0.22/am/final.mdl", opts)
                .unwrap();
            w.write_all(b"fake-model").unwrap();
            w.start_file("vosk-model-small-it-0.22/conf/model.conf", opts)
                .unwrap();
            w.write_all(b"dummy").unwrap();
            w.finish().unwrap();
        }
        let zip_len = zip_bytes.len();
        // Minimal HTTP server on 127.0.0.1.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut req = [0u8; 1024];
            let _ = stream.read(&mut req);
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {zip_len}\r\nContent-Type: application/zip\r\nConnection: close\r\n\r\n"
            );
            stream.write_all(head.as_bytes()).unwrap();
            stream.write_all(&zip_bytes).unwrap();
        });
        let base = std::env::temp_dir().join(format!("nova-ai-wake-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let mut seen: Vec<(u64, u64)> = Vec::new();
        let dir = download_model_from(
            &format!("http://127.0.0.1:{port}/model.zip"),
            &base,
            |got, total| seen.push((got, total)),
        )
        .unwrap();
        assert_eq!(dir, base.join(MODEL_NAME));
        assert!(model_present_in(&dir));
        assert_eq!(
            fs::read_to_string(dir.join("am/final.mdl")).unwrap(),
            "fake-model"
        );
        assert!(!seen.is_empty());
        assert_eq!(seen.last(), Some(&(zip_len as u64, zip_len as u64)));
        server.join().unwrap();
        let _ = fs::remove_dir_all(&base);
    }

    /// Manual verification only (needs the real 47 MB model + network):
    /// downloads the model to a temp dir and loads it in Vosk.
    /// Run: cargo test --features wakeword -- --ignored download_real_model
    #[test]
    #[ignore]
    #[cfg(feature = "wakeword")]
    fn download_real_model() {
        let base = std::env::temp_dir().join(format!("nova-ai-wake-real-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let dir = download_model_from(MODEL_URL, &base, |_, _| {}).unwrap();
        assert!(model_present_in(&dir));
        assert!(dir.join("am/final.mdl").is_file());
        assert!(dir.join("conf/model.conf").is_file());
        let _ = fs::remove_dir_all(&base);
    }

    /// Manual verification only: loads the real model and feeds 1 s of
    /// silence to the recognizer (no crash, no false detection).
    /// Needs NOVA_AI_MODEL_DIR pointing at an extracted model, or the
    /// model in the default location.
    /// Run: cargo test --features wakeword -- --ignored vosk_silence
    #[test]
    #[ignore]
    #[cfg(feature = "wakeword")]
    fn vosk_silence_no_false_detection() {
        let dir = model_dir();
        assert!(
            model_present_in(&dir),
            "model missing in {}; run the download_real_model test first or set NOVA_AI_MODELS_DIR",
            dir.display()
        );
        let mut rec = listener::VoskRecognizer::new(&dir).unwrap();
        let silence = vec![0i16; 16_000];
        assert!(!rec.accept(&silence));
        // The detector on top of it also stays quiet.
        let mut det = WakeDetector::new(rec);
        assert_eq!(det.feed(&silence), None);
        assert!(!det.is_paused());
    }
}
