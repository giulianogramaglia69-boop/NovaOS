//! Tools the agent can call. Read-only tools execute immediately;
//! mutating tools (install/remove/set_*, shell mutations, file writes,
//! system changes) first return `ToolOutcome::NeedsConfirmation` and
//! only run after the user approves in the UI. The pending-action state
//! lives in core, not in the UI, so the rule cannot be bypassed by a UI
//! bug.
//!
//! Errors are honest and specific: a tool that cannot do its job says
//! why (missing command, app not found, no Flatpak remote) instead of
//! pretending success.
//!
//! The registry is one list: [`tool_definitions`] (schemas sent to the
//! LLM), [`execute_tool`] (dispatch) and [`execute_confirmed`] (the
//! second half of mutating tools). New tools belong in all three.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

use crate::llm::ToolDef;

/// What the agent knows about the current turn that tools need.
/// Kept tiny on purpose: only capabilities that change a tool's
/// behaviour (e.g. whether the model can see images).
#[derive(Debug, Clone, Default)]
pub struct ToolContext {
    /// True when the configured model accepts `image_url` parts.
    pub vision_supported: bool,
    /// Calendar ICS feed URL (may be empty = not configured).
    pub calendar_url: String,
    /// Calendar local .ics file path (may be empty).
    pub calendar_file: String,
    /// Whether persistent memory is enabled (settings toggle).
    pub memory_enabled: bool,
    /// Data directory override (tests only; None = ~/.local/share/nova-ai).
    pub data_dir: Option<PathBuf>,
}

/// Data dir for this turn: the test override, or the default.
fn turn_data_dir(ctx: &ToolContext) -> PathBuf {
    ctx.data_dir.clone().unwrap_or_else(crate::data_dir)
}

// ---------------------------------------------------------------------------
// Tool definitions (sent to the LLM)
// ---------------------------------------------------------------------------

pub fn tool_definitions() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "list_apps",
            description: "Elenca le applicazioni installate (nome e id). Usalo per sapere cosa c'è sul sistema prima di aprire o installare qualcosa.",
            parameters: json!({ "type": "object", "properties": {} }),
        },
        ToolDef {
            name: "open_app",
            description: "Apre un'applicazione per nome o id (ricerca approssimativa sul nome).",
            parameters: json!({
                "type": "object",
                "properties": { "query": { "type": "string", "description": "nome o id dell'app" } },
                "required": ["query"],
            }),
        },
        ToolDef {
            name: "install_app",
            description: "Installa un'applicazione da Flathub (Flatpak). Prima cerca l'id, poi chiede conferma all'utente.",
            parameters: json!({
                "type": "object",
                "properties": { "query": { "type": "string", "description": "nome dell'app da cercare su Flathub" } },
                "required": ["query"],
            }),
        },
        ToolDef {
            name: "remove_app",
            description: "Disinstalla un'applicazione Flatpak installata. Chiede conferma all'utente.",
            parameters: json!({
                "type": "object",
                "properties": { "query": { "type": "string", "description": "nome o id dell'app" } },
                "required": ["query"],
            }),
        },
        ToolDef {
            name: "get_volume",
            description: "Legge il volume attuale (0-100).",
            parameters: json!({ "type": "object", "properties": {} }),
        },
        ToolDef {
            name: "set_volume",
            description: "Imposta il volume (0-100).",
            parameters: json!({
                "type": "object",
                "properties": { "level": { "type": "integer", "description": "volume 0-100" } },
                "required": ["level"],
            }),
        },
        ToolDef {
            name: "get_brightness",
            description: "Legge la luminosità dello schermo (0-100).",
            parameters: json!({ "type": "object", "properties": {} }),
        },
        ToolDef {
            name: "set_brightness",
            description: "Imposta la luminosità dello schermo (0-100).",
            parameters: json!({
                "type": "object",
                "properties": { "level": { "type": "integer", "description": "luminosità 0-100" } },
                "required": ["level"],
            }),
        },
        ToolDef {
            name: "get_wifi",
            description: "Dice se il Wi-Fi è acceso o spento.",
            parameters: json!({ "type": "object", "properties": {} }),
        },
        ToolDef {
            name: "set_wifi",
            description: "Accende o spegne il Wi-Fi.",
            parameters: json!({
                "type": "object",
                "properties": { "enabled": { "type": "boolean" } },
                "required": ["enabled"],
            }),
        },
        ToolDef {
            name: "set_theme",
            description: "Imposta il tema chiaro o scuro (GNOME; sugli altri desktop è best-effort via gsettings).",
            parameters: json!({
                "type": "object",
                "properties": { "mode": { "type": "string", "enum": ["dark", "light"] } },
                "required": ["mode"],
            }),
        },
        ToolDef {
            name: "system_info",
            description: "Informazioni sul sistema: nome OS, desktop in uso, uptime, spazio libero.",
            parameters: json!({ "type": "object", "properties": {} }),
        },
        ToolDef {
            name: "shell",
            description: "Esegue un comando shell (bash -c) e restituisce stdout, stderr e exit code. I comandi di sola lettura girano subito; quelli che modificano il sistema chiedono prima conferma all'utente; i comandi catastrofici (cancellazione di / o della home, formattazione, scrittura su device) sono rifiutati.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "comando shell da eseguire" },
                    "timeout_secs": { "type": "integer", "description": "timeout in secondi (default 30, max 300)" }
                },
                "required": ["command"],
            }),
        },
        ToolDef {
            name: "files",
            description: "Gestisce file e directory. Azioni: read (legge un file di testo, max 64 KiB), write (crea/sovrascrive un file), mkdir, list (elenca una directory), search (cerca un testo in una directory, ricorsivo), move (sposta/rinomina), delete (cancella, sempre con conferma). I percorsi fuori dalla home richiedono conferma.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["read", "write", "mkdir", "list", "search", "move", "delete"] },
                    "path": { "type": "string", "description": "percorso del file o della directory" },
                    "content": { "type": "string", "description": "contenuto per write" },
                    "to": { "type": "string", "description": "destinazione per move" },
                    "pattern": { "type": "string", "description": "testo da cercare per search" }
                },
                "required": ["action", "path"],
            }),
        },
        ToolDef {
            name: "screenshot",
            description: "Cattura lo schermo intero e lo mostra al modello, che può così vedere cosa sta succedendo sul desktop. Funziona solo se il modello configurato supporta la visione, altrimenti restituisce un errore.",
            parameters: json!({ "type": "object", "properties": {} }),
        },
        ToolDef {
            name: "windows",
            description: "Gestisce le finestre del desktop. Azioni: list (elenca finestre aperte), focus (porta in primo piano), close (chiude, con conferma), move (sposta su un workspace), workspace (cambia workspace). Supportato su Niri e Hyprland; su GNOME/COSMIC restituisce un errore onesto.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["list", "focus", "close", "move", "workspace"] },
                    "window_id": { "type": "string", "description": "id della finestra (da list) per focus/close/move" },
                    "workspace": { "type": "string", "description": "nome o numero del workspace per move/workspace" }
                },
                "required": ["action"],
            }),
        },
        ToolDef {
            name: "system",
            description: "Gestisce il sistema. Azioni: os_update_check (controlla aggiornamenti OS via bootc/rpm-ostree), os_update (applica l'aggiornamento OS, con conferma e autenticazione di sistema), service (status/start/stop/restart di un servizio utente systemd), power_profile (legge o imposta il profilo energetico), bluetooth (power/list/connect/disconnect), audio (list dei dispositivi, set_default). Le azioni che modificano il sistema chiedono conferma.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["os_update_check", "os_update", "service", "power_profile", "bluetooth", "audio"] },
                    "name": { "type": "string", "description": "nome del servizio/dispositivo (service, bluetooth connect/disconnect, audio set_default)" },
                    "operation": { "type": "string", "description": "operazione: status/start/stop/restart (service), get/set (power_profile), power/list/connect/disconnect (bluetooth), list/set_default (audio)" },
                    "value": { "type": "string", "description": "valore: profilo energetico per power_profile set, on/off per bluetooth power" }
                },
                "required": ["action"],
            }),
        },
        ToolDef {
            name: "distrobox",
            description: "Gestisce container Distrobox (Ubuntu, Arch, Debian, Fedora...). Azioni: list (container esistenti), create (crea un container da un'immagine, con conferma), run (esegue un comando nel container), remove (elimina un container, con conferma).",
            parameters: json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["list", "create", "run", "remove"] },
                    "name": { "type": "string", "description": "nome del container" },
                    "image": { "type": "string", "description": "immagine per create, es. ubuntu:24.04, archlinux:latest, debian:12" },
                    "command": { "type": "string", "description": "comando da eseguire nel container per run" }
                },
                "required": ["action"],
            }),
        },
        ToolDef {
            name: "install_system_package",
            description: "Installa un pacchetto RPM di sistema con rpm-ostree (layering). Chiede conferma: il layering rallenta gli aggiornamenti e la modifica si applica al prossimo riavvio.",
            parameters: json!({
                "type": "object",
                "properties": { "package": { "type": "string", "description": "nome del pacchetto RPM" } },
                "required": ["package"],
            }),
        },
        ToolDef {
            name: "web",
            description: "Web senza chiave API. Azioni: search (ricerca DuckDuckGo, max 8 risultati), fetch (legge una URL come testo, max 100 KiB), download (scarica un file in ~/Downloads, max 50 MB).",
            parameters: json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["search", "fetch", "download"] },
                    "query": { "type": "string", "description": "testo da cercare per search" },
                    "url": { "type": "string", "description": "URL per fetch/download" },
                    "filename": { "type": "string", "description": "nome file opzionale per download" }
                },
                "required": ["action"],
            }),
        },
        ToolDef {
            name: "memory",
            description: "Memoria persistente: ricorda fatti per le prossime sessioni. Azioni: remember (salva un fatto, es. \"ricorda che il cane si chiama Rex\"), recall (cerca fatti per parola chiave; query vuota = i più recenti), forget (cancella i fatti che contengono la query). La memoria è attiva solo se abilitata nelle impostazioni.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["remember", "recall", "forget"] },
                    "text": { "type": "string", "description": "fatto da ricordare (remember) o parola chiave (recall/forget)" }
                },
                "required": ["action"],
            }),
        },
        ToolDef {
            name: "plan",
            description: "Piani multi-passo: scomponi un obiettivo in passi e portalo avanti anche su più turni. Azioni: create (titolo + passi), list (piani esistenti con avanzamento), show (dettaglio di un piano), update (segna un passo fatto/non fatto, indice da 0), complete (chiude il piano), delete (cancella il piano). Quando un piano è attivo il lavoro continua finché i passi sono finiti.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["create", "list", "show", "update", "complete", "delete"] },
                    "id": { "type": "string", "description": "id del piano (show/update/complete/delete)" },
                    "title": { "type": "string", "description": "titolo per create" },
                    "steps": { "type": "array", "items": { "type": "string" }, "description": "passi per create" },
                    "step": { "type": "integer", "description": "indice del passo per update (da 0)" },
                    "done": { "type": "boolean", "description": "fatto o no per update" }
                },
                "required": ["action"],
            }),
        },
        ToolDef {
            name: "calendar",
            description: "Calendario in sola lettura (ICS: URL del calendario o file .ics configurato nelle impostazioni). Azioni: today (eventi di oggi), tomorrow (eventi di domani), date (eventi di una data YYYY-MM-DD), search (cerca per parola nel titolo o nel luogo).",
            parameters: json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["today", "tomorrow", "date", "search"] },
                    "date": { "type": "string", "description": "data YYYY-MM-DD per date" },
                    "query": { "type": "string", "description": "parola da cercare per search" }
                },
                "required": ["action"],
            }),
        },
        ToolDef {
            name: "contacts",
            description: "Rubrica locale (file contacts.vcf in ~/.local/share/nova-ai/). Azioni: search (cerca un contatto per nome; query vuota = tutti). Restituisce nome, email e telefono.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["search"] },
                    "query": { "type": "string", "description": "nome da cercare (vuoto = tutti)" }
                },
                "required": ["action"],
            }),
        },
        ToolDef {
            name: "browser",
            description: "Browser vero (Firefox headless via WebDriver). Azioni: navigate (apre una URL), find (cerca un elemento con un selettore CSS), click (clicca un elemento, con conferma), type (scrive in un campo, con conferma), text (legge il testo visibile della pagina), screenshot (foto della pagina, visibile solo se il modello supporta la visione), close (chiude il browser). La prima volta scarica geckodriver (~4 MB).",
            parameters: json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["navigate", "find", "click", "type", "text", "screenshot", "close"] },
                    "url": { "type": "string", "description": "URL per navigate" },
                    "selector": { "type": "string", "description": "selettore CSS per find/click/type" },
                    "text": { "type": "string", "description": "testo da scrivere per type" }
                },
                "required": ["action"],
            }),
        },
    ]
}

/// JSON Schema of one tool's parameters, or None for unknown names.
pub fn tool_schema(name: &str) -> Option<Value> {
    tool_definitions()
        .into_iter()
        .find(|d| d.name == name)
        .map(|d| d.parameters)
}

/// Tools that mutate the system and therefore need confirmation.
pub fn tool_needs_confirmation(name: &str) -> bool {
    matches!(
        name,
        "install_app"
            | "remove_app"
            | "set_volume"
            | "set_brightness"
            | "set_wifi"
            | "set_theme"
            | "install_system_package"
    )
}

// ---------------------------------------------------------------------------
// Outcome + pending actions
// ---------------------------------------------------------------------------

/// Result of a tool call from the agent's point of view.
#[derive(Debug, Clone)]
pub enum ToolOutcome {
    /// Finished: this text goes back to the LLM as the tool result.
    Done(String),
    /// Finished with an image for the model (screenshot): `text` goes
    /// back as the tool result, the image follows as a user message.
    DoneWithImage {
        text: String,
        image_data_url: String,
    },
    /// Mutating tool: the user must approve first. The UI shows
    /// `description` with Approve/Deny; on approval the agent calls
    /// `execute_confirmed` with this same action.
    NeedsConfirmation(PendingAction),
}

/// A mutating action waiting for user approval.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingAction {
    pub tool: String,
    pub args: Value,
    /// Human-readable description shown in the confirmation dialog.
    pub description: String,
}

impl PendingAction {
    pub fn describe(&self) -> &str {
        &self.description
    }
}

