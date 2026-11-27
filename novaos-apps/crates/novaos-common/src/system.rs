//! Esecuzione di comandi, con o senza privilegi (pkexec), con output
//! catturato oppure in streaming riga per riga (per i log in UI).
//!
//! Nessuna shell viene mai invocata: gli argomenti sono passati come
//! argv, quindi non c'è quoting da sbagliare e niente injection da
//! stringhe costruite a runtime.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

/// Output catturato di un comando terminato.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmdOutput {
    /// Exit code (`None` se il processo è stato ucciso da un segnale).
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl CmdOutput {
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }

    /// stdout + stderr concatenati (stderr solo se non vuoto).
    pub fn combined(&self) -> String {
        if self.stderr.trim().is_empty() {
            self.stdout.clone()
        } else if self.stdout.trim().is_empty() {
            self.stderr.clone()
        } else {
            format!("{}{}", self.stdout, self.stderr)
        }
    }
}

/// `true` se `name` è un eseguibile risolvibile da PATH.
pub fn command_exists(name: &str) -> bool {
    if name.contains('/') {
        return std::path::Path::new(name).is_file();
    }
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(name);
            if let Ok(meta) = std::fs::metadata(&candidate)
                && meta.is_file()
                && meta.permissions().mode() & 0o111 != 0
            {
                return true;
            }
        }
        false
    }
    #[cfg(not(unix))]
    {
        std::env::split_paths(&path).any(|dir| dir.join(name).is_file())
    }
}

fn run_as(argv: &[&str], privileged: bool) -> Result<CmdOutput, String> {
    if argv.is_empty() {
        return Err("comando vuoto".to_string());
    }
    let mut cmd = if privileged {
        let mut c = Command::new("pkexec");
        c.args(argv);
        c
    } else {
        let mut c = Command::new(argv[0]);
        c.args(&argv[1..]);
        c
    };
    let out = cmd
        .output()
        .map_err(|e| format!("avvio di `{}` fallito: {e}", argv[0]))?;
    Ok(CmdOutput {
        code: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

/// Esegue `argv` come utente corrente, output catturato.
pub fn run(argv: &[&str]) -> Result<CmdOutput, String> {
    run_as(argv, false)
}

/// Esegue `argv` come root via pkexec (chiede la password con l'agente
/// polkit del DE). Senza agente polkit attivo pkexec fallisce subito:
/// il chiamante deve mostrare l'errore e suggerire il terminale.
pub fn run_privileged(argv: &[&str]) -> Result<CmdOutput, String> {
    run_as(argv, true)
}

/// Esegue `argv` in streaming: ogni riga di stdout e stderr viene
/// passata a `on_line` man mano che arriva (per i log in UI).
/// Ritorna l'exit code (`None` se ucciso da un segnale).
/// `privileged = true` antepone pkexec (stesso discorso di
/// [`run_privileged`] sull'agente polkit).
/// `on_line` deve essere `Sync`: viene chiamata da due thread (uno per
/// pipe); in pratica la callback inoltra le righe su un canale.
pub fn run_streaming(
    argv: &[&str],
    privileged: bool,
    on_line: &(dyn Fn(&str) + Sync),
) -> Result<Option<i32>, String> {
    if argv.is_empty() {
        return Err("comando vuoto".to_string());
    }
    let mut cmd = if privileged {
        let mut c = Command::new("pkexec");
        c.args(argv);
        c
    } else {
        let mut c = Command::new(argv[0]);
        c.args(&argv[1..]);
        c
    };
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("avvio di `{}` fallito: {e}", argv[0]))?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    // I due thread leggono i pipe in parallelo; senza di essi un pipe
    // pieno bloccherebbe il figlio. std::thread::scope li riunisce.
    std::thread::scope(|s| {
        let mut handles = Vec::new();
        if let Some(stream) = stdout {
            handles.push(s.spawn(move || {
                let reader = BufReader::new(stream);
                for line in reader.lines().map_while(Result::ok) {
                    on_line(&line);
                }
            }));
        }
        if let Some(stream) = stderr {
            handles.push(s.spawn(move || {
                let reader = BufReader::new(stream);
                for line in reader.lines().map_while(Result::ok) {
                    on_line(&line);
                }
            }));
        }
        for h in handles {
            let _ = h.join();
        }
    });
    let status = child
        .wait()
        .map_err(|e| format!("attesa di `{}` fallita: {e}", argv[0]))?;
    Ok(status.code())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_echo_ok() {
        let out = run(&["echo", "ciao"]).unwrap();
        assert!(out.success());
        assert_eq!(out.stdout.trim(), "ciao");
    }

    #[test]
    fn run_false_code() {
        let out = run(&["false"]).unwrap();
        assert!(!out.success());
        assert_eq!(out.code, Some(1));
    }

    #[test]
    fn run_missing_binary_errors() {
        assert!(run(&["novaos-comando-che-non-esiste"]).is_err());
    }

    #[test]
    fn run_empty_errors() {
        assert!(run(&[]).is_err());
    }

    #[test]
    fn streaming_collects_lines() {
        let lines = std::sync::Mutex::new(Vec::<String>::new());
        let code = run_streaming(
            &["sh", "-c", "echo uno; echo due >&2; echo tre"],
            false,
            &|l| lines.lock().unwrap().push(l.to_string()),
        )
        .unwrap();
        assert_eq!(code, Some(0));
        let lines = lines.into_inner().unwrap();
        assert_eq!(lines.len(), 3);
        assert!(lines.contains(&"uno".to_string()));
        assert!(lines.contains(&"due".to_string()));
        assert!(lines.contains(&"tre".to_string()));
    }

    #[test]
    fn command_exists_sh() {
        assert!(command_exists("sh"));
        assert!(!command_exists("novaos-comando-che-non-esiste"));
    }
}
