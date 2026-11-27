//! Passi di installazione dei tre gruppi di NovaOS Setup.
//!
//! Ogni gruppo è una lista di passi; ogni passo è un argv eseguito
//! senza shell (mai con quoting: gli argomenti sono passati come
//! vettore, quindi niente injection da stringhe costruite a runtime).
//! L'unica eccezione è rustup: l'installer ufficiale è uno script
//! scaricato via pipe (`curl … | sh`), quindi quel passo è
//! `sh -c "<comando fisso>"` — il comando è una costante, nessun
//! input dell'utente viene mai interpolato.
//!
//! Regole verificate il 26 nov 2026 (non indovinare oltre):
//! - Codec: bastano i repo RPM Fusion **free**. `ffmpeg` confligge
//!   con `ffmpeg-free`: su rpm-ostree la via documentata è
//!   `rpm-ostree override remove ffmpeg-free --install ffmpeg`.
//!   TODO: verificare su Fedora reale se `ffmpeg-free` sia davvero
//!   installato nell'immagine (qui si controlla con `rpm -q`, e se
//!   l'installazione normale fallisce citando `ffmpeg-free` si
//!   riprova con l'override).
//! - Gaming: Steam è il Flatpak `com.valvesoftware.Steam` (Flathub);
//!   MangoHUD/GameMode sono pacchetti Fedora (anche `.i686`).
//! - Dev: rustup ufficiale come **utente** (mai con pkexec); VS Code
//!   è il Flatpak `com.visualstudio.code` (Flathub); `code-oss` NON
//!   è nei repo ufficiali F45 (solo COPR) e non viene usato;
//!   `git` è nei repo Fedora.
//! - I pacchetti layered rpm-ostree richiedono riavvio; Flatpak e
//!   rustup no.
//! - pkexec senza agente polkit attivo fallisce subito: il chiamante
//!   deve mostrare l'errore e suggerire il terminale.

use std::path::PathBuf;

use novaos_common::system::{CmdOutput, run, run_streaming};

/// URL del pacchetto release di RPM Fusion free per Fedora 45
/// (pacchetto `rpmfusion-free-release-45-2.noarch` esistente).
pub const RPMFUSION_FREE_RELEASE_URL: &str =
    "https://download1.rpmfusion.org/free/fedora/rpmfusion-free-release-45.noarch.rpm";

/// Pacchetti codec da installare (tutti nel repo free).
/// Nota: `gstreamer1-plugin-libav` al SINGOLARE, non `gstreamer1-libav`.
pub const CODEC_PACKAGES: &[&str] = &[
    "ffmpeg",
    "gstreamer1-plugins-bad-freeworld",
    "gstreamer1-plugins-ugly",
    "gstreamer1-plugin-libav",
];

/// Steam su Flathub (verificato, v1.0.0.87).
pub const STEAM_APP_ID: &str = "com.valvesoftware.Steam";
/// VS Code su Flathub (verificato, v1.145.3).
pub const VSCODE_APP_ID: &str = "com.visualstudio.code";

/// Comando ufficiale di installazione di rustup (eseguito come utente).
pub const RUSTUP_INSTALL_SH: &str = "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable";

/// Ultime righe di output tenute nella coda d'errore di un gruppo.
const TAIL_LINES: usize = 30;

/// I tre gruppi installabili.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GroupId {
    Codec,
    Gaming,
    Dev,
}

impl GroupId {
    /// Tutti i gruppi, nell'ordine in cui appaiono nella UI.
    pub fn all() -> [GroupId; 3] {
        [GroupId::Codec, GroupId::Gaming, GroupId::Dev]
    }

    /// Nome del flag persistente (`setup/<flag>`, vedi `novaos-common`
    /// flags; app `"setup"`).
    pub fn flag(&self) -> &'static str {
        match self {
            GroupId::Codec => "codec",
            GroupId::Gaming => "gaming",
            GroupId::Dev => "dev",
        }
    }

    /// Titolo mostrato nella scheda.
    pub fn title(&self) -> &'static str {
        match self {
            GroupId::Codec => "Codec multimediali",
            GroupId::Gaming => "Gaming",
            GroupId::Dev => "Strumenti per sviluppare",
        }
    }

    /// Descrizione mostrata nella scheda.
    pub fn description(&self) -> &'static str {
        match self {
            GroupId::Codec => {
                "Video e audio in tutti i formati: abilita RPM Fusion (free) \
                e installa ffmpeg + codec GStreamer. Richiede riavvio."
            }
            GroupId::Gaming => {
                "Steam + MangoHUD + GameMode. Steam è Flatpak (pronto subito), \
                MangoHUD/GameMode sono pacchetti di sistema e richiedono riavvio."
            }
            GroupId::Dev => {
                "Strumenti per sviluppare: rustup (Rust), Visual Studio Code, git. \
                Solo git richiede riavvio."
            }
        }
    }
}

