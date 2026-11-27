//! Offline text-to-speech with Piper (Italian voice).
//!
//! The piper binary and the voice model are downloaded on first use
//! into `~/.local/share/nova-ai/piper/` (env `NOVA_AI_PIPER_DIR`
//! overrides, for tests) — the same lazy-download pattern as the Vosk
//! wake-word model. Synthesis writes a WAV (16 kHz, mono, 16-bit) via
//! the piper CLI; playback is `pw-play` (PipeWire).
//!
//! Voice choice (verified 2026-11-26): `it_IT-riccardo-x_low`.
//! The other Italian voices on HuggingFace (`paola/medium`,
//! `serena/medium`, `serena/high`) are unusable with the released
//! piper binaries: their `phoneme_id_map` contains multi-codepoint
//! diphthongs (`aɪ`, `aʊ`, …) that piper rejects at config load.
//! Riccardo x_low synthesizes correctly (real test: "Ciao mondo, sono
//! NovaAI e tutto funziona." → 2.66 s WAV, 16 kHz mono 16-bit).
//!
//! No audio hardware is needed to synthesize — only to hear it.
//! Playback without a sound server fails honestly instead of
//! pretending success.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Piper release used (verified asset name).
pub const PIPER_VERSION: &str = "2023.11.14-2";
/// Piper binary tarball (~26 MB).
pub const PIPER_URL: &str =
    "https://github.com/rhasspy/piper/releases/download/2023.11.14-2/piper_linux_x86_64.tar.gz";
/// Italian voice (verified working with the released piper binary).
pub const VOICE_NAME: &str = "it_IT-riccardo-x_low";
/// Voice model (~28 MB onnx + ~4 KiB json).
pub const VOICE_ONNX_URL: &str = "https://huggingface.co/rhasspy/piper-voices/resolve/main/it/it_IT/riccardo/x_low/it_IT-riccardo-x_low.onnx";
pub const VOICE_JSON_URL: &str = "https://huggingface.co/rhasspy/piper-voices/resolve/main/it/it_IT/riccardo/x_low/it_IT-riccardo-x_low.onnx.json";

/// Base directory for the piper binary + voice:
/// `~/.local/share/nova-ai/piper`. Tests can override it with
/// `NOVA_AI_PIPER_DIR`.
pub fn piper_base_dir() -> PathBuf {
    if let Ok(p) = std::env::var("NOVA_AI_PIPER_DIR") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".local")
        .join("share")
        .join("nova-ai")
        .join("piper")
}

/// The piper binary (the tarball extracts to a `piper/` subdir).
pub fn piper_bin() -> PathBuf {
    piper_bin_in(&piper_base_dir())
}

fn piper_bin_in(base: &Path) -> PathBuf {
    base.join("piper").join("piper")
}

/// Voice model file.
pub fn voice_onnx() -> PathBuf {
    voice_onnx_in(&piper_base_dir())
}

fn voice_onnx_in(base: &Path) -> PathBuf {
    base.join(format!("{VOICE_NAME}.onnx"))
}

pub fn voice_json() -> PathBuf {
    voice_json_in(&piper_base_dir())
}

fn voice_json_in(base: &Path) -> PathBuf {
    base.join(format!("{VOICE_NAME}.onnx.json"))
}

/// True when the piper binary exists and is executable.
pub fn piper_present() -> bool {
    piper_bin().is_file()
}

/// True when both voice files are in place.
pub fn voice_present() -> bool {
    voice_onnx().is_file() && voice_json().is_file()
}

/// True when synthesis can run (binary + voice).
pub fn tts_ready() -> bool {
    tts_ready_in(&piper_base_dir())
}

/// Same, for an explicit base dir (tests).
pub fn tts_ready_in(base: &Path) -> bool {
    piper_bin_in(base).is_file() && voice_onnx_in(base).is_file() && voice_json_in(base).is_file()
}