/// Dispatch a tool call. Read-only tools run now; mutating tools return
/// `NeedsConfirmation` (nothing is changed yet). `ctx` carries the
/// turn's capabilities (e.g. whether the model supports vision).
pub fn execute_tool(name: &str, args: &Value, ctx: &ToolContext) -> Result<ToolOutcome, String> {
    match name {
        "list_apps" => Ok(ToolOutcome::Done(list_apps_text())),
        "open_app" => {
            let q = arg_str(args, "query")?;
            open_app(&q).map(ToolOutcome::Done)
        }
        "install_app" => {
            let q = arg_str(args, "query")?;
            let id = resolve_flatpak_id(&q)?;
            Ok(ToolOutcome::NeedsConfirmation(PendingAction {
                tool: name.to_string(),
                args: json!({ "id": id, "query": q }),
                description: format!("Installare {id} da Flathub?"),
            }))
        }
        "remove_app" => {
            let q = arg_str(args, "query")?;
            let id = resolve_installed_app_id(&q)?;
            Ok(ToolOutcome::NeedsConfirmation(PendingAction {
                tool: name.to_string(),
                args: json!({ "id": id, "query": q }),
                description: format!("Disinstallare {id}?"),
            }))
        }
        "get_volume" => get_volume().map(ToolOutcome::Done),
        "set_volume" => {
            let level = arg_level(args, "level")?;
            Ok(ToolOutcome::NeedsConfirmation(PendingAction {
                tool: name.to_string(),
                args: json!({ "level": level }),
                description: format!("Impostare il volume a {level}%?"),
            }))
        }
        "get_brightness" => get_brightness().map(ToolOutcome::Done),
        "set_brightness" => {
            let level = arg_level(args, "level")?;
            Ok(ToolOutcome::NeedsConfirmation(PendingAction {
                tool: name.to_string(),
                args: json!({ "level": level }),
                description: format!("Impostare la luminosità a {level}%?"),
            }))
        }
        "get_wifi" => get_wifi().map(ToolOutcome::Done),
        "set_wifi" => {
            let enabled = arg_bool(args, "enabled")?;
            Ok(ToolOutcome::NeedsConfirmation(PendingAction {
                tool: name.to_string(),
                args: json!({ "enabled": enabled }),
                description: format!(
                    "Accendere il Wi-Fi? (azione: {})",
                    if enabled { "accendi" } else { "spegni" }
                ),
            }))
        }
        "set_theme" => {
            let mode = arg_str(args, "mode")?;
            if mode != "dark" && mode != "light" {
                return Err("mode deve essere \"dark\" o \"light\"".to_string());
            }
            Ok(ToolOutcome::NeedsConfirmation(PendingAction {
                tool: name.to_string(),
                args: json!({ "mode": mode }),
                description: format!("Passare al tema {mode}?"),
            }))
        }
        "system_info" => Ok(ToolOutcome::Done(system_info())),
        "shell" => {
            let command = arg_str(args, "command")?;
            let timeout = args
                .get("timeout_secs")
                .and_then(|v| v.as_i64())
                .unwrap_or(30)
                .clamp(1, 300);
            match classify_shell_command(&command) {
                ShellClass::Denied => Err(format!(
                    "comando rifiutato: {}",
                    shell_deny_reason(&command).unwrap_or_default()
                )),
                ShellClass::ReadOnly => run_shell(&command, timeout, true).map(ToolOutcome::Done),
                ShellClass::Mutating => Ok(ToolOutcome::NeedsConfirmation(PendingAction {
                    tool: name.to_string(),
                    args: json!({ "command": command, "timeout_secs": timeout }),
                    description: format!(
                        "Eseguire questo comando (modifica il sistema)?\n{command}"
                    ),
                })),
            }
        }
        "memory" => execute_memory(args, ctx).map(ToolOutcome::Done),
        "plan" => execute_plan(args, ctx).map(ToolOutcome::Done),
        "calendar" => execute_calendar(args, ctx).map(ToolOutcome::Done),
        "contacts" => execute_contacts(args, ctx).map(ToolOutcome::Done),
        "browser" => execute_browser(args, ctx),
        "files" => execute_files(args, false).map(|o| match o {
            ToolOutcome::Done(s) => ToolOutcome::Done(s),
            other => other,
        }),
        "screenshot" => screenshot(ctx).map(|(text, data_url)| ToolOutcome::DoneWithImage {
            text,
            image_data_url: data_url,
        }),
        "windows" => execute_windows(args, false),
        "system" => execute_system(args, false),
        "distrobox" => execute_distrobox(args, false),
        "install_system_package" => {
            let package = arg_str(args, "package")?;
            Ok(ToolOutcome::NeedsConfirmation(PendingAction {
                tool: name.to_string(),
                args: json!({ "package": package }),
                description: format!(
                    "Installare il pacchetto di sistema {package} con rpm-ostree (layering)? Il layering rallenta gli aggiornamenti e la modifica si applica al prossimo riavvio."
                ),
            }))
        }
        "web" => execute_web(args),
        _ => Err(format!("tool sconosciuto: {name}")),
    }
}

/// Run a previously confirmed pending action. Only called after the
/// user pressed Approve.
pub fn execute_confirmed(action: &PendingAction) -> Result<String, String> {
    match action.tool.as_str() {
        "install_app" => {
            let id = action.args.get("id").and_then(|v| v.as_str()).unwrap_or("");
            flatpak_install(id)
        }
        "remove_app" => {
            let id = action.args.get("id").and_then(|v| v.as_str()).unwrap_or("");
            flatpak_remove(id)
        }
        "set_volume" => {
            let level = action
                .args
                .get("level")
                .and_then(|v| v.as_i64())
                .unwrap_or(-1);
            set_volume(level)
        }
        "set_brightness" => {
            let level = action
                .args
                .get("level")
                .and_then(|v| v.as_i64())
                .unwrap_or(-1);
            set_brightness(level)
        }
        "set_wifi" => {
            let enabled = action
                .args
                .get("enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            set_wifi(enabled)
        }
        "set_theme" => {
            let mode = action
                .args
                .get("mode")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            set_theme(mode)
        }
        "shell" => {
            let command = action
                .args
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let timeout = action
                .args
                .get("timeout_secs")
                .and_then(|v| v.as_i64())
                .unwrap_or(30);
            // Re-check the denylist at execution time: the command
            // must still be allowed even after approval.
            match classify_shell_command(command) {
                ShellClass::Denied => Err(format!(
                    "comando rifiutato: {}",
                    shell_deny_reason(command).unwrap_or_default()
                )),
                _ => run_shell(command, timeout, false),
            }
        }
        "browser" => {
            let op = action.args.get("op").and_then(|v| v.as_str()).unwrap_or("");
            let selector = action
                .args
                .get("selector")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            match op {
                "click" => crate::browser::browser_click(selector),
                "type" => {
                    let text = action
                        .args
                        .get("text")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    crate::browser::browser_type(selector, text)
                }
                _ => Err(format!("azione browser non confermabile: {op}")),
            }
        }
        "files" => match execute_files(&action.args, true)? {
            ToolOutcome::Done(s) => Ok(s),
            ToolOutcome::DoneWithImage { .. } => Err("azione file non valida".to_string()),
            ToolOutcome::NeedsConfirmation(_) => {
                Err("l'azione richiede ancora conferma".to_string())
            }
        },
        "windows" => match execute_windows(&action.args, true)? {
            ToolOutcome::Done(s) => Ok(s),
            ToolOutcome::DoneWithImage { .. } => Err("azione finestre non valida".to_string()),
            ToolOutcome::NeedsConfirmation(_) => {
                Err("l'azione richiede ancora conferma".to_string())
            }
        },
        "system" => match execute_system(&action.args, true)? {
            ToolOutcome::Done(s) => Ok(s),
            ToolOutcome::DoneWithImage { .. } => Err("azione sistema non valida".to_string()),
            ToolOutcome::NeedsConfirmation(_) => {
                Err("l'azione richiede ancora conferma".to_string())
            }
        },
        "distrobox" => match execute_distrobox(&action.args, true)? {
            ToolOutcome::Done(s) => Ok(s),
            ToolOutcome::DoneWithImage { .. } => Err("azione distrobox non valida".to_string()),
            ToolOutcome::NeedsConfirmation(_) => {
                Err("l'azione richiede ancora conferma".to_string())
            }
        },
        "install_system_package" => {
            let package = action
                .args
                .get("package")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            rpm_ostree_install(package)
        }
        other => Err(format!("azione non confermabile: {other}")),
    }
}

// ---------------------------------------------------------------------------
// Argument validation
// ---------------------------------------------------------------------------

fn arg_str(args: &Value, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("argomento mancante o vuoto: {key}"))
}

/// Optional string argument (None when missing or empty).
fn arg_opt_str(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn arg_bool(args: &Value, key: &str) -> Result<bool, String> {
    args.get(key)
        .and_then(|v| v.as_bool())
        .ok_or_else(|| format!("argomento mancante o non booleano: {key}"))
}

fn arg_level(args: &Value, key: &str) -> Result<i64, String> {
    let v = args
        .get(key)
        .and_then(|v| v.as_i64())
        .ok_or_else(|| format!("argomento mancante o non intero: {key}"))?;
    if !(0..=100).contains(&v) {
        return Err(format!("{key} deve essere tra 0 e 100 (dato: {v})"));
    }
    Ok(v)
}

// ---------------------------------------------------------------------------
// .desktop inventory
// ---------------------------------------------------------------------------

/// One installed application.
#[derive(Debug, Clone, PartialEq)]
pub struct AppInfo {
    /// Desktop file id, e.g. "org.mozilla.firefox" or "firefox".
    pub id: String,
    pub name: String,
    pub exec: String,
    pub icon: String,
    pub categories: String,
    /// true for Flatpak apps (from /var/lib/flatpak or ~/.local/share/flatpak).
    pub flatpak: bool,
}

/// Directories scanned for .desktop files.
pub fn app_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![
        PathBuf::from("/usr/share/applications"),
        PathBuf::from("/var/lib/flatpak/exports/share/applications"),
    ];
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(&home).join(".local/share/applications"));
        dirs.push(PathBuf::from(&home).join(".local/share/flatpak/exports/share/applications"));
    }
    dirs
}

/// Parse one .desktop file. Returns None when it is not an application
/// entry (or is hidden / NoDisplay).
pub fn parse_desktop_file(path: &Path) -> Option<AppInfo> {
    let text = std::fs::read_to_string(path).ok()?;
    parse_desktop_text(&text, path)
}

pub fn parse_desktop_text(text: &str, path: &Path) -> Option<AppInfo> {
    let mut in_entry = false;
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry || line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            // Skip localized keys (Name[it]=...); keep the plain ones.
            if !k.contains('[') {
                map.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
    }
    if map.get("Type").map(|s| s.as_str()) != Some("Application") {
        return None;
    }
    if map.get("Hidden").map(|s| s.as_str()) == Some("true")
        || map.get("NoDisplay").map(|s| s.as_str()) == Some("true")
    {
        return None;
    }
    let name = map.get("Name")?.clone();
    let exec = map.get("Exec").cloned().unwrap_or_default();
    if exec.is_empty() {
        return None;
    }
    let id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let flatpak = path.to_string_lossy().contains("flatpak");
    Some(AppInfo {
        id,
        name,
        exec,
        icon: map.get("Icon").cloned().unwrap_or_default(),
        categories: map.get("Categories").cloned().unwrap_or_default(),
        flatpak,
    })
}

/// Full inventory, sorted by name (case-insensitive), deduped by id.
pub fn list_apps() -> Vec<AppInfo> {
    let mut apps: Vec<AppInfo> = Vec::new();
    let mut seen: std::collections::HashSet<String> = Default::default();
    for dir in app_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
                continue;
            }
            if let Some(app) = parse_desktop_file(&path)
                && seen.insert(app.id.clone())
            {
                apps.push(app);
            }
        }
    }
    apps.sort_by_key(|a| a.name.to_ascii_lowercase());
    apps
}

fn list_apps_text() -> String {
    let apps = list_apps();
    if apps.is_empty() {
        return "Nessuna applicazione trovata nelle directory standard.".to_string();
    }
    let mut out = format!("Applicazioni installate ({}):\n", apps.len());
    for app in apps {
        out.push_str(&format!("- {} ({})\n", app.name, app.id));
    }
    out
}

/// Fuzzy resolve: exact id, then exact name (case-insensitive), then
/// substring on name. Returns the best match or an honest error.
fn resolve_app(query: &str) -> Result<AppInfo, String> {
    let apps = list_apps();
    let q = query.to_ascii_lowercase();
    if let Some(a) = apps.iter().find(|a| a.id.to_ascii_lowercase() == q) {
        return Ok(a.clone());
    }
    if let Some(a) = apps.iter().find(|a| a.name.to_ascii_lowercase() == q) {
        return Ok(a.clone());
    }
    let matches: Vec<&AppInfo> = apps
        .iter()
        .filter(|a| a.name.to_ascii_lowercase().contains(&q))
        .collect();
    match matches.len() {
        0 => Err(format!("nessuna applicazione trovata per \"{query}\"")),
        1 => Ok(matches[0].clone()),
        _ => {
            let names: Vec<String> = matches.iter().map(|a| a.name.clone()).collect();
            Err(format!(
                "troppe corrispondenze per \"{query}\": {} — sii più preciso",
                names.join(", ")
            ))
        }
    }
}

fn open_app(query: &str) -> Result<String, String> {
    let app = resolve_app(query)?;
    // Preferred: gtk-launch with the desktop id.
    let launched = Command::new("gtk-launch")
        .arg(&app.id)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .is_ok();
    if launched {
        return Ok(format!("Apro {}…", app.name));
    }
    // Fallback: run the Exec line (field codes stripped) via sh.
    let exec = strip_exec_codes(&app.exec);
    let spawned = Command::new("sh")
        .arg("-c")
        .arg(format!("{exec} &"))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .is_ok();
    if spawned {
        Ok(format!("Apro {}…", app.name))
    } else {
        Err(format!(
            "non riesco ad avviare {} (né gtk-launch né Exec hanno funzionato)",
            app.name
        ))
    }
}

/// Remove .desktop field codes (%f, %F, %u, %U, %i, %c, %k, ...) from an
/// Exec line.
fn strip_exec_codes(exec: &str) -> String {
    let mut out = String::with_capacity(exec.len());
    let mut chars = exec.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            let _ = chars.next(); // skip the code letter
        } else {
            out.push(c);
        }
    }
    out.trim().to_string()
}

// ---------------------------------------------------------------------------
// Flatpak
// ---------------------------------------------------------------------------