/// Stato di un gruppo, mostrato nella scheda.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupState {
    NotInstalled,
    Installed,
    Installing,
    Error,
}

impl GroupState {
    /// Etichetta italiana di stato.
    pub fn label(&self) -> &'static str {
        match self {
            GroupState::NotInstalled => "Non installato",
            GroupState::Installed => "Installato",
            GroupState::Installing => "Installazione…",
            GroupState::Error => "Errore",
        }
    }
}

/// Un singolo passo: argv + se serve pkexec + se richiede riavvio.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepSpec {
    /// Riga di log mostrata prima dell'esecuzione.
    pub label: &'static str,
    pub argv: Vec<String>,
    /// `true` = eseguito come root via pkexec.
    pub privileged: bool,
    /// `true` = pacchetto layered rpm-ostree (serve riavvio).
    pub needs_reboot: bool,
}

/// Esito dell'installazione di un gruppo.
#[derive(Debug, Clone)]
pub struct GroupOutcome {
    /// `true` se il gruppo è installato (anche se era già tutto pronto).
    pub success: bool,
    /// `true` se non c'era niente da fare (tutto già installato).
    pub skipped: bool,
    /// `true` se almeno un passo eseguito richiede il riavvio.
    pub needs_reboot: bool,
    /// In caso di fallimento: messaggio + coda dell'output.
    pub error: Option<String>,
}

// --- costruzione degli argv (funzioni pure, testate) ---

/// `rpm-ostree install <rpmfusion-free-release-45.noarch.rpm>`.
pub fn codec_repo_argv() -> Vec<String> {
    vec![
        "rpm-ostree".to_string(),
        "install".to_string(),
        RPMFUSION_FREE_RELEASE_URL.to_string(),
    ]
}

/// Installazione dei codec. Con `ffmpeg_free_installed = true` usa
/// l'override (`rpm-ostree override remove ffmpeg-free --install …`),
/// altrimenti l'installazione normale.
pub fn codec_install_argv(ffmpeg_free_installed: bool) -> Vec<String> {
    if ffmpeg_free_installed {
        let mut argv = vec![
            "rpm-ostree".to_string(),
            "override".to_string(),
            "remove".to_string(),
            "ffmpeg-free".to_string(),
        ];
        for pkg in CODEC_PACKAGES {
            argv.push("--install".to_string());
            argv.push((*pkg).to_string());
        }
        argv
    } else {
        let mut argv = vec!["rpm-ostree".to_string(), "install".to_string()];
        argv.extend(CODEC_PACKAGES.iter().map(|p| (*p).to_string()));
        argv
    }
}

/// `flatpak install --system -y flathub com.valvesoftware.Steam`.
pub fn gaming_steam_argv() -> Vec<String> {
    vec![
        "flatpak".to_string(),
        "install".to_string(),
        "--system".to_string(),
        "-y".to_string(),
        "flathub".to_string(),
        STEAM_APP_ID.to_string(),
    ]
}

/// `rpm-ostree install mangohud mangohud.i686 gamemode gamemode.i686`.
pub fn gaming_system_argv() -> Vec<String> {
    vec![
        "rpm-ostree".to_string(),
        "install".to_string(),
        "mangohud".to_string(),
        "mangohud.i686".to_string(),
        "gamemode".to_string(),
        "gamemode.i686".to_string(),
    ]
}

/// Installazione di rustup come utente: `sh -c "<comando fisso>"`.
/// Il comando è la costante [`RUSTUP_INSTALL_SH`]: nessun input
/// utente viene interpolato.
pub fn dev_rustup_argv() -> Vec<String> {
    vec![
        "sh".to_string(),
        "-c".to_string(),
        RUSTUP_INSTALL_SH.to_string(),
    ]
}

/// `flatpak install --system -y flathub com.visualstudio.code`.
pub fn dev_vscode_argv() -> Vec<String> {
    vec![
        "flatpak".to_string(),
        "install".to_string(),
        "--system".to_string(),
        "-y".to_string(),
        "flathub".to_string(),
        VSCODE_APP_ID.to_string(),
    ]
}

/// `rpm-ostree install git`.
pub fn dev_git_argv() -> Vec<String> {
    vec![
        "rpm-ostree".to_string(),
        "install".to_string(),
        "git".to_string(),
    ]
}

// --- helper puri ---

/// `true` se `rpm -q <pacchetti…>` è riuscito (codice 0 = tutti
/// i pacchetti richiesti sono installati).
pub fn rpm_packages_installed(out: &CmdOutput) -> bool {
    out.success()
}

/// `true` se l'output di
/// `flatpak list --system --app --columns=application` contiene
/// esattamente una riga uguale ad `app_id` (confronto per riga intera:
/// `com.valvesoftware.Steam` non deve matchare `…SteamLink`).
pub fn flatpak_app_installed(list_stdout: &str, app_id: &str) -> bool {
    list_stdout.lines().any(|line| line.trim() == app_id)
}