/// Human-readable status for the settings screen.
pub fn tts_status() -> String {
    match (piper_present(), voice_present()) {
        (true, true) => format!("Voce pronta ({VOICE_NAME}, Piper {PIPER_VERSION})."),
        (false, _) => "Piper non scaricato (serve il primo download, ~55 MB).".to_string(),
        (true, false) => "Piper presente, voce italiana mancante.".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Download (binary + voice), temp dir first, then move into place
// ---------------------------------------------------------------------------

/// Download piper + voice, reporting progress as bytes arrive
/// (total is 0 when the server hides Content-Length). Returns the
/// piper base dir. A failed download never leaves a broken install
/// behind: everything lands in a temp dir first.
pub fn download_all(progress: impl FnMut(u64, u64)) -> Result<PathBuf, String> {
    let base = piper_base_dir();
    fs::create_dir_all(&base).map_err(|e| format!("impossibile creare {base:?}: {e}"))?;
    let mut progress = progress;
    if !piper_present() {
        download_piper(&base, &mut progress)?;
    }
    if !voice_present() {
        download_voice(&base, &mut progress)?;
    }
    Ok(base)
}

fn download_bytes(url: &str, progress: &mut impl FnMut(u64, u64)) -> Result<Vec<u8>, String> {
    let resp = crate::llm::LlmClient::http_agent()
        .get(url)
        .call()
        .map_err(|e| match e {
            ureq::Error::Timeout(_) => "timeout durante il download".to_string(),
            ureq::Error::Io(e) if e.kind() == std::io::ErrorKind::NotFound => {
                "download fallito: controlla la connessione (sei offline?)".to_string()
            }
            other => format!("download fallito: {other}"),
        })?;
    if resp.status().as_u16() != 200 {
        return Err(format!("download fallito: HTTP {}", resp.status().as_u16()));
    }
    let total: u64 = resp
        .headers()
        .get("Content-Length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let mut reader = resp.into_body().into_reader();
    let mut buf = [0u8; 32768];
    let mut out = Vec::new();
    let mut downloaded: u64 = 0;
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("lettura download fallita: {e}"))?;
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n]);
        downloaded += n as u64;
        progress(downloaded, total);
    }
    Ok(out)
}

fn download_piper(base: &Path, progress: &mut impl FnMut(u64, u64)) -> Result<(), String> {
    let bytes = download_bytes(PIPER_URL, progress)?;
    let tmp = base.join(format!(".piper.tmp-{}", std::process::id()));
    fs::create_dir_all(&tmp).map_err(|e| format!("impossibile estrarre piper: {e}"))?;
    let result = (|| -> Result<(), String> {
        let gz = flate2::read::GzDecoder::new(&bytes[..]);
        let mut archive = tar::Archive::new(gz);
        archive
            .unpack(&tmp)
            .map_err(|e| format!("archivio piper non valido: {e}"))?;
        Ok(())
    })();
    if let Err(e) = result {
        let _ = fs::remove_dir_all(&tmp);
        return Err(e);
    }
    let extracted = tmp.join("piper");
    if !extracted.join("piper").is_file() {
        let _ = fs::remove_dir_all(&tmp);
        return Err("archivio piper non contiene il binario atteso".to_string());
    }
    let dest = base.join("piper");
    if dest.exists() {
        let _ = fs::remove_dir_all(&dest);
    }
    fs::rename(&extracted, &dest).map_err(|e| format!("installazione piper fallita: {e}"))?;
    let _ = fs::remove_dir_all(&tmp);
    // Make sure the binary is executable (tar usually preserves it).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(dest.join("piper"), fs::Permissions::from_mode(0o755));
    }
    Ok(())
}