fn flatpak_available() -> Result<(), String> {
    match Command::new("flatpak").arg("--version").output() {
        Ok(o) if o.status.success() => Ok(()),
        _ => Err("flatpak non è installato o non funziona su questo sistema".to_string()),
    }
}

fn flatpak_has_flathub() -> Result<(), String> {
    let out = Command::new("flatpak")
        .args(["remote-list", "--columns=name"])
        .output()
        .map_err(|e| format!("flatpak remote-list fallito: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    if text.lines().any(|l| l.trim() == "flathub") {
        Ok(())
    } else {
        Err(
            "il remote Flathub non è configurato (manca 'flathub' in flatpak remote-list)"
                .to_string(),
        )
    }
}

/// Resolve a search query to a Flatpak app id via `flatpak search`.
/// Prefers an exact id/name match; otherwise takes the first result.
fn resolve_flatpak_id(query: &str) -> Result<String, String> {
    flatpak_available()?;
    flatpak_has_flathub()?;
    let out = Command::new("flatpak")
        .args(["search", "--columns=application", query])
        .output()
        .map_err(|e| format!("flatpak search fallito: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let ids: Vec<&str> = text
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();
    if ids.is_empty() {
        return Err(format!("nessun risultato su Flathub per \"{query}\""));
    }
    let q = query.to_ascii_lowercase();
    if let Some(id) = ids.iter().find(|id| id.to_ascii_lowercase() == q) {
        return Ok(id.to_string());
    }
    Ok(ids[0].to_string())
}

/// Resolve a query to an installed Flatpak id (from the .desktop
/// inventory first, then `flatpak list`).
fn resolve_installed_app_id(query: &str) -> Result<String, String> {
    if let Ok(app) = resolve_app(query) {
        if app.flatpak {
            return Ok(app.id);
        }
        return Err(format!(
            "{} è installato ma non è un Flatpak: la rimozione via NovaAI supporta solo Flatpak",
            app.name
        ));
    }
    flatpak_available()?;
    let out = Command::new("flatpak")
        .args(["list", "--app", "--columns=application"])
        .output()
        .map_err(|e| format!("flatpak list fallito: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let q = query.to_ascii_lowercase();
    for id in text.lines().map(|l| l.trim()).filter(|l| !l.is_empty()) {
        if id.to_ascii_lowercase() == q || id.to_ascii_lowercase().contains(&q) {
            return Ok(id.to_string());
        }
    }
    Err(format!(
        "nessuna app Flatpak installata trovata per \"{query}\""
    ))
}

fn flatpak_install(id: &str) -> Result<String, String> {
    flatpak_available()?;
    flatpak_has_flathub()?;
    let out = Command::new("flatpak")
        .args(["install", "--user", "-y", "flathub", id])
        .output()
        .map_err(|e| format!("flatpak install fallito: {e}"))?;
    if out.status.success() {
        Ok(format!("{id} installato da Flathub."))
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(format!("installazione di {id} fallita: {}", err.trim()))
    }
}

fn flatpak_remove(id: &str) -> Result<String, String> {
    flatpak_available()?;
    let out = Command::new("flatpak")
        .args(["uninstall", "--user", "-y", id])
        .output()
        .map_err(|e| format!("flatpak uninstall fallito: {e}"))?;
    if out.status.success() {
        Ok(format!("{id} disinstallato."))
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(format!("rimozione di {id} fallita: {}", err.trim()))
    }
}

// ---------------------------------------------------------------------------
// Volume / brightness / wifi / theme
// ---------------------------------------------------------------------------

fn run_capture(cmd: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(cmd).args(args).output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            format!("comando non trovato: {cmd}")
        } else {
            format!("errore eseguendo {cmd}: {e}")
        }
    })?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("{cmd} fallito: {}", err.trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn get_volume() -> Result<String, String> {
    let out = run_capture("wpctl", &["get-volume", "@DEFAULT_AUDIO_SINK@"])?;
    // "Volume: 0.42" (muted adds "[MUTED]")
    let v: f64 = out
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("output wpctl non riconosciuto: {out}"))?;
    Ok(format!("Volume attuale: {}%.", (v * 100.0).round() as i64))
}

fn set_volume(level: i64) -> Result<String, String> {
    if !(0..=100).contains(&level) {
        return Err(format!("volume deve essere tra 0 e 100 (dato: {level})"));
    }
    run_capture(
        "wpctl",
        &[
            "set-volume",
            "@DEFAULT_AUDIO_SINK@",
            &format!("{}", level as f64 / 100.0),
        ],
    )?;
    Ok(format!("Volume impostato a {level}%."))
}

fn get_brightness() -> Result<String, String> {
    let cur: i64 = run_capture("brightnessctl", &["get"])?
        .parse()
        .map_err(|_| "output brightnessctl get non numerico".to_string())?;
    let max: i64 = run_capture("brightnessctl", &["max"])?
        .parse()
        .map_err(|_| "output brightnessctl max non numerico".to_string())?;
    if max <= 0 {
        return Err("brightnessctl max è zero".to_string());
    }
    Ok(format!(
        "Luminosità attuale: {}%.",
        (cur * 100 + max / 2) / max
    ))
}

fn set_brightness(level: i64) -> Result<String, String> {
    if !(0..=100).contains(&level) {
        return Err(format!(
            "luminosità deve essere tra 0 e 100 (dato: {level})"
        ));
    }
    run_capture("brightnessctl", &["set", &format!("{level}%")])?;
    Ok(format!("Luminosità impostata a {level}%."))
}

fn get_wifi() -> Result<String, String> {
    let out = run_capture("nmcli", &["radio", "wifi"])?;
    match out.as_str() {
        "enabled" => Ok("Il Wi-Fi è acceso.".to_string()),
        "disabled" => Ok("Il Wi-Fi è spento.".to_string()),
        other => Err(format!("stato Wi-Fi non riconosciuto: {other}")),
    }
}

fn set_wifi(enabled: bool) -> Result<String, String> {
    run_capture(
        "nmcli",
        &["radio", "wifi", if enabled { "on" } else { "off" }],
    )?;
    Ok(if enabled {
        "Wi-Fi acceso.".to_string()
    } else {
        "Wi-Fi spento.".to_string()
    })
}

fn set_theme(mode: &str) -> Result<String, String> {
    let scheme = match mode {
        "dark" => "prefer-dark",
        "light" => "prefer-light",
        _ => return Err("mode deve essere \"dark\" o \"light\"".to_string()),
    };
    run_capture(
        "gsettings",
        &["set", "org.gnome.desktop.interface", "color-scheme", scheme],
    )?;
    Ok(format!(
        "Tema {mode} impostato (GNOME; su COSMIC e i tiling è best-effort via gsettings)."
    ))
}

// ---------------------------------------------------------------------------
// Shell
// ---------------------------------------------------------------------------

/// How a shell command is classified for safety.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellClass {
    /// Read-only: runs immediately.
    ReadOnly,
    /// Mutates the system: needs user confirmation first.
    Mutating,
    /// Catastrophic / irreversible: refused outright, no confirmation.
    Denied,
}

/// First words that always mutate the system (any pipeline segment).
/// Interpreters and VCS tools are here on purpose: `git`, `python3`,
/// `sed` or `cargo` can all write, so they ask first even for reads —
/// the confirmation dialog shows the command, and the user approves
/// routine ones. Safe default over convenience.
const MUTATING_COMMANDS: &[&str] = &[
    "rm",
    "mv",
    "cp",
    "mkdir",
    "rmdir",
    "touch",
    "ln",
    "chmod",
    "chown",
    "chgrp",
    "tee",
    "install",
    "systemctl",
    "rpm-ostree",
    "bootc",
    "flatpak",
    "dnf",
    "dnf5",
    "distrobox",
    "toolbox",
    "podman",
    "docker",
    "kill",
    "killall",
    "pkill",
    "useradd",
    "usermod",
    "passwd",
    "mount",
    "umount",
    "nmcli",
    "gsettings",
    "dconf",
    "timedatectl",
    "hostnamectl",
    "localectl",
    "firewall-cmd",
    "setenforce",
    "grub2-mkconfig",
    "dracut",
    "make",
    "cargo",
    "pip",
    "npm",
    "git",
    "sed",
    "awk",
    "perl",
    "python3",
    "python",
    "sh",
    "bash",
    "sudo",
    "pkexec",
    "su",
    "dd",
    "truncate",
    "shred",
    "wget",
    "curl",
];

/// Filesystem roots whose recursive removal is refused.
const DENY_RM_TARGETS: &[&str] = &[
    "/", "~", "$HOME", "${HOME}", "/home", "/usr", "/etc", "/var", "/boot", "/opt",
];

/// Split a shell line into pipeline segments, respecting single and
/// double quotes (a `|` inside quotes is not a separator).
fn split_pipeline(command: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut chars = command.chars().peekable();
    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                cur.push(c);
                if c == q {
                    quote = None;
                }
            }
            None => match c {
                '\'' | '"' => {
                    quote = Some(c);
                    cur.push(c);
                }
                ';' | '&' | '|' => {
                    // `&&` / `||` are two chars: swallow the second.
                    if (c == '&' || c == '|') && chars.peek() == Some(&c) {
                        let _ = chars.next();
                    }
                    segments.push(std::mem::take(&mut cur));
                }
                _ => cur.push(c),
            },
        }
    }
    segments.push(cur);
    segments
}

/// Remove quoted spans from a segment (for operator detection).
fn strip_quoted(segment: &str) -> String {
    let mut out = String::new();
    let mut quote: Option<char> = None;
    for c in segment.chars() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
            }
            None => match c {
                '\'' | '"' => quote = Some(c),
                _ => out.push(c),
            },
        }
    }
    out
}

/// First word of a segment (after leading whitespace).
fn first_word(segment: &str) -> &str {
    segment
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_matches(|c| c == '"' || c == '\'')
}

/// Skip leading `-flag` tokens; flags listed in `value_flags` also eat
/// the following token (their value). `--flag=value` forms eat nothing
/// extra. Stops at the first non-flag token.
fn skip_flags<'a>(mut rest: &'a str, value_flags: &[&str]) -> &'a str {
    loop {
        let w = first_word(rest);
        if !w.starts_with('-') || w == "-" {
            break;
        }
        let after = rest[w.len()..].trim_start();
        if !w.contains('=') && value_flags.contains(&w) {
            let v = first_word(after);
            rest = after[v.len()..].trim_start();
        } else {
            rest = after;
        }
    }
    rest
}

/// Strip a leading run of command wrappers (with their flags and flag
/// values) so the real command is what gets classified: `sudo -u root
/// rm -rf /`, `env FOO=1 rm -rf ~`, `xargs rm -rf /` and `timeout 5 rm
/// -rf /` are all still `rm -rf /`. Without this, a wrapper hides the
/// dangerous verb from the first-word check and the command would run
/// with no confirmation at all.
fn strip_command_prefix(segment: &str) -> &str {
    let mut rest = segment.trim_start();
    loop {
        let word = first_word(rest).to_owned();
        let after = rest[word.len()..].trim_start();
        match word.as_str() {
            "sudo" | "pkexec" => {
                rest = skip_flags(
                    after,
                    &[
                        "-u",
                        "-g",
                        "-h",
                        "-p",
                        "-C",
                        "-D",
                        "-R",
                        "-U",
                        "--user",
                        "--group",
                        "--host",
                        "--prompt",
                        "--chdir",
                        "--close-from",
                    ],
                );
            }
            "env" => {
                // `env [-i] [-u VAR] [VAR=val ...] cmd`
                let mut r = after;
                loop {
                    let w = first_word(r);
                    if w.starts_with('-') && w != "-" {
                        let r2 = r[w.len()..].trim_start();
                        r = if w == "-u" || w == "--unset" {
                            let v = first_word(r2);
                            r2[v.len()..].trim_start()
                        } else {
                            r2
                        };
                    } else if w.contains('=') && !w.starts_with('=') {
                        r = r[w.len()..].trim_start();
                    } else {
                        break;
                    }
                }
                rest = r;
            }
            // `command -v/-V` only asks what a command is: do not strip,
            // so `command -v rm` stays a harmless read.
            "command" if after.starts_with("-v") || after.starts_with("-V") => break,
            "command" | "time" | "setsid" | "nohup" => {
                rest = skip_flags(after, &[]);
            }
            "nice" => {
                rest = skip_flags(after, &["-n", "--adjustment"]);
            }
            "timeout" => {
                // `timeout [flags] DURATION cmd`: skip flags, then the duration.
                let r = skip_flags(after, &["-k", "--kill-after", "-s", "--signal"]);
                let dur = first_word(r);
                rest = r[dur.len()..].trim_start();
            }
            "xargs" => {
                rest = skip_flags(
                    after,
                    &[
                        "-n",
                        "-I",
                        "-P",
                        "-a",
                        "-d",
                        "-E",
                        "-L",
                        "-s",
                        "--max-args",
                        "--replace",
                        "--max-procs",
                        "--arg-file",
                        "--delimiter",
                        "--eof",
                        "--max-lines",
                        "--max-chars",
                    ],
                );
            }
            "stdbuf" => {
                rest = skip_flags(after, &["-o", "-e", "-i", "--output", "--error", "--input"]);
            }
            _ => break,
        }
    }
    rest
}

/// Deny reason for a single segment, or None.
fn segment_denied(segment: &str) -> Option<String> {
    let seg = segment.trim();
    if seg.is_empty() {
        return None;
    }
    // Fork bomb.
    if seg.contains(":(){:|:&};:") || seg.contains(":(){") {
        return Some("fork bomb".to_string());
    }
    // Strip command wrappers: `sudo -u root rm -rf /` is still `rm -rf /`.
    let effective = strip_command_prefix(seg);
    let word = first_word(effective);
    // Filesystem destruction tools.
    if word.starts_with("mkfs") || matches!(word, "mkswap" | "fdisk" | "parted" | "sgdisk") {
        return Some(format!("{word} può distruggere dischi/partizioni"));
    }
    if word == "shred" && effective.contains("/dev/") {
        return Some("shred su un device".to_string());
    }
    if word == "dd" && effective.contains("of=/dev/") {
        return Some("dd con destinazione su un device".to_string());
    }
    // Output redirection onto a device (e.g. `echo x > /dev/sda`).
    let bare = strip_quoted(effective);
    if let Some(pos) = bare.find('>') {
        let target = bare[pos + 1..].trim_start_matches(['>', ' ']);
        if target.starts_with("/dev/") {
            return Some("reindirizzamento su un device".to_string());
        }
    }
    // Recursive removal of a system root.
    if word == "rm" {
        let tokens: Vec<&str> = effective.split_whitespace().collect();
        for tok in tokens.iter().skip(1) {
            if tok.starts_with('-') {
                continue;
            }
            let target = tok.trim_end_matches("/*").trim_end_matches('/');
            let target = if target.is_empty() { "/" } else { target };
            if DENY_RM_TARGETS.contains(&target) {
                return Some(format!("cancellazione ricorsiva di {target}"));
            }
            break; // only the first non-flag target counts
        }
    }
    None
}