/// `true` se l'output (stdout+stderr) cita `ffmpeg-free`: è il segnale
/// che l'installazione normale dei codec è fallita per il conflitto
/// con `ffmpeg-free` e vale la pena riprovare con l'override.
pub fn codec_needs_override_retry(combined_output: &str) -> bool {
    combined_output.contains("ffmpeg-free")
}

/// Percorso dell'eseguibile rustup dell'utente (`~/.cargo/bin/rustup`).
pub fn rustup_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("~"));
    home.join(".cargo/bin/rustup")
}

// --- runner (astrae l'esecuzione: reale in produzione, finto nei test) ---

/// Esecuzione di comandi. L'implementazione reale delega a
/// `novaos_common::system`; i test usano un finto.
pub trait Runner {
    /// Esegue `argv` come utente corrente, output catturato.
    fn run(&self, argv: &[&str]) -> Result<CmdOutput, String>;
    /// Esegue `argv` in streaming (ogni riga a `on_line`).
    /// `privileged = true` antepone pkexec.
    fn run_streaming(
        &self,
        argv: &[&str],
        privileged: bool,
        on_line: &(dyn Fn(&str) + Sync),
    ) -> Result<Option<i32>, String>;
    /// `true` se rustup è già installato per l'utente.
    fn rustup_installed(&self) -> bool;
}

/// Runner reale: comandi veri del sistema.
pub struct RealRunner;

impl Runner for RealRunner {
    fn run(&self, argv: &[&str]) -> Result<CmdOutput, String> {
        run(argv)
    }

    fn run_streaming(
        &self,
        argv: &[&str],
        privileged: bool,
        on_line: &(dyn Fn(&str) + Sync),
    ) -> Result<Option<i32>, String> {
        run_streaming(argv, privileged, on_line)
    }

    fn rustup_installed(&self) -> bool {
        rustup_path().is_file()
    }
}

// --- skip-check e esecuzione ---

/// `true` se il gruppo risulta già completamente installato
/// (tutti gli skip-check passano). Usato all'avvio per lo stato
/// iniziale delle schede.
pub fn check_installed(group: GroupId, runner: &dyn Runner) -> bool {
    match group {
        GroupId::Codec => {
            let repo_ok = runner
                .run(&["rpm", "-q", "rpmfusion-free-release"])
                .map(|o| rpm_packages_installed(&o))
                .unwrap_or(false);
            let mut argv: Vec<&str> = vec!["rpm", "-q"];
            argv.extend(CODEC_PACKAGES.iter().copied());
            let pkgs_ok = runner
                .run(&argv)
                .map(|o| rpm_packages_installed(&o))
                .unwrap_or(false);
            repo_ok && pkgs_ok
        }
        GroupId::Gaming => {
            let steam_ok = runner
                .run(&[
                    "flatpak",
                    "list",
                    "--system",
                    "--app",
                    "--columns=application",
                ])
                .map(|o| flatpak_app_installed(&o.stdout, STEAM_APP_ID))
                .unwrap_or(false);
            let sys_ok = runner
                .run(&["rpm", "-q", "mangohud", "gamemode"])
                .map(|o| rpm_packages_installed(&o))
                .unwrap_or(false);
            steam_ok && sys_ok
        }
        GroupId::Dev => {
            let vscode_ok = runner
                .run(&[
                    "flatpak",
                    "list",
                    "--system",
                    "--app",
                    "--columns=application",
                ])
                .map(|o| flatpak_app_installed(&o.stdout, VSCODE_APP_ID))
                .unwrap_or(false);
            let git_ok = runner
                .run(&["rpm", "-q", "git"])
                .map(|o| rpm_packages_installed(&o))
                .unwrap_or(false);
            runner.rustup_installed() && vscode_ok && git_ok
        }
    }
}

/// Esegue un passo in streaming: logga il comando, poi ogni riga di
/// output (a `emit` e nella coda `tail`, ultime [`TAIL_LINES`] righe).
/// `Err` se il comando non parte o esce con codice != 0.
/// `tail` è un `Mutex` perché `run_streaming` chiama la callback da
/// due thread (stdout e stderr) e la callback deve essere `Sync`.
fn run_step(
    runner: &dyn Runner,
    spec: &StepSpec,
    emit: &(dyn Fn(&str) + Sync),
    tail: &std::sync::Mutex<Vec<String>>,
) -> Result<(), String> {
    emit(&format!("$ {}", spec.argv.join(" ")));
    let argv_refs: Vec<&str> = spec.argv.iter().map(String::as_str).collect();
    let code = runner.run_streaming(&argv_refs, spec.privileged, &|line| {
        let mut guard = tail.lock().unwrap_or_else(|e| e.into_inner());
        if guard.len() >= TAIL_LINES {
            guard.remove(0);
        }
        guard.push(line.to_string());
        drop(guard);
        emit(line);
    })?;
    match code {
        Some(0) => Ok(()),
        other => Err(format!(
            "`{}` terminato con codice {:?}",
            spec.argv.join(" "),
            other
        )),
    }
}

