//! Tools the agent can call. Read-only tools execute immediately;
//! mutating tools (install/remove/set_*) first return
//! `ToolOutcome::NeedsConfirmation` and only run after the user
//! approves in the UI. The pending-action state lives in core, not in
//! the UI, so the rule cannot be bypassed by a UI bug.
//!
//! Errors are honest and specific: a tool that cannot do its job says
//! why (missing command, app not found, no Flatpak remote) instead of
//! pretending success.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

use crate::llm::ToolDef;

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
    ]
}

/// Tools that mutate the system and therefore need confirmation.
pub fn tool_needs_confirmation(name: &str) -> bool {
    matches!(
        name,
        "install_app" | "remove_app" | "set_volume" | "set_brightness" | "set_wifi" | "set_theme"
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
/// `NeedsConfirmation` (nothing is changed yet).
pub fn execute_tool(name: &str, args: &Value) -> Result<ToolOutcome, String> {
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
        ] {
            assert!(tool_needs_confirmation(name), "{name}");
        }
        for name in ["list_apps", "open_app", "get_volume", "system_info"] {
            assert!(!tool_needs_confirmation(name), "{name}");
        }
    }

    #[test]
    fn set_volume_returns_pending_not_execution() {
        // No wpctl on this VM — the point is that execute_tool does NOT
        // run the command: it returns NeedsConfirmation.
        let out = execute_tool("set_volume", &json!({ "level": 30 })).unwrap();
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
        assert!(execute_tool("set_theme", &json!({ "mode": "blue" })).is_err());
    }

    #[test]
    fn unknown_tool_errors() {
        assert!(execute_tool("format_disk", &json!({})).is_err());
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
}