/// Classify a shell command: Denied (catastrophic) > Mutating >
/// ReadOnly. The check runs per pipeline segment.
pub fn classify_shell_command(command: &str) -> ShellClass {
    let mut mutating = false;
    for seg in split_pipeline(command) {
        if segment_denied(&seg).is_some() {
            return ShellClass::Denied;
        }
        let word = first_word(&seg);
        // The real command after any wrappers (`env rm …`, `xargs rm …`).
        let stripped_word = first_word(strip_command_prefix(&seg));
        if MUTATING_COMMANDS.contains(&word) || MUTATING_COMMANDS.contains(&stripped_word) {
            mutating = true;
        }
        // `find … -delete` / `-exec …` deletes files or runs commands.
        if stripped_word == "find" {
            let bare = strip_quoted(&seg);
            if bare
                .split_whitespace()
                .any(|t| t == "-delete" || t == "-exec" || t == "-execdir")
            {
                mutating = true;
            }
        }
        let bare = strip_quoted(&seg);
        if bare.contains('>') {
            mutating = true; // output redirection writes a file
        }
    }
    if mutating {
        ShellClass::Mutating
    } else {
        ShellClass::ReadOnly
    }
}

/// Why a command was denied (for the error message).
pub fn shell_deny_reason(command: &str) -> Option<String> {
    split_pipeline(command)
        .iter()
        .find_map(|seg| segment_denied(seg))
}

/// Truncate output to `cap` chars, noting the cut.
fn truncate_output(s: &str, cap: usize) -> String {
    if s.chars().count() <= cap {
        return s.to_string();
    }
    let mut out: String = s.chars().take(cap).collect();
    out.push_str("\n… [output troncato]");
    out
}

/// Public truncation helper (the browser tool truncates page text
/// with it).
pub fn truncate_public(s: &str, cap: usize) -> String {
    truncate_output(s, cap)
}

// ---------------------------------------------------------------------------
// Shell sandbox (bubblewrap)
// ---------------------------------------------------------------------------

/// Find an executable in $PATH.
fn find_in_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|p| {
        std::env::split_paths(&p).find_map(|dir| {
            let cand = dir.join(name);
            if cand.is_file() { Some(cand) } else { None }
        })
    })
}

/// The bubblewrap argv for a read-only command: / read-only, /tmp
/// writable, $HOME read-only, no network. `net` toggles
/// --unshare-net (some systems cannot configure loopback in a new
/// net namespace — the caller retries without it then).
pub fn sandbox_argv(net: bool) -> Option<Vec<String>> {
    let bwrap = find_in_path("bwrap")?;
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let mut argv = vec![
        bwrap.to_string_lossy().to_string(),
        "--ro-bind".to_string(),
        "/".to_string(),
        "/".to_string(),
        "--dev".to_string(),
        "/dev".to_string(),
        "--proc".to_string(),
        "/proc".to_string(),
        "--tmpfs".to_string(),
        "/tmp".to_string(),
        "--ro-bind".to_string(),
        home.clone(),
        home,
    ];
    if net {
        argv.push("--unshare-net".to_string());
    }
    argv.push("--".to_string());
    Some(argv)
}

pub fn bwrap_available() -> bool {
    find_in_path("bwrap").is_some()
}

/// Run argv, capturing stdout/stderr. Returns (exit code, stdout,
/// stderr). The first element is the program to run.
fn run_argv(argv: &[String]) -> Result<(i32, String, String), String> {
    let (prog, args) = argv
        .split_first()
        .ok_or_else(|| "comando vuoto".to_string())?;
    let out = Command::new(prog).args(args).output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            format!("comando '{prog}' non trovato")
        } else {
            format!("errore eseguendo il comando: {e}")
        }
    })?;
    Ok((
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    ))
}

fn format_shell_output(stdout: &str, stderr: &str, code: i32, timeout_secs: i64) -> String {
    let mut s = String::new();
    if !stdout.trim().is_empty() {
        s.push_str(&truncate_output(stdout, 24_000));
    }
    if !stderr.trim().is_empty() {
        if !s.is_empty() {
            s.push('\n');
        }
        s.push_str("[stderr]\n");
        s.push_str(&truncate_output(stderr, 8_000));
    }
    if s.is_empty() {
        s.push_str("(nessun output)");
    }
    s.push_str(&format!("\n[exit code: {code}]"));
    if code == 124 {
        s.push_str(&format!(
            " (timeout: il comando ha superato i {timeout_secs}s)"
        ));
    }
    s
}

/// Run a command via `bash -c` under the `timeout` CLI. Output is
/// capped; a timeout is reported, not hidden (exit 124).
///
/// When `sandboxed` is true (read-only commands), the command runs
/// under bubblewrap: / read-only, /tmp writable, $HOME read-only, no
/// network. Mutating commands the user approved run OUTSIDE the
/// sandbox (they must be able to write). When bubblewrap is missing
/// the command still runs, with a one-time note in the result.
fn run_shell(command: &str, timeout_secs: i64, sandboxed: bool) -> Result<String, String> {
    let plain: Vec<String> = vec![
        "timeout".to_string(),
        timeout_secs.to_string(),
        "bash".to_string(),
        "-c".to_string(),
        command.to_string(),
    ];
    if sandboxed {
        if let Some(mut prefix) = sandbox_argv(true) {
            let mut argv = std::mem::take(&mut prefix);
            argv.extend(plain.iter().cloned());
            let (code, stdout, stderr) = run_argv(&argv)?;
            // bwrap itself failed to set up the net namespace (this
            // VM cannot configure loopback there): retry without
            // --unshare-net instead of failing a harmless command.
            if !stderr.is_empty()
                && stderr.contains("bwrap: loopback")
                && let Some(mut prefix2) = sandbox_argv(false)
            {
                let mut argv2 = std::mem::take(&mut prefix2);
                argv2.extend(plain.iter().cloned());
                let (code2, stdout2, stderr2) = run_argv(&argv2)?;
                return Ok(format_shell_output(&stdout2, &stderr2, code2, timeout_secs));
            }
            let _ = code;
            return Ok(format_shell_output(&stdout, &stderr, code, timeout_secs));
        }
        static SANDBOX_NOTE_SHOWN: std::sync::atomic::AtomicBool =
            std::sync::atomic::AtomicBool::new(false);
        let note = if !SANDBOX_NOTE_SHOWN.swap(true, std::sync::atomic::Ordering::SeqCst) {
            "\n(nota: sandbox bubblewrap non disponibile — installa bubblewrap per isolare i comandi di sola lettura)"
        } else {
            ""
        };
        let (code, stdout, stderr) = run_argv(&plain)?;
        let mut out = format_shell_output(&stdout, &stderr, code, timeout_secs);
        out.push_str(note);
        return Ok(out);
    }
    let (code, stdout, stderr) = run_argv(&plain)?;
    Ok(format_shell_output(&stdout, &stderr, code, timeout_secs))
}

// ---------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------

const FILES_READ_CAP: usize = 65_536; // 64 KiB
const FILES_SEARCH_MAX_FILES: usize = 2000;
const FILES_SEARCH_MAX_HITS: usize = 50;

/// Expand `~` / `~/...` and resolve relative paths against $HOME
/// (the agent has no meaningful working directory).
fn expand_path(path: &str) -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    if path == "~" {
        return PathBuf::from(home);
    }
    if let Some(rest) = path.strip_prefix("~/") {
        return PathBuf::from(&home).join(rest);
    }
    let p = PathBuf::from(path);
    if p.is_absolute() {
        p
    } else {
        PathBuf::from(&home).join(p)
    }
}

/// True when `path` is inside $HOME (lexically, after expansion).
fn under_home(path: &Path) -> bool {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let home = PathBuf::from(home);
    path.starts_with(&home)
}

fn files_pending(args: &Value, description: String) -> ToolOutcome {
    ToolOutcome::NeedsConfirmation(PendingAction {
        tool: "files".to_string(),
        args: args.clone(),
        description,
    })
}

fn execute_files(args: &Value, confirmed: bool) -> Result<ToolOutcome, String> {
    let action = arg_str(args, "action")?;
    let path = arg_str(args, "path")?;
    let p = expand_path(&path);
    let disp = p.display().to_string();
    match action.as_str() {
        "read" => {
            let bytes = std::fs::read(&p).map_err(|e| format!("lettura di {disp} fallita: {e}"))?;
            let text = String::from_utf8(bytes)
                .map_err(|_| format!("{disp} non è un file di testo (binario?)"))?;
            let mut out = truncate_output(&text, FILES_READ_CAP);
            if text.chars().count() > FILES_READ_CAP {
                out.push_str(&format!("\n[file troncato a {FILES_READ_CAP} caratteri]"));
            }
            Ok(ToolOutcome::Done(out))
        }
        "write" => {
            let content = arg_str(args, "content").unwrap_or_default();
            if !under_home(&p) && !confirmed {
                return Ok(files_pending(
                    args,
                    format!("Scrivere il file {disp} (fuori dalla home)?"),
                ));
            }
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("creazione directory fallita: {e}"))?;
            }
            if p.is_dir() {
                return Err(format!("{disp} è una directory, non un file"));
            }
            std::fs::write(&p, &content)
                .map_err(|e| format!("scrittura di {disp} fallita: {e}"))?;
            Ok(ToolOutcome::Done(format!(
                "File scritto: {disp} ({} byte).",
                content.len()
            )))
        }
        "mkdir" => {
            if !under_home(&p) && !confirmed {
                return Ok(files_pending(
                    args,
                    format!("Creare la directory {disp} (fuori dalla home)?"),
                ));
            }
            std::fs::create_dir_all(&p).map_err(|e| format!("creazione di {disp} fallita: {e}"))?;
            Ok(ToolOutcome::Done(format!("Directory creata: {disp}.")))
        }
        "list" => {
            let entries =
                std::fs::read_dir(&p).map_err(|e| format!("lettura di {disp} fallita: {e}"))?;
            let mut names: Vec<String> = Vec::new();
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                let suffix = if e.path().is_dir() { "/" } else { "" };
                names.push(format!("{name}{suffix}"));
            }
            names.sort();
            if names.is_empty() {
                return Ok(ToolOutcome::Done(format!("{disp} è vuota.")));
            }
            let mut out = format!("{disp} ({} voci):\n", names.len());
            for n in names.iter().take(200) {
                out.push_str(&format!("- {n}\n"));
            }
            if names.len() > 200 {
                out.push_str("… [altre voci omesse]");
            }
            Ok(ToolOutcome::Done(out))
        }
        "search" => {
            let pattern = arg_str(args, "pattern")?;
            let hits = search_files(&p, &pattern)?;
            if hits.is_empty() {
                return Ok(ToolOutcome::Done(format!(
                    "Nessuna occorrenza di \"{pattern}\" in {disp}."
                )));
            }
            let mut out = format!("{} occorrenze di \"{pattern}\" in {disp}:\n", hits.len());
            for h in hits {
                out.push_str(&format!("- {h}\n"));
            }
            Ok(ToolOutcome::Done(out))
        }
        "move" => {
            let to = arg_str(args, "to")?;
            let dest = expand_path(&to);
            if (!under_home(&p) || !under_home(&dest)) && !confirmed {
                return Ok(files_pending(
                    args,
                    format!("Spostare {disp} in {} (fuori dalla home)?", dest.display()),
                ));
            }
            if dest.exists() {
                return Err(format!("la destinazione {} esiste già", dest.display()));
            }
            std::fs::rename(&p, &dest).map_err(|e| format!("spostamento fallito: {e}"))?;
            Ok(ToolOutcome::Done(format!(
                "Spostato: {disp} → {}.",
                dest.display()
            )))
        }
        "delete" => {
            if !confirmed {
                return Ok(files_pending(
                    args,
                    format!("Cancellare {disp}? (le directory non vuote non si cancellano)"),
                ));
            }
            if p.is_dir() {
                std::fs::remove_dir(&p).map_err(|e| {
                    format!("cancellazione di {disp} fallita (directory non vuota?): {e}")
                })?;
            } else {
                std::fs::remove_file(&p)
                    .map_err(|e| format!("cancellazione di {disp} fallita: {e}"))?;
            }
            Ok(ToolOutcome::Done(format!("Cancellato: {disp}.")))
        }
        other => Err(format!("azione file sconosciuta: {other}")),
    }
}

/// Recursive substring search. Skips hidden VCS/build dirs and files
/// over 256 KiB; caps visited files and hits.
fn search_files(root: &Path, pattern: &str) -> Result<Vec<String>, String> {
    if !root.exists() {
        return Err(format!("percorso inesistente: {}", root.display()));
    }
    let mut hits: Vec<String> = Vec::new();
    let mut visited = 0usize;
    let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if hits.len() >= FILES_SEARCH_MAX_HITS || visited >= FILES_SEARCH_MAX_FILES {
                return Ok(hits);
            }
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if path.is_dir() {
                if name == ".git" || name == "target" || name == "node_modules" {
                    continue;
                }
                if path.components().count() - root.components().count() > 12 {
                    continue;
                }
                stack.push(path);
                continue;
            }
            visited += 1;
            let Ok(meta) = entry.metadata() else { continue };
            if meta.len() > 262_144 {
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let Ok(text) = String::from_utf8(bytes) else {
                continue;
            };
            for (i, line) in text.lines().enumerate() {
                if line.contains(pattern) {
                    hits.push(format!("{}:{}: {}", path.display(), i + 1, line.trim()));
                    if hits.len() >= FILES_SEARCH_MAX_HITS {
                        break;
                    }
                }
            }
        }
    }
    Ok(hits)
}

// ---------------------------------------------------------------------------
// Screenshot
// ---------------------------------------------------------------------------

/// PNG dimensions from the IHDR chunk (bytes 16..24, big-endian).
fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24 || &bytes[0..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let w = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    Some((w, h))
}