/// Installa un gruppo: salta i passi già soddisfatti, esegue gli altri
/// in streaming. In caso di errore `GroupOutcome::error` contiene il
/// messaggio + la coda dell'output (sempre visibile in UI).
pub fn run_group(
    group: GroupId,
    runner: &dyn Runner,
    emit: &(dyn Fn(&str) + Sync),
) -> GroupOutcome {
    let mut outcome = GroupOutcome {
        success: false,
        skipped: false,
        needs_reboot: false,
        error: None,
    };
    let tail = std::sync::Mutex::new(Vec::<String>::new());
    let result = run_group_inner(group, runner, emit, &tail, &mut outcome);
    match result {
        Ok(()) => outcome.success = true,
        Err(mut msg) => {
            let tail = tail.into_inner().unwrap_or_default();
            if !tail.is_empty() {
                msg.push_str("\n--- ultimi output ---\n");
                msg.push_str(&tail.join("\n"));
            }
            outcome.error = Some(msg);
        }
    }
    outcome
}

fn run_group_inner(
    group: GroupId,
    runner: &dyn Runner,
    emit: &(dyn Fn(&str) + Sync),
    tail: &std::sync::Mutex<Vec<String>>,
    outcome: &mut GroupOutcome,
) -> Result<(), String> {
    let mut any_work = false;
    match group {
        GroupId::Codec => {
            // Passo 1: repository RPM Fusion free (skip se già presente).
            let repo_ok = runner
                .run(&["rpm", "-q", "rpmfusion-free-release"])
                .map(|o| rpm_packages_installed(&o))
                .unwrap_or(false);
            if !repo_ok {
                let spec = StepSpec {
                    label: "Abilita il repository RPM Fusion (free)",
                    argv: codec_repo_argv(),
                    privileged: true,
                    needs_reboot: true,
                };
                emit(spec.label);
                run_step(runner, &spec, emit, tail)?;
                outcome.needs_reboot = true;
                any_work = true;
            }
            // Passo 2: codec (skip se tutti già installati).
            let mut argv: Vec<&str> = vec!["rpm", "-q"];
            argv.extend(CODEC_PACKAGES.iter().copied());
            let pkgs_ok = runner
                .run(&argv)
                .map(|o| rpm_packages_installed(&o))
                .unwrap_or(false);
            if !pkgs_ok {
                // TODO: verificare su Fedora reale se `ffmpeg-free`
                // sia davvero installato nell'immagine.
                let ffmpeg_free = runner
                    .run(&["rpm", "-q", "ffmpeg-free"])
                    .map(|o| rpm_packages_installed(&o))
                    .unwrap_or(false);
                let spec = StepSpec {
                    label: "Installa ffmpeg e i codec GStreamer",
                    argv: codec_install_argv(ffmpeg_free),
                    privileged: true,
                    needs_reboot: true,
                };
                emit(spec.label);
                match run_step(runner, &spec, emit, tail) {
                    Ok(()) => {
                        outcome.needs_reboot = true;
                        any_work = true;
                    }
                    Err(e) => {
                        let combined = tail.lock().unwrap_or_else(|e| e.into_inner()).join("\n");
                        if !ffmpeg_free && codec_needs_override_retry(&combined) {
                            emit(
                                "Conflitto con ffmpeg-free rilevato: riprovo con \
                                 `rpm-ostree override remove ffmpeg-free`.",
                            );
                            let retry = StepSpec {
                                label: "Installa ffmpeg e i codec GStreamer \
                                    (override ffmpeg-free)",
                                argv: codec_install_argv(true),
                                privileged: true,
                                needs_reboot: true,
                            };
                            emit(retry.label);
                            run_step(runner, &retry, emit, tail)?;
                            outcome.needs_reboot = true;
                            any_work = true;
                        } else {
                            return Err(e);
                        }
                    }
                }
            }
        }
        GroupId::Gaming => {
            // Passo 1: Steam (Flatpak, skip se già presente).
            let steam_ok = runner
                .run(&[
                    "flatpak",
                    "list",
                    "--system",
                    "--app",
                    "--columns=application",
                ])
                .map(|o| flatpak_app_installed(&o.stdout, STEAM_APP_ID))
                .unwrap_or(false);
            if !steam_ok {
                let spec = StepSpec {
                    label: "Installa Steam (Flatpak)",
                    argv: gaming_steam_argv(),
                    privileged: true,
                    needs_reboot: false,
                };
                emit(spec.label);
                run_step(runner, &spec, emit, tail)?;
                any_work = true;
            }
            // Passo 2: MangoHUD + GameMode (skip se già installati).
            let sys_ok = runner
                .run(&["rpm", "-q", "mangohud", "gamemode"])
                .map(|o| rpm_packages_installed(&o))
                .unwrap_or(false);
            if !sys_ok {
                let spec = StepSpec {
                    label: "Installa MangoHUD e GameMode",
                    argv: gaming_system_argv(),
                    privileged: true,
                    needs_reboot: true,
                };
                emit(spec.label);
                run_step(runner, &spec, emit, tail)?;
                outcome.needs_reboot = true;
                any_work = true;
            }
        }
        GroupId::Dev => {
            // Passo 1: rustup come utente (skip se già presente).
            if !runner.rustup_installed() {
                let spec = StepSpec {
                    label: "Installa rustup (Rust)",
                    argv: dev_rustup_argv(),
                    privileged: false,
                    needs_reboot: false,
                };
                emit(spec.label);
                run_step(runner, &spec, emit, tail)?;
                emit(
                    "rustup installato: apri un nuovo terminale oppure esegui \
                     `source ~/.cargo/env`.",
                );
                any_work = true;
            }
            // Passo 2: VS Code (Flatpak, skip se già presente).
            let vscode_ok = runner
                .run(&[
                    "flatpak",
                    "list",
                    "--system",
                    "--app",
                    "--columns=application",
                ])
                .map(|o| flatpak_app_installed(&o.stdout, VSCODE_APP_ID))
                .unwrap_or(false);
            if !vscode_ok {
                let spec = StepSpec {
                    label: "Installa Visual Studio Code (Flatpak)",
                    argv: dev_vscode_argv(),
                    privileged: true,
                    needs_reboot: false,
                };
                emit(spec.label);
                run_step(runner, &spec, emit, tail)?;
                any_work = true;
            }
            // Passo 3: git (skip se già installato).
            let git_ok = runner
                .run(&["rpm", "-q", "git"])
                .map(|o| rpm_packages_installed(&o))
                .unwrap_or(false);
            if !git_ok {
                let spec = StepSpec {
                    label: "Installa git",
                    argv: dev_git_argv(),
                    privileged: true,
                    needs_reboot: true,
                };
                emit(spec.label);
                run_step(runner, &spec, emit, tail)?;
                outcome.needs_reboot = true;
                any_work = true;
            }
        }
    }
    outcome.skipped = !any_work;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::{HashMap, VecDeque};

    /// Esito in coda per uno streaming finto: (codice, righe emesse).
    type StreamOutcome = (Option<i32>, Vec<String>);

    /// Runner finto: `run` risponde da una mappa, `run_streaming`
    /// consuma una coda di esiti per argv (default: codice 0, nessun
    /// output). Niente rete, niente comandi reali.
    struct FakeRunner {
        run_map: HashMap<String, CmdOutput>,
        stream_queue: RefCell<HashMap<String, VecDeque<StreamOutcome>>>,
        /// (argv, privileged) di ogni passo eseguito in streaming.
        streamed: RefCell<Vec<(String, bool)>>,
        rustup: bool,
    }

    impl FakeRunner {
        fn new() -> Self {
            FakeRunner {
                run_map: HashMap::new(),
                stream_queue: RefCell::new(HashMap::new()),
                streamed: RefCell::new(Vec::new()),
                rustup: false,
            }
        }

        fn with_run(mut self, argv: &[&str], code: i32, stdout: &str) -> Self {
            self.run_map.insert(
                argv.join(" "),
                CmdOutput {
                    code: Some(code),
                    stdout: stdout.to_string(),
                    stderr: String::new(),
                },
            );
            self
        }

        fn with_stream(self, argv: &[&str], code: i32, lines: &[&str]) -> Self {
            self.stream_queue
                .borrow_mut()
                .entry(argv.join(" "))
                .or_default()
                .push_back((Some(code), lines.iter().map(|s| (*s).to_string()).collect()));
            self
        }

        fn with_rustup(mut self, installed: bool) -> Self {
            self.rustup = installed;
            self
        }

        fn streamed_argv(&self) -> Vec<String> {
            self.streamed
                .borrow()
                .iter()
                .map(|(a, _)| a.clone())
                .collect()
        }
    }

    impl Runner for FakeRunner {
        fn run(&self, argv: &[&str]) -> Result<CmdOutput, String> {
            Ok(self
                .run_map
                .get(&argv.join(" "))
                .cloned()
                .unwrap_or(CmdOutput {
                    code: Some(1),
                    stdout: String::new(),
                    stderr: String::new(),
                }))
        }

        fn run_streaming(
            &self,
            argv: &[&str],
            privileged: bool,
            on_line: &(dyn Fn(&str) + Sync),
        ) -> Result<Option<i32>, String> {
            self.streamed
                .borrow_mut()
                .push((argv.join(" "), privileged));
            let (code, lines) = self
                .stream_queue
                .borrow_mut()
                .get_mut(&argv.join(" "))
                .and_then(VecDeque::pop_front)
                .unwrap_or((Some(0), Vec::new()));
            for line in &lines {
                on_line(line);
            }
            Ok(code)
        }

        fn rustup_installed(&self) -> bool {
            self.rustup
        }
    }

    fn sink() -> impl Fn(&str) {
        |_| {}
    }

    #[test]
    fn group_metadata() {
        assert_eq!(GroupId::Codec.flag(), "codec");
        assert_eq!(GroupId::Gaming.flag(), "gaming");
        assert_eq!(GroupId::Dev.flag(), "dev");
        assert_eq!(GroupId::Codec.title(), "Codec multimediali");
        assert!(!GroupId::Gaming.description().is_empty());
        assert!(!GroupId::Dev.description().is_empty());
        assert_eq!(GroupState::NotInstalled.label(), "Non installato");
        assert_eq!(GroupState::Installed.label(), "Installato");
        assert_eq!(GroupState::Installing.label(), "Installazione…");
        assert_eq!(GroupState::Error.label(), "Errore");
    }

    #[test]
    fn codec_install_argv_normal() {
        let argv = codec_install_argv(false);
        assert_eq!(
            argv,
            vec![
                "rpm-ostree",
                "install",
                "ffmpeg",
                "gstreamer1-plugins-bad-freeworld",
                "gstreamer1-plugins-ugly",
                "gstreamer1-plugin-libav",
            ]
        );
        // Il nome è al singolare: `gstreamer1-plugin-libav`.
        assert!(argv.contains(&"gstreamer1-plugin-libav".to_string()));
        assert!(!argv.contains(&"gstreamer1-libav".to_string()));
    }

    #[test]
    fn codec_install_argv_override() {
        let argv = codec_install_argv(true);
        assert_eq!(
            argv,
            vec![
                "rpm-ostree",
                "override",
                "remove",
                "ffmpeg-free",
                "--install",
                "ffmpeg",
                "--install",
                "gstreamer1-plugins-bad-freeworld",
                "--install",
                "gstreamer1-plugins-ugly",
                "--install",
                "gstreamer1-plugin-libav",
            ]
        );
    }

    #[test]
    fn codec_repo_argv_points_to_free_release_45() {
        let argv = codec_repo_argv();
        assert_eq!(argv[0], "rpm-ostree");
        assert_eq!(argv[1], "install");
        assert_eq!(argv[2], RPMFUSION_FREE_RELEASE_URL);
        assert!(argv[2].contains("rpmfusion-free-release-45"));
        assert!(!argv[2].contains("nonfree"));
    }

    #[test]
    fn codec_retry_detection() {
        assert!(codec_needs_override_retry(
            "error: package ffmpeg-9.0.2 conflicts with ffmpeg-free"
        ));
        assert!(!codec_needs_override_retry("error: qualcosa d'altro"));
        assert!(!codec_needs_override_retry(""));
    }

    #[test]
    fn flatpak_parse_exact_line() {
        let out = "com.valvesoftware.Steam\norg.gnome.TextEditor\n";
        assert!(flatpak_app_installed(out, "com.valvesoftware.Steam"));
        assert!(!flatpak_app_installed(out, "com.valvesoftware.SteamLink"));
        assert!(!flatpak_app_installed(out, "org.gnome.Text"));
        assert!(!flatpak_app_installed("", "com.valvesoftware.Steam"));
    }

    #[test]
    fn rpm_installed_from_code() {
        let ok = CmdOutput {
            code: Some(0),
            stdout: "ffmpeg-9.0.2".to_string(),
            stderr: String::new(),
        };
        let missing = CmdOutput {
            code: Some(1),
            stdout: String::new(),
            stderr: "package not installed".to_string(),
        };
        assert!(rpm_packages_installed(&ok));
        assert!(!rpm_packages_installed(&missing));
    }

    #[test]
    fn rustup_command_is_official_and_unprivileged() {
        assert!(RUSTUP_INSTALL_SH.contains("https://sh.rustup.rs"));
        assert!(RUSTUP_INSTALL_SH.contains("-y --default-toolchain stable"));
        assert!(RUSTUP_INSTALL_SH.contains("--proto '=https'"));
        let argv = dev_rustup_argv();
        assert_eq!(argv[0], "sh");
        assert_eq!(argv[1], "-c");
        // Il passo rustup deve girare come utente: il flag privileged
        // è deciso in run_group; qui si verifica che l'argv non
        // contenga pkexec.
        assert!(!argv.iter().any(|a| a == "pkexec"));
    }

    #[test]
    fn gaming_argv() {
        assert_eq!(
            gaming_steam_argv(),
            vec![
                "flatpak",
                "install",
                "--system",
                "-y",
                "flathub",
                "com.valvesoftware.Steam",
            ]
        );
        assert_eq!(
            gaming_system_argv(),
            vec![
                "rpm-ostree",
                "install",
                "mangohud",
                "mangohud.i686",
                "gamemode",
                "gamemode.i686",
            ]
        );
        assert_eq!(
            dev_vscode_argv(),
            vec![
                "flatpak",
                "install",
                "--system",
                "-y",
                "flathub",
                "com.visualstudio.code",
            ]
        );
        assert_eq!(dev_git_argv(), vec!["rpm-ostree", "install", "git"]);
    }

    #[test]
    fn codec_all_installed_skips_everything() {
        let runner = FakeRunner::new()
            .with_run(
                &["rpm", "-q", "rpmfusion-free-release"],
                0,
                "rpmfusion-free-release-45-2",
            )
            .with_run(
                &[
                    "rpm",
                    "-q",
                    "ffmpeg",
                    "gstreamer1-plugins-bad-freeworld",
                    "gstreamer1-plugins-ugly",
                    "gstreamer1-plugin-libav",
                ],
                0,
                "ffmpeg-9.0.2",
            );
        assert!(check_installed(GroupId::Codec, &runner));
        let out = run_group(GroupId::Codec, &runner, &sink());
        assert!(out.success);
        assert!(out.skipped);
        assert!(!out.needs_reboot);
        assert!(out.error.is_none());
        assert!(runner.streamed_argv().is_empty());
    }

    #[test]
    fn codec_installs_repo_and_packages() {
        let runner = FakeRunner::new();
        assert!(!check_installed(GroupId::Codec, &runner));
        let out = run_group(GroupId::Codec, &runner, &sink());
        assert!(out.success);
        assert!(!out.skipped);
        assert!(out.needs_reboot);
        assert_eq!(
            runner.streamed_argv(),
            vec![
                codec_repo_argv().join(" "),
                codec_install_argv(false).join(" "),
            ]
        );
    }

    #[test]
    fn codec_falls_back_to_override_on_ffmpeg_free_conflict() {
        let normal = codec_install_argv(false);
        let normal_refs: Vec<&str> = normal.iter().map(String::as_str).collect();
        let over = codec_install_argv(true);
        let over_refs: Vec<&str> = over.iter().map(String::as_str).collect();
        // `rpm -q ffmpeg-free` dice "non installato", ma l'install
        // normale fallisce citando ffmpeg-free: si riprova con override.
        let runner = FakeRunner::new()
            .with_run(&["rpm", "-q", "ffmpeg-free"], 1, "")
            .with_stream(
                &normal_refs,
                1,
                &["error: ffmpeg-9.0.2 conflicts with ffmpeg-free"],
            )
            .with_stream(&over_refs, 0, &["override applicato"]);
        let out = run_group(GroupId::Codec, &runner, &sink());
        assert!(out.success, "errore: {:?}", out.error);
        assert!(out.needs_reboot);
        assert_eq!(
            runner.streamed_argv(),
            vec![
                codec_repo_argv().join(" "),
                normal.join(" "),
                over.join(" "),
            ]
        );
    }

    #[test]
    fn codec_uses_override_directly_when_ffmpeg_free_present() {
        let over = codec_install_argv(true);
        let over_refs: Vec<&str> = over.iter().map(String::as_str).collect();
        let runner = FakeRunner::new()
            .with_run(&["rpm", "-q", "ffmpeg-free"], 0, "ffmpeg-free-7.1")
            .with_stream(&over_refs, 0, &[]);
        let out = run_group(GroupId::Codec, &runner, &sink());
        assert!(out.success);
        assert_eq!(
            runner.streamed_argv(),
            vec![codec_repo_argv().join(" "), over.join(" ")]
        );
    }

    #[test]
    fn codec_hard_failure_reports_tail() {
        let normal = codec_install_argv(false);
        let normal_refs: Vec<&str> = normal.iter().map(String::as_str).collect();
        let runner = FakeRunner::new().with_stream(
            &normal_refs,
            1,
            &["errore di rete", "impossibile scaricare"],
        );
        let out = run_group(GroupId::Codec, &runner, &sink());
        assert!(!out.success);
        assert!(!out.skipped);
        let err = out.error.expect("serve un errore");
        assert!(err.contains("errore di rete"), "{err}");
        assert!(err.contains("impossibile scaricare"), "{err}");
    }

    #[test]
    fn gaming_skips_steam_when_present() {
        let sys = gaming_system_argv();
        let sys_refs: Vec<&str> = sys.iter().map(String::as_str).collect();
        let runner = FakeRunner::new()
            .with_run(
                &[
                    "flatpak",
                    "list",
                    "--system",
                    "--app",
                    "--columns=application",
                ],
                0,
                "com.valvesoftware.Steam\norg.foo.Bar\n",
            )
            .with_run(&["rpm", "-q", "mangohud", "gamemode"], 1, "")
            .with_stream(&sys_refs, 0, &[]);
        let out = run_group(GroupId::Gaming, &runner, &sink());
        assert!(out.success);
        assert!(!out.skipped);
        // Solo il passo di sistema: serve il riavvio.
        assert!(out.needs_reboot);
        assert_eq!(runner.streamed_argv(), vec![sys.join(" ")]);
    }

    #[test]
    fn gaming_all_installed_skips() {
        let runner = FakeRunner::new()
            .with_run(
                &[
                    "flatpak",
                    "list",
                    "--system",
                    "--app",
                    "--columns=application",
                ],
                0,
                "com.valvesoftware.Steam\n",
            )
            .with_run(&["rpm", "-q", "mangohud", "gamemode"], 0, "mangohud-0.8.3");
        assert!(check_installed(GroupId::Gaming, &runner));
        let out = run_group(GroupId::Gaming, &runner, &sink());
        assert!(out.success && out.skipped && !out.needs_reboot);
        assert!(runner.streamed_argv().is_empty());
    }

    #[test]
    fn dev_installs_only_git_when_rest_present() {
        let git = dev_git_argv();
        let git_refs: Vec<&str> = git.iter().map(String::as_str).collect();
        let runner = FakeRunner::new()
            .with_rustup(true)
            .with_run(
                &[
                    "flatpak",
                    "list",
                    "--system",
                    "--app",
                    "--columns=application",
                ],
                0,
                "com.visualstudio.code\n",
            )
            .with_run(&["rpm", "-q", "git"], 1, "")
            .with_stream(&git_refs, 0, &[]);
        assert!(!check_installed(GroupId::Dev, &runner));
        let out = run_group(GroupId::Dev, &runner, &sink());
        assert!(out.success);
        assert!(out.needs_reboot);
        assert_eq!(runner.streamed_argv(), vec![git.join(" ")]);
        // git via pkexec.
        assert!(runner.streamed.borrow()[0].1);
    }

    #[test]
    fn dev_rustup_runs_unprivileged_and_vscode_privileged() {
        let rustup = dev_rustup_argv();
        let rustup_refs: Vec<&str> = rustup.iter().map(String::as_str).collect();
        let vscode = dev_vscode_argv();
        let vscode_refs: Vec<&str> = vscode.iter().map(String::as_str).collect();
        let git = dev_git_argv();
        let git_refs: Vec<&str> = git.iter().map(String::as_str).collect();
        let runner = FakeRunner::new()
            .with_rustup(false)
            .with_run(
                &[
                    "flatpak",
                    "list",
                    "--system",
                    "--app",
                    "--columns=application",
                ],
                0,
                "",
            )
            .with_run(&["rpm", "-q", "git"], 0, "git-2.55")
            .with_stream(&rustup_refs, 0, &["info: profile set"])
            .with_stream(&vscode_refs, 0, &[]);
        let _ = git_refs;
        let out = run_group(GroupId::Dev, &runner, &sink());
        assert!(out.success);
        // Solo rustup + VS Code: nessun pacchetto di sistema, nessun riavvio.
        assert!(!out.needs_reboot);
        let streamed = runner.streamed.borrow();
        assert_eq!(streamed.len(), 2);
        assert_eq!(streamed[0].0, rustup.join(" "));
        assert!(
            !streamed[0].1,
            "rustup deve girare come utente, non con pkexec"
        );
        assert_eq!(streamed[1].0, vscode.join(" "));
        assert!(streamed[1].1, "il Flatpak di sistema va con pkexec");
    }

    #[test]
    fn dev_failure_stops_and_reports_error() {
        let rustup = dev_rustup_argv();
        let rustup_refs: Vec<&str> = rustup.iter().map(String::as_str).collect();
        let runner = FakeRunner::new().with_rustup(false).with_stream(
            &rustup_refs,
            1,
            &["curl: (6) could not resolve host"],
        );
        let out = run_group(GroupId::Dev, &runner, &sink());
        assert!(!out.success);
        let err = out.error.expect("serve un errore");
        assert!(err.contains("could not resolve host"), "{err}");
        // Fallito al primo passo: VS Code e git non sono stati toccati.
        assert_eq!(runner.streamed_argv().len(), 1);
    }

    #[test]
    fn log_lines_reach_emit() {
        let lines = std::sync::Mutex::new(Vec::<String>::new());
        let runner = FakeRunner::new().with_stream(
            &codec_repo_argv()
                .iter()
                .map(String::as_str)
                .collect::<Vec<&str>>(),
            0,
            &["riga uno", "riga due"],
        );
        let out = run_group(GroupId::Codec, &runner, &|l| {
            lines.lock().unwrap().push(l.to_string());
        });
        assert!(out.success);
        let lines = lines.lock().unwrap();
        assert!(lines.iter().any(|l| l.contains("riga uno")));
        assert!(lines.iter().any(|l| l.contains("riga due")));
        assert!(lines.iter().any(|l| l.starts_with("$ rpm-ostree")));
    }
}