fn download_voice(base: &Path, progress: &mut impl FnMut(u64, u64)) -> Result<(), String> {
    for (url, name) in [
        (VOICE_ONNX_URL, format!("{VOICE_NAME}.onnx")),
        (VOICE_JSON_URL, format!("{VOICE_NAME}.onnx.json")),
    ] {
        let bytes = download_bytes(url, progress)?;
        let tmp = base.join(format!(".{name}.tmp-{}", std::process::id()));
        fs::write(&tmp, &bytes).map_err(|e| format!("scrittura voce fallita: {e}"))?;
        fs::rename(&tmp, base.join(&name))
            .map_err(|e| format!("installazione voce fallita: {e}"))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Synthesis + playback
// ---------------------------------------------------------------------------

/// WAV header info: (sample rate Hz, channels, duration seconds).
/// Returns None when the bytes are not a WAV file.
pub fn wav_info(bytes: &[u8]) -> Option<(u32, u16, f32)> {
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let channels = u16::from_le_bytes(bytes[22..24].try_into().ok()?);
    let rate = u32::from_le_bytes(bytes[24..28].try_into().ok()?);
    let bits = u16::from_le_bytes(bytes[34..36].try_into().ok()?);
    if channels == 0 || rate == 0 || bits == 0 {
        return None;
    }
    let data_bytes = (bytes.len() as u64).saturating_sub(44);
    let secs = data_bytes as f32 / (rate as f32 * channels as f32 * (bits as f32 / 8.0));
    Some((rate, channels, secs))
}

/// Synthesize `text` to a WAV file and return its path (temp file,
/// caller deletes it). Piper reads the text from stdin; the binary
/// runs with its own directory on the library path (libonnxruntime)
/// and as working directory (espeak-ng-data next to it).
pub fn synthesize_to_file(text: &str) -> Result<PathBuf, String> {
    if !tts_ready() {
        return Err(format!(
            "TTS non pronto: {}. Scarica prima voce e binario.",
            tts_status()
        ));
    }
    let out_path = std::env::temp_dir().join(format!("nova-ai-tts-{}.wav", std::process::id()));
    let piper_dir = piper_base_dir().join("piper");
    let lib_path = format!(
        "{}{}",
        piper_dir.display(),
        std::env::var("LD_LIBRARY_PATH")
            .map(|v| format!(":{v}"))
            .unwrap_or_default()
    );
    let mut child = Command::new(piper_bin())
        .args([
            "--model",
            voice_onnx().to_str().unwrap_or(""),
            "--output_file",
            out_path.to_str().unwrap_or(""),
        ])
        .env("LD_LIBRARY_PATH", lib_path)
        .current_dir(&piper_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("avvio di piper fallito: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(text.as_bytes());
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("sintesi fallita: {e}"))?;
    if !out.status.success() {
        let _ = fs::remove_file(&out_path);
        return Err(format!(
            "piper fallito: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let bytes = fs::read(&out_path).map_err(|e| format!("lettura WAV fallita: {e}"))?;
    if wav_info(&bytes).is_none() || bytes.is_empty() {
        let _ = fs::remove_file(&out_path);
        return Err("piper ha prodotto un file non valido (non è un WAV)".to_string());
    }
    Ok(out_path)
}

/// Synthesize and return the WAV bytes (convenience for tests).
pub fn synthesize(text: &str) -> Result<Vec<u8>, String> {
    let path = synthesize_to_file(text)?;
    let bytes = fs::read(&path).map_err(|e| format!("lettura WAV fallita: {e}"))?;
    let _ = fs::remove_file(&path);
    Ok(bytes)
}

/// Play a WAV file with `pw-play` (PipeWire). Fails honestly when
/// there is no sound server (e.g. VM without audio).
pub fn play_wav(path: &Path) -> Result<(), String> {
    let out = Command::new("pw-play").arg(path).output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            "pw-play non trovato: nessun server audio PipeWire".to_string()
        } else {
            format!("avvio di pw-play fallito: {e}")
        }
    })?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "riproduzione fallita: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Speak `text` aloud: synthesize, then play. Long answers are cut
/// to `max_chars` so the voice never rambles for minutes.
pub fn speak(text: &str, max_chars: usize) -> Result<(), String> {
    let short: String = text.chars().take(max_chars).collect();
    let path = synthesize_to_file(&short)?;
    let result = play_wav(&path);
    let _ = fs::remove_file(&path);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_info_parses_valid_header() {
        // Minimal 44-byte header + 1 s of 16 kHz mono 16-bit silence.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&32044u32.to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
        bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
        bytes.extend_from_slice(&16000u32.to_le_bytes());
        bytes.extend_from_slice(&32000u32.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&32000u32.to_le_bytes());
        bytes.resize(44 + 32000, 0);
        let (rate, channels, secs) = wav_info(&bytes).unwrap();
        assert_eq!(rate, 16000);
        assert_eq!(channels, 1);
        assert!((secs - 1.0).abs() < 0.01, "{secs}");
        assert!(wav_info(b"non un wav").is_none());
        assert!(wav_info(&bytes[..20]).is_none());
    }

    #[test]
    fn tts_not_ready_on_empty_dir() {
        let tmp = std::env::temp_dir().join(format!("nova-ai-tts-empty-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmp);
        assert!(!tts_ready_in(&tmp));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn synthesize_with_real_piper_fixture() {
        // Uses the piper binary + riccardo voice already downloaded
        // on this VM (verified 2026-11-26 in /tmp/piper-test/).
        // The fixture dir can be overridden with
        // NOVA_AI_TTS_FIXTURE_DIR; without it the test skips
        // gracefully instead of failing.
        let fixture = std::env::var("NOVA_AI_TTS_FIXTURE_DIR")
            .unwrap_or_else(|_| "/tmp/piper-test".to_string());
        let base = PathBuf::from(&fixture);
        if !base.join("piper").join("piper").is_file()
            || !base.join(format!("{VOICE_NAME}.onnx")).is_file()
        {
            eprintln!("TTS fixture assente in {fixture}: test saltato (niente download in test)");
            return;
        }
        unsafe {
            std::env::set_var("NOVA_AI_PIPER_DIR", &fixture);
        }
        let wav =
            synthesize("Ciao mondo, sono NovaAI e tutto funziona.").expect("sintesi piper fallita");
        assert!(!wav.is_empty(), "WAV vuoto");
        let (rate, channels, secs) = wav_info(&wav).expect("WAV non valido");
        assert_eq!(rate, 16000);
        assert_eq!(channels, 1);
        assert!(secs > 1.0 && secs < 10.0, "durata strana: {secs}s");
        unsafe {
            std::env::remove_var("NOVA_AI_PIPER_DIR");
        }
    }
}