/// Capture the whole screen and return (result text, base64 data URL).
/// `grim` (wlroots: Niri/Hyprland/MangoWC, half scale to keep the
/// request small) is tried first, then `gnome-screenshot` (GNOME).
/// Fails honestly when the model cannot see images or no capture tool
/// exists.
fn screenshot(ctx: &ToolContext) -> Result<(String, String), String> {
    if !ctx.vision_supported {
        return Err("il modello configurato non supporta la visione: lo screenshot non verrebbe visto. Usa un modello vision (es. gpt-4o-mini, un modello *-vision su Groq, Claude su OpenRouter) per attivarlo."
            .to_string());
    }
    let mut path = std::env::temp_dir();
    path.push(format!("nova-ai-shot-{}.png", std::process::id()));
    let target = path.to_str().unwrap_or("/tmp/nova-ai-shot.png").to_string();
    let mut last_err = String::new();
    let grim = Command::new("grim").args(["-s", "0.5", &target]).output();
    let captured = match grim {
        Ok(o) if o.status.success() => true,
        Ok(o) => {
            last_err = format!("grim: {}", String::from_utf8_lossy(&o.stderr).trim());
            false
        }
        Err(e) => {
            last_err = format!("grim non trovato: {e}");
            false
        }
    };
    if !captured {
        let shot = Command::new("gnome-screenshot")
            .args(["-f", &target])
            .output();
        match shot {
            Ok(o) if o.status.success() => {}
            Ok(o) => {
                let _ = std::fs::remove_file(&path);
                return Err(format!(
                    "nessuno strumento di cattura disponibile ({}; gnome-screenshot: {})",
                    last_err,
                    String::from_utf8_lossy(&o.stderr).trim()
                ));
            }
            Err(e) => {
                let _ = std::fs::remove_file(&path);
                return Err(format!(
                    "nessuno strumento di cattura disponibile ({last_err}; gnome-screenshot non trovato: {e})"
                ));
            }
        }
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("lettura screenshot fallita: {e}"))?;
    let _ = std::fs::remove_file(&path);
    if bytes.len() > 3_000_000 {
        return Err(format!(
            "screenshot troppo grande ({} byte): riduci la risoluzione dello schermo",
            bytes.len()
        ));
    }
    let dims = png_dimensions(&bytes)
        .map(|(w, h)| format!("{w}×{h}"))
        .unwrap_or_else(|| "?".to_string());
    let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes);
    Ok((
        format!("Screenshot catturato ({dims}, {} KiB).", bytes.len() / 1024),
        format!("data:image/png;base64,{b64}"),
    ))
}

// ---------------------------------------------------------------------------
// Windows (Niri / Hyprland)
// ---------------------------------------------------------------------------

/// One window, backend-agnostic.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowInfo {
    pub id: String,
    pub title: String,
    pub app: String,
    pub workspace: String,
    pub focused: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WindowBackend {
    Niri,
    Hyprland,
}

/// Detect the window backend: the desktop hint first, then a probe of
/// the compositor CLIs. GNOME/COSMIC have no reliable window CLI, so
/// they get an honest error.
fn detect_window_backend() -> Result<WindowBackend, String> {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .to_ascii_lowercase();
    if desktop.contains("niri") {
        return Ok(WindowBackend::Niri);
    }
    if desktop.contains("hyprland") {
        return Ok(WindowBackend::Hyprland);
    }
    // Probe: a working `niri msg` / `hyprctl` means that compositor
    // is running (fails fast otherwise — no socket, no session).
    if Command::new("niri")
        .args(["msg", "--json", "windows"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return Ok(WindowBackend::Niri);
    }
    if Command::new("hyprctl")
        .args(["clients", "-j"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return Ok(WindowBackend::Hyprland);
    }
    Err("gestione finestre supportata solo su Niri e Hyprland: su GNOME/COSMIC non c'è un CLI affidabile per elencare e controllare le finestre".to_string())
}

/// Parse `niri msg --json windows` output.
pub fn parse_niri_windows(json: &str) -> Result<Vec<WindowInfo>, String> {
    let v: Value =
        serde_json::from_str(json).map_err(|e| format!("output niri non valido: {e}"))?;
    let arr = v
        .as_array()
        .ok_or_else(|| "output niri non è un array".to_string())?;
    let mut out = Vec::new();
    for w in arr {
        out.push(WindowInfo {
            id: w
                .get("id")
                .and_then(|i| i.as_u64())
                .map(|i| i.to_string())
                .unwrap_or_default(),
            title: w
                .get("title")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string(),
            app: w
                .get("app_id")
                .and_then(|a| a.as_str())
                .unwrap_or("")
                .to_string(),
            workspace: w
                .get("workspace_id")
                .and_then(|i| i.as_u64())
                .map(|i| i.to_string())
                .unwrap_or_default(),
            focused: w
                .get("is_focused")
                .and_then(|f| f.as_bool())
                .unwrap_or(false),
        });
    }
    Ok(out)
}

/// Parse `hyprctl clients -j` output.
pub fn parse_hyprland_clients(json: &str) -> Result<Vec<WindowInfo>, String> {
    let v: Value =
        serde_json::from_str(json).map_err(|e| format!("output hyprctl non valido: {e}"))?;
    let arr = v
        .as_array()
        .ok_or_else(|| "output hyprctl non è un array".to_string())?;
    let mut out = Vec::new();
    for w in arr {
        out.push(WindowInfo {
            id: w
                .get("address")
                .and_then(|a| a.as_str())
                .unwrap_or("")
                .to_string(),
            title: w
                .get("title")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string(),
            app: w
                .get("class")
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .to_string(),
            workspace: w
                .get("workspace")
                .and_then(|ws| ws.get("name"))
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string(),
            focused: w.get("focused").and_then(|f| f.as_bool()).unwrap_or(false),
        });
    }
    Ok(out)
}

fn list_windows(backend: WindowBackend) -> Result<Vec<WindowInfo>, String> {
    match backend {
        WindowBackend::Niri => {
            let out = run_capture("niri", &["msg", "--json", "windows"])?;
            parse_niri_windows(&out)
        }
        WindowBackend::Hyprland => {
            let out = run_capture("hyprctl", &["clients", "-j"])?;
            parse_hyprland_clients(&out)
        }
    }
}

fn format_windows(windows: &[WindowInfo]) -> String {
    if windows.is_empty() {
        return "Nessuna finestra aperta.".to_string();
    }
    let mut out = format!("Finestre aperte ({}):\n", windows.len());
    for w in windows {
        out.push_str(&format!(
            "- [{}] {} ({}) — workspace {} {}\n",
            w.id,
            w.title,
            w.app,
            w.workspace,
            if w.focused { "[focus]" } else { "" }
        ));
    }
    out
}

fn execute_windows(args: &Value, confirmed: bool) -> Result<ToolOutcome, String> {
    let action = arg_str(args, "action")?;
    let backend = detect_window_backend()?;
    match action.as_str() {
        "list" => {
            let windows = list_windows(backend)?;
            Ok(ToolOutcome::Done(format_windows(&windows)))
        }
        "focus" => {
            let id = arg_str(args, "window_id")?;
            match backend {
                WindowBackend::Niri => {
                    run_capture("niri", &["msg", "action", "focus-window", "--id", &id])?;
                }
                WindowBackend::Hyprland => {
                    run_capture(
                        "hyprctl",
                        &["dispatch", "focuswindow", &format!("address:{id}")],
                    )?;
                }
            }
            Ok(ToolOutcome::Done(format!("Finestra {id} in primo piano.")))
        }
        "close" => {
            let id = arg_str(args, "window_id")?;
            if !confirmed {
                return Ok(ToolOutcome::NeedsConfirmation(PendingAction {
                    tool: "windows".to_string(),
                    args: args.clone(),
                    description: format!(
                        "Chiudere la finestra {id}? (il lavoro non salvato potrebbe andare perso)"
                    ),
                }));
            }
            match backend {
                WindowBackend::Niri => {
                    run_capture("niri", &["msg", "action", "close-window", "--id", &id])?;
                }
                WindowBackend::Hyprland => {
                    run_capture(
                        "hyprctl",
                        &["dispatch", "killwindow", &format!("address:{id}")],
                    )?;
                }
            }
            Ok(ToolOutcome::Done(format!("Finestra {id} chiusa.")))
        }
        "move" => {
            let id = arg_str(args, "window_id")?;
            let ws = arg_str(args, "workspace")?;
            match backend {
                WindowBackend::Niri => {
                    run_capture(
                        "niri",
                        &[
                            "msg",
                            "action",
                            "move-window-to-workspace",
                            &ws,
                            "--id",
                            &id,
                        ],
                    )?;
                }
                WindowBackend::Hyprland => {
                    run_capture(
                        "hyprctl",
                        &["dispatch", "movetoworkspace", &format!("{ws},address:{id}")],
                    )?;
                }
            }
            Ok(ToolOutcome::Done(format!(
                "Finestra {id} spostata sul workspace {ws}."
            )))
        }
        "workspace" => {
            let ws = arg_str(args, "workspace")?;
            match backend {
                WindowBackend::Niri => {
                    run_capture("niri", &["msg", "action", "focus-workspace", &ws])?;
                }
                WindowBackend::Hyprland => {
                    run_capture("hyprctl", &["dispatch", "workspace", &ws])?;
                }
            }
            Ok(ToolOutcome::Done(format!("Workspace {ws} attivo.")))
        }
        other => Err(format!("azione finestre sconosciuta: {other}")),
    }
}

// ---------------------------------------------------------------------------
// System
// ---------------------------------------------------------------------------

fn system_pending(args: &Value, description: String) -> ToolOutcome {
    ToolOutcome::NeedsConfirmation(PendingAction {
        tool: "system".to_string(),
        args: args.clone(),
        description,
    })
}

/// Check for OS updates: `bootc upgrade --check` first, `rpm-ostree
/// upgrade --check` as fallback. The output is interpreted
/// textually (exit codes differ between versions); when it cannot be
/// interpreted, the raw output is reported instead of guessing.
fn os_update_check() -> String {
    let try_cmd = |cmd: &str| -> Option<(bool, String)> {
        let out = Command::new(cmd)
            .args(["upgrade", "--check"])
            .output()
            .ok()?;
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        Some((out.status.success(), text))
    };
    let (cmd, result) = match try_cmd("bootc") {
        Some(r) => ("bootc", r),
        None => match try_cmd("rpm-ostree") {
            Some(r) => ("rpm-ostree", r),
            None => {
                return "Aggiornamenti OS non verificabili: né bootc né rpm-ostree sono disponibili su questo sistema.".to_string();
            }
        },
    };
    let (_, text) = result;
    let lower = text.to_ascii_lowercase();
    if lower.contains("update available") || lower.contains("updates available") {
        format!("Aggiornamento OS disponibile ({cmd}):\n{}", text.trim())
    } else if lower.contains("up to date")
        || lower.contains("no update")
        || lower.contains("no updates")
    {
        format!("Il sistema è aggiornato ({cmd}).")
    } else {
        format!(
            "Non riesco a interpretare l'output di {cmd} upgrade --check (exit: vedi sotto):\n{}",
            text.trim()
        )
    }
}

/// Apply the OS update via pkexec (system auth dialog). Long timeout:
/// downloads a whole new image.
fn os_update() -> Result<String, String> {
    let cmd = if Command::new("bootc")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        "bootc"
    } else if Command::new("rpm-ostree")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        "rpm-ostree"
    } else {
        return Err("aggiornamento OS non disponibile: né bootc né rpm-ostree trovati".to_string());
    };
    let out = Command::new("timeout")
        .args(["900", "pkexec", cmd, "upgrade"])
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "timeout/pkexec non trovato: l'aggiornamento richiede coreutils e polkit"
                    .to_string()
            } else {
                format!("errore eseguendo pkexec: {e}")
            }
        })?;
    // Timeout 900 s: l'upgrade scarica un'intera immagine e può
    // metterci minuti; oltre 900 s l'agente riporta il timeout invece
    // di restare bloccato (bootc/rpm-ostree sono transazionali).
    if out.status.success() {
        Ok(format!(
            "{cmd} upgrade completato. Riavvia per applicare la nuova immagine."
        ))
    } else {
        Err(format!(
            "{cmd} upgrade fallito: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

fn execute_system(args: &Value, confirmed: bool) -> Result<ToolOutcome, String> {
    let action = arg_str(args, "action")?;
    match action.as_str() {
        "os_update_check" => Ok(ToolOutcome::Done(os_update_check())),
        "os_update" => {
            if !confirmed {
                return Ok(system_pending(
                    args,
                    "Applicare l'aggiornamento del sistema (bootc upgrade)? Scarica la nuova immagine; serve un riavvio per applicarla.".to_string(),
                ));
            }
            os_update().map(ToolOutcome::Done)
        }
        "service" => {
            let name = arg_str(args, "name")?;
            let op = arg_str(args, "operation")?;
            match op.as_str() {
                "status" => {
                    let out = run_capture("systemctl", &["--user", "status", &name])?;
                    Ok(ToolOutcome::Done(truncate_output(&out, 8000)))
                }
                "start" | "stop" | "restart" => {
                    if !confirmed {
                        return Ok(system_pending(
                            args,
                            format!("Servizio utente {name}: {op}?"),
                        ));
                    }
                    run_capture("systemctl", &["--user", op.as_str(), name.as_str()])?;
                    Ok(ToolOutcome::Done(format!(
                        "Servizio {name}: {op} eseguito."
                    )))
                }
                other => Err(format!("operazione servizio sconosciuta: {other}")),
            }
        }
        "power_profile" => {
            let op = arg_str(args, "operation")?;
            match op.as_str() {
                "get" => {
                    let out = run_capture("powerprofilesctl", &["get"])?;
                    Ok(ToolOutcome::Done(format!("Profilo energetico: {out}.")))
                }
                "set" => {
                    let profile = arg_str(args, "value")?;
                    if !matches!(profile.as_str(), "performance" | "balanced" | "power-saver") {
                        return Err(
                            "profilo deve essere performance, balanced o power-saver".to_string()
                        );
                    }
                    if !confirmed {
                        return Ok(system_pending(
                            args,
                            format!("Impostare il profilo energetico su {profile}?"),
                        ));
                    }
                    run_capture("powerprofilesctl", &["set", &profile])?;
                    Ok(ToolOutcome::Done(format!(
                        "Profilo energetico impostato su {profile}."
                    )))
                }
                other => Err(format!("operazione profilo sconosciuta: {other}")),
            }
        }
        "bluetooth" => {
            let op = arg_str(args, "operation")?;
            match op.as_str() {
                "list" => {
                    let out = run_capture("bluetoothctl", &["devices"])?;
                    Ok(ToolOutcome::Done(if out.is_empty() {
                        "Nessun dispositivo Bluetooth associato.".to_string()
                    } else {
                        format!("Dispositivi Bluetooth:\n{out}")
                    }))
                }
                "power" => {
                    let value = arg_str(args, "value")?;
                    if value != "on" && value != "off" {
                        return Err("value deve essere on o off".to_string());
                    }
                    if !confirmed {
                        return Ok(system_pending(args, format!("Bluetooth: power {value}?")));
                    }
                    run_capture("bluetoothctl", &["power", &value])?;
                    Ok(ToolOutcome::Done(format!("Bluetooth {value}.")))
                }
                "connect" | "disconnect" => {
                    let device = arg_str(args, "name")?;
                    if !confirmed {
                        return Ok(system_pending(args, format!("Bluetooth: {op} {device}?")));
                    }
                    run_capture("bluetoothctl", &[op.as_str(), device.as_str()])?;
                    Ok(ToolOutcome::Done(format!(
                        "Bluetooth: {op} {device} eseguito."
                    )))
                }
                other => Err(format!("operazione bluetooth sconosciuta: {other}")),
            }
        }
        "audio" => {
            let op = arg_str(args, "operation")?;
            match op.as_str() {
                "list" => {
                    let out = run_capture("wpctl", &["status"])?;
                    Ok(ToolOutcome::Done(truncate_output(&out, 8000)))
                }
                "set_default" => {
                    let device = arg_str(args, "name")?;
                    if !confirmed {
                        return Ok(system_pending(
                            args,
                            format!("Impostare {device} come dispositivo audio predefinito?"),
                        ));
                    }
                    run_capture("wpctl", &["set-default", &device])?;
                    Ok(ToolOutcome::Done(format!(
                        "Dispositivo audio predefinito: {device}."
                    )))
                }
                other => Err(format!("operazione audio sconosciuta: {other}")),
            }
        }
        other => Err(format!("azione sistema sconosciuta: {other}")),
    }
}

// ---------------------------------------------------------------------------
// Distrobox + installazione pacchetti di sistema
// ---------------------------------------------------------------------------

fn distrobox_available() -> Result<(), String> {
    match Command::new("distrobox").arg("--version").output() {
        Ok(o) if o.status.success() => Ok(()),
        _ => Err("distrobox non è installato su questo sistema".to_string()),
    }
}

fn execute_distrobox(args: &Value, confirmed: bool) -> Result<ToolOutcome, String> {
    let action = arg_str(args, "action")?;
    distrobox_available()?;
    match action.as_str() {
        "list" => {
            let out = run_capture("distrobox", &["list"])?;
            Ok(ToolOutcome::Done(if out.is_empty() {
                "Nessun container Distrobox.".to_string()
            } else {
                format!("Container Distrobox:\n{out}")
            }))
        }
        "create" => {
            let name = arg_str(args, "name")?;
            let image = arg_str(args, "image")?;
            if !confirmed {
                return Ok(ToolOutcome::NeedsConfirmation(PendingAction {
                    tool: "distrobox".to_string(),
                    args: args.clone(),
                    description: format!(
                        "Creare il container Distrobox {name} dall'immagine {image}? (scarica l'immagine, può volerci un po')"
                    ),
                }));
            }
            let out = Command::new("timeout")
                .args(["600", "distrobox", "create", "-i", &image, "-n", &name])
                .output()
                .map_err(|e| format!("distrobox create fallito: {e}"))?;
            if out.status.success() {
                Ok(ToolOutcome::Done(format!(
                    "Container {name} creato da {image}. Entra con: distrobox enter {name} (o chiedimelo)."
                )))
            } else {
                Err(format!(
                    "creazione di {name} fallita: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                ))
            }
        }
        "run" => {
            let name = arg_str(args, "name")?;
            let command = arg_str(args, "command")?;
            match classify_shell_command(&command) {
                ShellClass::Denied => {
                    Err("comando rifiutato: comando catastrofico non consentito".to_string())
                }
                ShellClass::Mutating if !confirmed => {
                    Ok(ToolOutcome::NeedsConfirmation(PendingAction {
                        tool: "distrobox".to_string(),
                        args: args.clone(),
                        description: format!(
                            "Eseguire nel container {name} (modifica il container)?\n{command}"
                        ),
                    }))
                }
                _ => {
                    let out = Command::new("timeout")
                        .args([
                            "120",
                            "distrobox",
                            "run",
                            "-n",
                            &name,
                            "--",
                            "bash",
                            "-c",
                            &command,
                        ])
                        .output()
                        .map_err(|e| format!("distrobox run fallito: {e}"))?;
                    let code = out.status.code().unwrap_or(-1);
                    let mut s = String::from_utf8_lossy(&out.stdout).to_string();
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    if !stderr.trim().is_empty() {
                        s.push_str("\n[stderr]\n");
                        s.push_str(&stderr);
                    }
                    s.push_str(&format!("\n[exit code: {code}]"));
                    Ok(ToolOutcome::Done(truncate_output(&s, 24_000)))
                }
            }
        }
        "remove" => {
            let name = arg_str(args, "name")?;
            if !confirmed {
                return Ok(ToolOutcome::NeedsConfirmation(PendingAction {
                    tool: "distrobox".to_string(),
                    args: args.clone(),
                    description: format!(
                        "Eliminare il container Distrobox {name}? (tutto il suo contenuto va perso)"
                    ),
                }));
            }
            run_capture("distrobox", &["rm", "-f", &name])?;
            Ok(ToolOutcome::Done(format!("Container {name} eliminato.")))
        }
        other => Err(format!("azione distrobox sconosciuta: {other}")),
    }
}

/// Install a system RPM via rpm-ostree layering (pkexec auth dialog).
/// The caller already confirmed: layering slows updates and applies
/// on the next reboot.
fn rpm_ostree_install(package: &str) -> Result<String, String> {
    if package.is_empty()
        || package
            .chars()
            .any(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+')))
    {
        return Err(format!("nome pacchetto non valido: {package}"));
    }
    let out = Command::new("pkexec")
        .args(["rpm-ostree", "install", package])
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "pkexec non trovato: l'installazione richiede polkit".to_string()
            } else {
                format!("errore eseguendo pkexec: {e}")
            }
        })?;
    if out.status.success() {
        Ok(format!(
            "{package} installato come layer rpm-ostree. Riavvia per applicare la modifica."
        ))
    } else {
        Err(format!(
            "installazione di {package} fallita: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

// ---------------------------------------------------------------------------
// Web (ricerca, fetch, download) — senza chiave API
// ---------------------------------------------------------------------------

/// Minimal percent-encoding for query strings.
fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Decode the few HTML entities DuckDuckGo emits in titles/URLs.
fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
}

/// Parse DuckDuckGo HTML results: `<a ... class="result__a" href="URL">Title</a>`.
pub fn parse_ddg_results(html: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(pos) = rest.find("result__a") {
        rest = &rest[pos..];
        let Some(href_start) = rest.find("href=\"") else {
            break;
        };
        let href = &rest[href_start + 6..];
        let Some(href_end) = href.find('"') else {
            break;
        };
        let url = decode_entities(&href[..href_end]);
        let after_href = &href[href_end..];
        let Some(gt) = after_href.find('>') else {
            break;
        };
        let title_raw = &after_href[gt + 1..];
        let Some(end) = title_raw.find("</a>") else {
            break;
        };
        let title = strip_html_text(&title_raw[..end]);
        if !title.is_empty() && !url.is_empty() {
            out.push((title, url));
        }
        rest = &title_raw[end + 4..];
        if out.len() >= 8 {
            break;
        }
    }
    out
}

/// Strip <script>/<style> blocks and all tags; collapse whitespace.
pub fn strip_html_text(html: &str) -> String {
    let mut s = html.to_string();
    for tag in ["script", "style"] {
        while let Some(start) = s.to_ascii_lowercase().find(&format!("<{tag}")) {
            let Some(end) = s[start..].to_ascii_lowercase().find(&format!("</{tag}>")) else {
                break;
            };
            s.replace_range(start..start + end + tag.len() + 3, " ");
        }
    }
    let mut out = String::new();
    let mut in_tag = false;
    let mut tag_buf = String::new();
    for c in s.chars() {
        match c {
            '<' => {
                in_tag = true;
                tag_buf.clear();
            }
            '>' => {
                in_tag = false;
                // Block elements separate words: keep a space between
                // them (inline tags like <b> add none).
                let tag = tag_buf
                    .trim_start_matches('/')
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if matches!(
                    tag.as_str(),
                    "h1" | "h2"
                        | "h3"
                        | "h4"
                        | "h5"
                        | "h6"
                        | "p"
                        | "div"
                        | "li"
                        | "ul"
                        | "ol"
                        | "br"
                        | "tr"
                        | "table"
                        | "section"
                        | "article"
                        | "header"
                        | "footer"
                        | "hr"
                ) && !out.ends_with(char::is_whitespace)
                    && !out.is_empty()
                {
                    out.push(' ');
                }
            }
            _ if !in_tag => out.push(c),
            _ => tag_buf.push(c),
        }
    }
    let decoded = decode_entities(&out);
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn web_agent() -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(std::time::Duration::from_secs(30)))
        .build();
    ureq::Agent::new_with_config(config)
}

fn web_search(query: &str) -> Result<String, String> {
    let url = format!("https://html.duckduckgo.com/html/?q={}", url_encode(query));
    let resp = web_agent()
        .get(&url)
        .header("User-Agent", "Mozilla/5.0 (X11; Linux x86_64) NovaAI/2.0")
        .call()
        .map_err(|e| format!("ricerca fallita: {e}"))?;
    if resp.status().as_u16() != 200 {
        return Err(format!(
            "DuckDuckGo ha risposto HTTP {}",
            resp.status().as_u16()
        ));
    }
    let html = resp
        .into_body()
        .read_to_string()
        .map_err(|e| format!("lettura risultati fallita: {e}"))?;
    let results = parse_ddg_results(&html);
    if results.is_empty() {
        return Ok(format!("Nessun risultato per \"{query}\"."));
    }
    let mut out = format!("Risultati per \"{query}\":\n");
    for (i, (title, url)) in results.iter().enumerate() {
        out.push_str(&format!("{}. {title} — {url}\n", i + 1));
    }
    Ok(out)
}

fn web_fetch(url: &str) -> Result<String, String> {
    let resp = web_agent()
        .get(url)
        .header("User-Agent", "Mozilla/5.0 (X11; Linux x86_64) NovaAI/2.0")
        .call()
        .map_err(|e| format!("fetch di {url} fallito: {e}"))?;
    let status = resp.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(format!("{url} ha risposto HTTP {status}"));
    }
    let ctype = resp
        .headers()
        .get("Content-Type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let mut body = resp.into_body().into_reader();
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    use std::io::Read;
    loop {
        let n = body
            .read(&mut chunk)
            .map_err(|e| format!("lettura risposta fallita: {e}"))?;
        if n == 0 || buf.len() >= 102_400 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let text = String::from_utf8_lossy(&buf);
    let mut out = if ctype.contains("html") {
        strip_html_text(&text)
    } else {
        text.to_string()
    };
    if buf.len() >= 102_400 {
        out.push_str("\n… [pagina troncata a 100 KiB]");
    }
    if out.trim().is_empty() {
        return Err(format!("{url}: nessun testo leggibile"));
    }
    Ok(out)
}

fn web_download(url: &str, filename: Option<String>) -> Result<String, String> {
    let name = match filename {
        Some(f) => f,
        None => url
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or("download")
            .split('?')
            .next()
            .unwrap_or("download")
            .to_string(),
    };
    if name.contains('/') || name.contains('\\') || name == "." || name == ".." {
        return Err(format!("nome file non valido: {name}"));
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let dest = PathBuf::from(&home).join("Downloads").join(&name);
    let resp = web_agent()
        .get(url)
        .header("User-Agent", "Mozilla/5.0 (X11; Linux x86_64) NovaAI/2.0")
        .call()
        .map_err(|e| format!("download di {url} fallito: {e}"))?;
    let status = resp.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(format!("{url} ha risposto HTTP {status}"));
    }
    let mut reader = resp.into_body().into_reader();
    let mut file = std::fs::File::create(&dest)
        .map_err(|e| format!("creazione di {} fallita: {e}", dest.display()))?;
    use std::io::Read;
    let mut buf = [0u8; 32768];
    let mut total: u64 = 0;
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("download fallito: {e}"))?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > 52_428_800 {
            drop(file);
            let _ = std::fs::remove_file(&dest);
            return Err("download troppo grande (oltre 50 MB)".to_string());
        }
        use std::io::Write;
        file.write_all(&buf[..n])
            .map_err(|e| format!("scrittura download fallita: {e}"))?;
    }
    Ok(format!("Scaricato: {} ({} byte).", dest.display(), total))
}

fn execute_web(args: &Value) -> Result<ToolOutcome, String> {
    let action = arg_str(args, "action")?;
    match action.as_str() {
        "search" => {
            let query = arg_str(args, "query")?;
            web_search(&query).map(ToolOutcome::Done)
        }
        "fetch" => {
            let url = arg_str(args, "url")?;
            web_fetch(&url).map(ToolOutcome::Done)
        }
        "download" => {
            let url = arg_str(args, "url")?;
            let filename = arg_opt_str(args, "filename");
            web_download(&url, filename).map(ToolOutcome::Done)
        }
        other => Err(format!("azione web sconosciuta: {other}")),
    }
}

// ---------------------------------------------------------------------------
// Memory / plans / calendar / contacts / browser (v3)
// ---------------------------------------------------------------------------

fn execute_memory(args: &Value, ctx: &ToolContext) -> Result<String, String> {
    if !ctx.memory_enabled {
        return Err(
            "la memoria è disattivata: attivala nelle impostazioni per ricordare fatti".to_string(),
        );
    }
    let action = arg_str(args, "action")?;
    let mem = crate::memory::Memory::new(turn_data_dir(ctx).join("memory"));
    match action.as_str() {
        "remember" => {
            let text = arg_str(args, "text")?;
            let fact = mem.remember(&text)?;
            Ok(format!("Ricordato: {}", fact.text))
        }
        "recall" => {
            let query = arg_opt_str(args, "text").unwrap_or_default();
            let hits = mem.recall(&query, 10);
            if hits.is_empty() {
                Ok("Non ricordo niente del genere.".to_string())
            } else {
                let mut out = String::from("Ricordo:");
                for f in hits {
                    out.push_str(&format!("\n- [{}] {}", f.date, f.text));
                }
                Ok(out)
            }
        }
        "forget" => {
            let query = arg_str(args, "text")?;
            let removed = mem.forget(&query)?;
            if removed == 0 {
                Ok("Niente da dimenticare: nessun fatto corrisponde.".to_string())
            } else {
                Ok(format!("Dimenticato: {removed} fatto/i rimosso/i."))
            }
        }
        other => Err(format!("azione memory sconosciuta: {other}")),
    }
}

fn parse_plan_steps(args: &Value) -> Vec<String> {
    match args.get("steps") {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect(),
        Some(Value::String(s)) => s
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

fn execute_plan(args: &Value, ctx: &ToolContext) -> Result<String, String> {
    let action = arg_str(args, "action")?;
    let store = crate::plans::PlanStore::new(turn_data_dir(ctx).join("plans"));
    match action.as_str() {
        "create" => {
            let title = arg_str(args, "title")?;
            let steps = parse_plan_steps(args);
            let plan = store.create(&title, &steps)?;
            Ok(format!(
                "Piano creato: {}\n{}",
                plan.id,
                plan.progress_line()
            ))
        }
        "list" => {
            let plans = store.list();
            if plans.is_empty() {
                Ok("Nessun piano.".to_string())
            } else {
                let mut out = String::from("Piani:");
                for p in plans {
                    out.push_str(&format!("\n- [{}] {}", p.id, p.progress_line()));
                }
                Ok(out)
            }
        }
        "show" => {
            let id = arg_str(args, "id")?;
            let plan = store.load(&id)?;
            let mut out = plan.progress_line();
            for (i, s) in plan.steps.iter().enumerate() {
                out.push_str(&format!(
                    "\n  {i}. {} {}",
                    if s.done { "✓" } else { "○" },
                    s.text
                ));
            }
            Ok(out)
        }
        "update" => {
            let id = arg_str(args, "id")?;
            let step = args
                .get("step")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| "serve l'indice del passo (step)".to_string())?;
            let done = args
                .get("done")
                .and_then(|v| v.as_bool())
                .ok_or_else(|| "serve done (true/false)".to_string())?;
            if step < 0 {
                return Err("l'indice del passo non può essere negativo".to_string());
            }
            let plan = store.set_step(&id, step as usize, done)?;
            Ok(plan.progress_line())
        }
        "complete" => {
            let id = arg_str(args, "id")?;
            let plan = store.complete(&id)?;
            Ok(format!("Piano completato: {}", plan.progress_line()))
        }
        "delete" => {
            let id = arg_str(args, "id")?;
            store.delete(&id)?;
            Ok("Piano cancellato.".to_string())
        }
        other => Err(format!("azione plan sconosciuta: {other}")),
    }
}

fn execute_calendar(args: &Value, ctx: &ToolContext) -> Result<String, String> {
    let action = arg_str(args, "action")?;
    let cal = crate::calendar::Calendar::new(
        ctx.calendar_url.clone(),
        ctx.calendar_file.clone(),
        turn_data_dir(ctx).join("calendar.ics"),
    );
    let (events, note) = match action.as_str() {
        "today" => cal.events_today()?,
        "tomorrow" => cal.events_tomorrow()?,
        "date" => {
            let d = arg_str(args, "date")?;
            let date = crate::civil::parse_date(&d)
                .ok_or_else(|| "data non valida: usa YYYY-MM-DD".to_string())?;
            cal.events_on(date)?
        }
        "search" => {
            let q = arg_str(args, "query")?;
            let (hits, note) = cal.search(&q)?;
            if hits.is_empty() {
                return Ok("Nessun evento trovato.".to_string());
            }
            let mut out = String::from("Eventi trovati:");
            for e in hits {
                out.push_str(&format!(
                    "\n- {} {}",
                    crate::civil::format_date(e.start.0, e.start.1, e.start.2),
                    e.line()
                ));
            }
            if let Some(n) = note {
                out.push_str(&format!("\n({n})"));
            }
            return Ok(out);
        }
        other => return Err(format!("azione calendar sconosciuta: {other}")),
    };
    let label = match action.as_str() {
        "today" => "oggi".to_string(),
        "tomorrow" => "domani".to_string(),
        "date" => arg_str(args, "date").unwrap_or_default(),
        _ => String::new(),
    };
    let mut out = crate::calendar::format_events(&label, &events);
    if let Some(n) = note {
        out.push_str(&format!("\n({n})"));
    }
    Ok(out)
}

fn execute_contacts(args: &Value, ctx: &ToolContext) -> Result<String, String> {
    let action = arg_str(args, "action")?;
    if action != "search" {
        return Err(format!("azione contacts sconosciuta: {action}"));
    }
    let query = arg_opt_str(args, "query").unwrap_or_default();
    let path = turn_data_dir(ctx).join("contacts.vcf");
    let (all, exists) = crate::contacts::load_from(&path);
    if !exists {
        return Err(format!(
            "nessun contatto: crea il file {} con le tue schede vCard (FN / EMAIL / TEL)",
            path.display()
        ));
    }
    let hits = crate::contacts::search(&all, &query);
    if hits.is_empty() {
        Ok("Nessun contatto trovato.".to_string())
    } else {
        let mut out = String::from("Contatti:");
        for c in hits {
            out.push_str(&format!("\n- {}", c.line()));
        }
        Ok(out)
    }
}

fn execute_browser(args: &Value, ctx: &ToolContext) -> Result<ToolOutcome, String> {
    let action = arg_str(args, "action")?;
    match action.as_str() {
        "navigate" => {
            let url = arg_str(args, "url")?;
            crate::browser::browser_navigate(&url).map(ToolOutcome::Done)
        }
        "find" => {
            let selector = arg_str(args, "selector")?;
            crate::browser::browser_find(&selector).map(ToolOutcome::Done)
        }
        "click" => {
            let selector = arg_str(args, "selector")?;
            Ok(ToolOutcome::NeedsConfirmation(PendingAction {
                tool: "browser".to_string(),
                args: json!({ "op": "click", "selector": selector }),
                description: format!("Cliccare su {selector} nel browser?"),
            }))
        }
        "type" => {
            let selector = arg_str(args, "selector")?;
            let text = arg_str(args, "text")?;
            Ok(ToolOutcome::NeedsConfirmation(PendingAction {
                tool: "browser".to_string(),
                args: json!({ "op": "type", "selector": selector, "text": text }),
                description: format!("Scrivere \"{text}\" in {selector} nel browser?"),
            }))
        }
        "text" => crate::browser::browser_text().map(ToolOutcome::Done),
        "screenshot" => {
            if !ctx.vision_supported {
                return Err(
                    "lo screenshot del browser serve solo con un modello che supporta la visione"
                        .to_string(),
                );
            }
            let b64 = crate::browser::browser_screenshot_b64()?;
            Ok(ToolOutcome::DoneWithImage {
                text: "Ecco la pagina del browser:".to_string(),
                image_data_url: format!("data:image/png;base64,{b64}"),
            })
        }
        "close" => crate::browser::browser_close().map(ToolOutcome::Done),
        other => Err(format!("azione browser sconosciuta: {other}")),
    }
}

// ---------------------------------------------------------------------------
// System info
// ---------------------------------------------------------------------------

fn system_info() -> String {
    let mut parts: Vec<String> = Vec::new();
    // OS from /usr/lib/os-release (PRETTY_NAME).
    if let Ok(text) = std::fs::read_to_string("/usr/lib/os-release") {
        for line in text.lines() {
            if let Some(v) = line.strip_prefix("PRETTY_NAME=") {
                parts.push(format!("OS: {}", v.trim_matches('"')));
                break;
            }
        }
    }
    let desktop = std::env::var("XDG_CURRENT_DESKTOP")
        .or_else(|_| std::env::var("DESKTOP_SESSION"))
        .unwrap_or_else(|_| "sconosciuto".to_string());
    parts.push(format!("Desktop: {desktop}"));
    if let Ok(up) = std::fs::read_to_string("/proc/uptime")
        && let Some(secs) = up
            .split_whitespace()
            .next()
            .and_then(|s| s.parse::<f64>().ok())
    {
        let h = (secs as i64) / 3600;
        let m = ((secs as i64) % 3600) / 60;
        parts.push(format!("Uptime: {h}h {m}m"));
    }
    match run_capture("df", &["--output=avail", "-B1", "/"]) {
        Ok(out) => {
            if let Some(bytes) = out
                .lines()
                .nth(1)
                .and_then(|l| l.trim().parse::<i64>().ok())
            {
                parts.push(format!("Spazio libero su /: {:.1} GB", bytes as f64 / 1e9));
            }
        }
        Err(_) => parts.push("Spazio libero: non rilevabile".to_string()),
    }
    parts.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_desktop_basic() {
        let text = "[Desktop Entry]\nType=Application\nName=Firefox\nExec=firefox %u\nIcon=firefox\nCategories=Network;WebBrowser;\n";
        let app =
            parse_desktop_text(text, Path::new("/usr/share/applications/firefox.desktop")).unwrap();
        assert_eq!(app.name, "Firefox");
        assert_eq!(app.id, "firefox");
        assert_eq!(app.exec, "firefox %u");
        assert!(!app.flatpak);
    }

    #[test]
    fn parse_desktop_skips_hidden_and_non_apps() {
        let hidden = "[Desktop Entry]\nType=Application\nName=X\nExec=x\nNoDisplay=true\n";
        assert!(
            parse_desktop_text(hidden, Path::new("/usr/share/applications/x.desktop")).is_none()
        );
        let link = "[Desktop Entry]\nType=Link\nName=X\nURL=https://example.org\n";
        assert!(parse_desktop_text(link, Path::new("/usr/share/applications/x.desktop")).is_none());
        let no_name = "[Desktop Entry]\nType=Application\nExec=x\n";
        assert!(
            parse_desktop_text(no_name, Path::new("/usr/share/applications/x.desktop")).is_none()
        );
    }

    #[test]
    fn parse_desktop_flatpak_detected() {
        let text = "[Desktop Entry]\nType=Application\nName=Spotify\nExec=flatpak run com.spotify.Client\n";
        let app = parse_desktop_text(
            text,
            Path::new("/var/lib/flatpak/exports/share/applications/com.spotify.Client.desktop"),
        )
        .unwrap();
        assert!(app.flatpak);
        assert_eq!(app.id, "com.spotify.Client");
    }

    #[test]
    fn strip_codes() {
        assert_eq!(
            strip_exec_codes("firefox %u --new-window"),
            "firefox  --new-window"
        );
        assert_eq!(strip_exec_codes("app %F %i %c"), "app");
    }

    #[test]
    fn arg_validation() {
        assert!(arg_level(&json!({ "level": 150 }), "level").is_err());
        assert!(arg_level(&json!({ "level": -1 }), "level").is_err());
        assert_eq!(arg_level(&json!({ "level": 42 }), "level").unwrap(), 42);
        assert!(arg_str(&json!({}), "query").is_err());
        assert!(arg_bool(&json!({ "enabled": "yes" }), "enabled").is_err());
    }

    #[test]
    fn mutating_tools_need_confirmation() {
        for name in [
            "install_app",
            "remove_app",
            "set_volume",
            "set_brightness",
            "set_wifi",
            "set_theme",
            "install_system_package",
        ] {
            assert!(tool_needs_confirmation(name), "{name}");
        }
        for name in [
            "list_apps",
            "open_app",
            "get_volume",
            "system_info",
            "shell",
            "files",
            "windows",
            "distrobox",
            "system",
        ] {
            // These decide per-action / per-command, not per-tool.
            assert!(!tool_needs_confirmation(name), "{name}");
        }
    }

    #[test]
    fn set_volume_returns_pending_not_execution() {
        // No wpctl on this VM — the point is that execute_tool does NOT
        // run the command: it returns NeedsConfirmation.
        let out = execute_tool(
            "set_volume",
            &json!({ "level": 30 }),
            &ToolContext::default(),
        )
        .unwrap();
        match out {
            ToolOutcome::NeedsConfirmation(p) => {
                assert_eq!(p.tool, "set_volume");
                assert_eq!(p.args["level"], 30);
            }
            other => panic!("expected NeedsConfirmation, got {other:?}"),
        }
    }

    #[test]
    fn set_theme_bad_mode_errors() {
        assert!(
            execute_tool(
                "set_theme",
                &json!({ "mode": "blue" }),
                &ToolContext::default()
            )
            .is_err()
        );
    }

    #[test]
    fn unknown_tool_errors() {
        assert!(execute_tool("format_disk", &json!({}), &ToolContext::default()).is_err());
    }

    #[test]
    fn system_info_has_os_and_desktop() {
        let info = system_info();
        assert!(info.contains("OS:") || info.contains("Desktop:"));
    }

    #[test]
    fn confirmed_volume_without_wpctl_fails_honestly() {
        // On this VM wpctl is absent: executing must fail with a clear
        // message, not a fake success.
        let action = PendingAction {
            tool: "set_volume".to_string(),
            args: json!({ "level": 10 }),
            description: "test".to_string(),
        };
        let err = execute_confirmed(&action).unwrap_err();
        assert!(
            err.contains("wpctl") || err.contains("non trovato"),
            "{err}"
        );
    }

    // --- shell classification -------------------------------------------------

    #[test]
    fn shell_denied_patterns() {
        for cmd in [
            "rm -rf /",
            "rm -rf ~",
            "rm -rf $HOME",
            "rm -rf /usr",
            "sudo rm -rf /etc",
            // Wrappers must not hide the dangerous verb.
            "sudo -u root rm -rf /",
            "sudo -n rm -rf /var",
            "sudo --user=root rm -rf /",
            "pkexec rm -rf /boot",
            "env FOO=1 rm -rf ~",
            "env -i rm -rf /",
            "command rm -rf /etc",
            "xargs rm -rf /",
            "nice rm -rf /usr",
            "timeout 5 rm -rf /",
            "mkfs.ext4 /dev/sda1",
            "dd if=/dev/zero of=/dev/sda",
            ":(){:|:&};:",
            "echo ciao > /dev/sda",
            "shred -n 1 /dev/sda",
            "ls /tmp | rm -rf /",
        ] {
            assert_eq!(classify_shell_command(cmd), ShellClass::Denied, "{cmd}");
        }
        // Quoted text must not trigger a denial.
        assert_eq!(
            classify_shell_command("echo 'rm -rf /'"),
            ShellClass::ReadOnly
        );
    }

    #[test]
    fn shell_mutating_patterns() {
        for cmd in [
            "rm file.txt",
            "mkdir nuova",
            "cp a.txt b.txt",
            "systemctl --user restart foo.service",
            "flatpak install flathub org.foo.Bar",
            "echo ciao > nota.txt",
            "ls | tee out.txt",
            "git status",
            "curl https://example.org",
            // Wrapped mutating commands still need confirmation.
            "env rm -rf /tmp/x",
            "find /tmp -delete",
            "find . -exec rm {} \\;",
            "sudo -u root ls /tmp",
        ] {
            assert_eq!(classify_shell_command(cmd), ShellClass::Mutating, "{cmd}");
        }
    }

    #[test]
    fn shell_readonly_patterns() {
        for cmd in [
            "ls -la /tmp",
            "cat /etc/os-release",
            "df -h",
            "echo ciao | grep ciao",
            "uname -a && whoami",
            // Wrappers around harmless reads stay reads.
            "env ls /tmp",
            "command -v ls",
        ] {
            assert_eq!(classify_shell_command(cmd), ShellClass::ReadOnly, "{cmd}");
        }
    }

    #[test]
    fn shell_tool_roundtrip() {
        let ctx = ToolContext::default();
        let out = execute_tool("shell", &json!({ "command": "echo ciao" }), &ctx).unwrap();
        match out {
            ToolOutcome::Done(s) => assert!(s.contains("ciao"), "{s}"),
            other => panic!("expected Done, got {other:?}"),
        }
        let denied = execute_tool("shell", &json!({ "command": "rm -rf /" }), &ctx);
        assert!(denied.is_err());
        let pending = execute_tool("shell", &json!({ "command": "rm nota.txt" }), &ctx).unwrap();
        assert!(matches!(pending, ToolOutcome::NeedsConfirmation(_)));
    }

    // --- sandbox (bubblewrap) ------------------------------------------------

    #[test]
    fn sandbox_argv_shape() {
        // argv shape must match bwrap availability exactly.
        assert_eq!(sandbox_argv(true).is_some(), bwrap_available());
        if let Some(argv) = sandbox_argv(true) {
            assert!(argv.contains(&"--ro-bind".to_string()));
            assert!(argv.contains(&"--tmpfs".to_string()));
            assert!(argv.contains(&"--unshare-net".to_string()));
            assert!(argv.contains(&"--".to_string()));
            let argv_no_net = sandbox_argv(false).unwrap();
            assert!(!argv_no_net.contains(&"--unshare-net".to_string()));
        }
    }

    #[test]
    fn sandbox_readonly_command_runs_and_cannot_write_home() {
        if !bwrap_available() {
            return; // sandbox not testable here; the missing-bwrap note path is covered by run_shell
        }
        let ctx = ToolContext::default();
        let out = execute_tool("shell", &json!({ "command": "echo sandbox-prova" }), &ctx).unwrap();
        match out {
            ToolOutcome::Done(s) => assert!(s.contains("sandbox-prova"), "{s}"),
            other => panic!("expected Done, got {other:?}"),
        }
        // A write into $HOME must fail inside the sandbox. `touch`
        // is classified Mutating (correctly), so it would stop at
        // confirmation; test the sandbox itself via run_shell, which
        // is what the ReadOnly path calls with sandboxed=true.
        let probe = format!("nova-ai-sandbox-probe-{}", std::process::id());
        let home = std::env::var("HOME").unwrap_or_default();
        let probe_path = PathBuf::from(&home).join(&probe);
        let _ = std::fs::remove_file(&probe_path);
        let s = run_shell(&format!("touch ~/{probe}"), 30, true).unwrap();
        assert!(
            s.contains("Read-only file system") || s.contains("exit code: 1"),
            "{s}"
        );
        assert!(!probe_path.exists(), "sandbox let the command write home!");
        let _ = std::fs::remove_file(&probe_path);
    }

    // --- v3 tools -------------------------------------------------------------

    fn v3_ctx(dir: &Path) -> ToolContext {
        ToolContext {
            vision_supported: true,
            calendar_url: String::new(),
            calendar_file: String::new(),
            memory_enabled: true,
            data_dir: Some(dir.to_path_buf()),
        }
    }

    fn v3_tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("nova-ai-v3-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn memory_tool_roundtrip() {
        let dir = v3_tmp("memory");
        let ctx = v3_ctx(&dir);
        let out = execute_tool(
            "memory",
            &json!({ "action": "remember", "text": "Il cane si chiama Rex" }),
            &ctx,
        )
        .unwrap();
        match out {
            ToolOutcome::Done(s) => assert!(s.contains("Rex"), "{s}"),
            other => panic!("expected Done, got {other:?}"),
        }
        let out = execute_tool(
            "memory",
            &json!({ "action": "recall", "text": "cane" }),
            &ctx,
        )
        .unwrap();
        match out {
            ToolOutcome::Done(s) => assert!(s.contains("Rex"), "{s}"),
            other => panic!("expected Done, got {other:?}"),
        }
        let out = execute_tool(
            "memory",
            &json!({ "action": "forget", "text": "cane" }),
            &ctx,
        )
        .unwrap();
        match out {
            ToolOutcome::Done(s) => assert!(s.contains("1"), "{s}"),
            other => panic!("expected Done, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn memory_tool_disabled_is_honest() {
        let dir = v3_tmp("memory-off");
        let mut ctx = v3_ctx(&dir);
        ctx.memory_enabled = false;
        let err = execute_tool("memory", &json!({ "action": "recall", "text": "" }), &ctx);
        assert!(err.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn plan_tool_roundtrip() {
        let dir = v3_tmp("plan");
        let ctx = v3_ctx(&dir);
        let out = execute_tool(
            "plan",
            &json!({ "action": "create", "title": "Pulire foto", "steps": ["Esporta", "Controlla"] }),
            &ctx,
        )
        .unwrap();
        let plan_id = match out {
            ToolOutcome::Done(s) => {
                assert!(s.contains("0/2"), "{s}");
                s.lines()
                    .next()
                    .unwrap()
                    .trim_start_matches("Piano creato: ")
                    .trim()
                    .to_string()
            }
            other => panic!("expected Done, got {other:?}"),
        };
        let out = execute_tool(
            "plan",
            &json!({ "action": "update", "id": plan_id, "step": 0, "done": true }),
            &ctx,
        )
        .unwrap();
        match out {
            ToolOutcome::Done(s) => assert!(s.contains("1/2"), "{s}"),
            other => panic!("expected Done, got {other:?}"),
        }
        let out = execute_tool("plan", &json!({ "action": "list" }), &ctx).unwrap();
        match out {
            ToolOutcome::Done(s) => assert!(s.contains("Pulire foto"), "{s}"),
            other => panic!("expected Done, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn calendar_tool_from_file() {
        let dir = v3_tmp("calendar");
        let ics = dir.join("cal.ics");
        std::fs::write(
            &ics,
            "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nDTSTART;TZID=Europe/Rome:20261127T090000\r\nDTEND;TZID=Europe/Rome:20261127T100000\r\nSUMMARY:Lezione di inglese\r\nLOCATION:Aula 3\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        )
        .unwrap();
        let mut ctx = v3_ctx(&dir);
        ctx.calendar_file = ics.to_string_lossy().to_string();
        let out = execute_tool(
            "calendar",
            &json!({ "action": "date", "date": "2026-11-27" }),
            &ctx,
        )
        .unwrap();
        match out {
            ToolOutcome::Done(s) => assert!(s.contains("Lezione di inglese"), "{s}"),
            other => panic!("expected Done, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn contacts_tool_from_file() {
        let dir = v3_tmp("contacts");
        std::fs::write(
            dir.join("contacts.vcf"),
            "BEGIN:VCARD\r\nFN:Fabiana Spoto\r\nEMAIL:fabiana@example.it\r\nEND:VCARD\r\n",
        )
        .unwrap();
        let ctx = v3_ctx(&dir);
        let out = execute_tool(
            "contacts",
            &json!({ "action": "search", "query": "fabiana" }),
            &ctx,
        )
        .unwrap();
        match out {
            ToolOutcome::Done(s) => assert!(s.contains("fabiana@example.it"), "{s}"),
            other => panic!("expected Done, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn browser_tool_against_mock() {
        let _lock = crate::browser::test_support::TEST_BASE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let base = crate::browser::test_support::start_mock_wd();
        crate::browser::set_test_base(Some(base));
        let ctx = ToolContext {
            vision_supported: true,
            ..ToolContext::default()
        };
        let out = execute_tool(
            "browser",
            &json!({ "action": "navigate", "url": "https://example.org" }),
            &ctx,
        )
        .unwrap();
        match out {
            ToolOutcome::Done(s) => assert!(s.contains("Pagina di prova"), "{s}"),
            other => panic!("expected Done, got {other:?}"),
        }
        // Click and type need confirmation (mutating browser ops).
        let out = execute_tool(
            "browser",
            &json!({ "action": "click", "selector": "button" }),
            &ctx,
        )
        .unwrap();
        match out {
            ToolOutcome::NeedsConfirmation(a) => {
                assert_eq!(a.tool, "browser");
                let done = execute_confirmed(&a).unwrap();
                assert!(done.contains("Cliccato"), "{done}");
            }
            other => panic!("expected NeedsConfirmation, got {other:?}"),
        }
        let out = execute_tool("browser", &json!({ "action": "screenshot" }), &ctx).unwrap();
        match out {
            ToolOutcome::DoneWithImage {
                text,
                image_data_url,
            } => {
                assert!(text.contains("browser"));
                assert!(image_data_url.starts_with("data:image/png;base64,iVBORw0KGgo"));
            }
            other => panic!("expected DoneWithImage, got {other:?}"),
        }
        let out = execute_tool("browser", &json!({ "action": "close" }), &ctx).unwrap();
        match out {
            ToolOutcome::Done(s) => assert!(s.contains("chiuso"), "{s}"),
            other => panic!("expected Done, got {other:?}"),
        }
        crate::browser::set_test_base(None);
    }

    // --- files ------------------------------------------------------------------

    #[test]
    fn files_roundtrip_on_temp_dir() {
        // Absolute paths under /tmp (outside $HOME): writes/deletes go
        // through the confirmation flow, reads/searches run directly.
        let tmp = std::env::temp_dir().join(format!("nova-ai-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let file = tmp.join("prova.txt");
        let ctx = ToolContext::default();
        // Write needs confirmation for paths outside ~: execute it
        // confirmed, as the agent loop would after the user's ok.
        let write = execute_confirmed(&PendingAction {
            tool: "files".to_string(),
            args: json!({
                "action": "write",
                "path": file.to_str().unwrap(),
                "content": "ciao mondo\n",
            }),
            description: "test".to_string(),
        });
        assert!(write.is_ok(), "{write:?}");
        let read = execute_tool(
            "files",
            &json!({ "action": "read", "path": file.to_str().unwrap() }),
            &ctx,
        )
        .unwrap();
        match read {
            ToolOutcome::Done(s) => assert!(s.contains("ciao mondo"), "{s}"),
            other => panic!("expected Done, got {other:?}"),
        }
        let search = execute_tool(
            "files",
            &json!({ "action": "search", "path": tmp.to_str().unwrap(), "pattern": "mondo" }),
            &ctx,
        )
        .unwrap();
        match search {
            ToolOutcome::Done(s) => assert!(s.contains("prova.txt"), "{s}"),
            other => panic!("expected Done, got {other:?}"),
        }
        // Delete always asks first.
        let del = execute_tool(
            "files",
            &json!({ "action": "delete", "path": file.to_str().unwrap() }),
            &ctx,
        )
        .unwrap();
        assert!(matches!(del, ToolOutcome::NeedsConfirmation(_)));
        // Confirmed delete removes the file.
        let action = PendingAction {
            tool: "files".to_string(),
            args: json!({ "action": "delete", "path": file.to_str().unwrap() }),
            description: "test".to_string(),
        };
        assert!(execute_confirmed(&action).is_ok());
        assert!(!file.exists());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // --- windows parsing ---------------------------------------------------------

    #[test]
    fn parse_niri_windows_fixture() {
        let json = r#"[{"id": 1, "title": "Terminal", "app_id": "org.gnome.Console", "workspace_id": 2, "is_focused": true}, {"id": 3, "title": "Firefox", "app_id": "firefox", "workspace_id": 1, "is_focused": false}]"#;
        let windows = parse_niri_windows(json).unwrap();
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].title, "Terminal");
        assert!(windows[0].focused);
        assert_eq!(windows[1].app, "firefox");
        assert_eq!(windows[1].workspace, "1");
    }

    #[test]
    fn parse_hyprland_clients_fixture() {
        let json = r#"[{"address": "0xabc", "title": "Code", "class": "code", "workspace": {"name": "1"}, "focused": true}]"#;
        let windows = parse_hyprland_clients(json).unwrap();
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].id, "0xabc");
        assert_eq!(windows[0].app, "code");
        assert!(windows[0].focused);
    }

    // --- web parsing -------------------------------------------------------------

    #[test]
    fn parse_ddg_fixture() {
        let html = r#"<a rel="nofollow" class="result__a" href="https://example.org/a">Primo &amp; titolo</a><a rel="nofollow" class="result__a" href="https://example.org/b">Secondo</a>"#;
        let results = parse_ddg_results(html);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "Primo & titolo");
        assert_eq!(results[0].1, "https://example.org/a");
    }

    #[test]
    fn strip_html_text_fixture() {
        let html = "<html><head><style>a{color:red}</style><script>var x=1;</script></head><body><h1>Titolo</h1><p>Testo <b>grassetto</b>.</p></body></html>";
        assert_eq!(strip_html_text(html), "Titolo Testo grassetto.");
    }

    // --- registry -----------------------------------------------------------------

    #[test]
    fn registry_contains_all_tools() {
        let defs = tool_definitions();
        assert!(defs.len() >= 16, "got {} defs", defs.len());
        for name in [
            "list_apps",
            "shell",
            "files",
            "screenshot",
            "windows",
            "system",
            "distrobox",
            "install_system_package",
            "web",
        ] {
            assert!(
                defs.iter().any(|d| d.name == name),
                "registry missing {name}"
            );
            let s = tool_schema(name).unwrap();
            assert!(s["type"] == "object", "{name}");
        }
        assert!(tool_schema("nope").is_none());
    }
}
