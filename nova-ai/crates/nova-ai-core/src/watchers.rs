//! Proactive watchers: a `nova-ai --check` run looks at the system
//! and sends a desktop notification when something needs attention:
//! OS updates available, low battery, disk almost full. The checks
//! run on demand or hourly via a user systemd timer
//! (`nova-ai-watch.timer`), installed from the settings.
//!
//! The threshold logic is pure (string in -> alert out) so it is
//! testable with fixtures; the command runs and notify-send are the
//! thin impure shell around it.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// Battery alert threshold: discharging below this percentage.
pub const BATTERY_LOW_PCT: u32 = 20;
/// Disk alert threshold: use% at or above this.
pub const DISK_USE_PCT: u32 = 90;

/// Parse `upower -d` output. Returns an alert when a battery is
/// discharging below [`BATTERY_LOW_PCT`].
pub fn battery_alert(upower_output: &str) -> Option<String> {
    // Split into device blocks ("Device: ..." starts a block).
    let mut blocks: Vec<Vec<&str>> = Vec::new();
    for line in upower_output.lines() {
        if line.starts_with("Device:") {
            blocks.push(Vec::new());
        }
        if let Some(b) = blocks.last_mut() {
            b.push(line);
        }
    }
    for block in blocks {
        let head = block.first().copied().unwrap_or("");
        if !head.to_ascii_lowercase().contains("battery") {
            continue;
        }
        let mut pct: Option<u32> = None;
        let mut state: Option<String> = None;
        for line in &block {
            let t = line.trim();
            if let Some(v) = t.strip_prefix("percentage:") {
                pct = v.trim().trim_end_matches('%').trim().parse().ok();
            } else if let Some(v) = t.strip_prefix("state:") {
                state = Some(v.trim().to_string());
            }
        }
        if let (Some(p), Some(s)) = (pct, state)
            && s == "discharging"
            && p < BATTERY_LOW_PCT
        {
            return Some(format!(
                "Batteria bassa: {p}% e in scarica. Collegala alla corrente."
            ));
        }
    }
    None
}

/// Parse `df -P /` output. Returns an alert when use% is at or
/// above [`DISK_USE_PCT`].
pub fn disk_alert(df_output: &str) -> Option<String> {
    for line in df_output.lines().skip(1) {
        let cols: Vec<&str> = line.split_whitespace().collect();
        // Filesystem 1K-blocks Used Available Use% Mounted-on
        if cols.len() < 6 || cols[5] != "/" {
            continue;
        }
        if let Ok(use_pct) = cols[4].trim_end_matches('%').parse::<u32>()
            && use_pct >= DISK_USE_PCT
        {
            return Some(format!(
                "Disco quasi pieno: {use_pct}% usato ({} liberi). Libera spazio.",
                cols[3]
            ));
        }
    }
    None
}

/// Parse `rpm-ostree upgrade --check` output. Returns an alert when
/// an update is available (the `AvailableUpdate:` block, with the
/// version when present).
pub fn updates_alert(rpm_ostree_output: &str) -> Option<String> {
    if !rpm_ostree_output.contains("AvailableUpdate:") {
        return None;
    }
    let version = rpm_ostree_output
        .lines()
        .find_map(|l| l.trim().strip_prefix("Version:"))
        .map(|v| v.trim().to_string());
    Some(match version {
        Some(v) => format!("Aggiornamento di NovaOS disponibile (versione {v})."),
        None => "Aggiornamento di NovaOS disponibile.".to_string(),
    })
}

/// Run one command, capturing stdout. `None` when the command does
/// not exist or fails to start (the check is simply skipped).
fn run_capture(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).output().ok()?;
    if !out.status.success() && out.stdout.is_empty() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Run all three checks and return the alerts that fired (empty
/// when everything is fine). Commands that are missing on the system
/// (e.g. `upower` on a desktop without battery) are skipped.
pub fn run_checks() -> Vec<String> {
    let mut alerts = Vec::new();
    if let Some(out) = run_capture("rpm-ostree", &["upgrade", "--check"])
        && let Some(a) = updates_alert(&out)
    {
        alerts.push(a);
    }
    if let Some(out) = run_capture("upower", &["-d"])
        && let Some(a) = battery_alert(&out)
    {
        alerts.push(a);
    }
    if let Some(out) = run_capture("df", &["-P", "/"])
        && let Some(a) = disk_alert(&out)
    {
        alerts.push(a);
    }
    alerts
}

/// Send each alert as a desktop notification via notify-send.
/// Falls back to printing on stdout when notify-send is missing.
/// Returns how many alerts were delivered (notify or stdout).
pub fn notify(alerts: &[String]) -> usize {
    let mut sent = 0;
    for a in alerts {
        let ok = Command::new("notify-send")
            .arg("NovaAI")
            .arg(a)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !ok {
            println!("NovaAI: {a}");
        }
        sent += 1;
    }
    sent
}

/// Run the checks and notify. Returns the number of alerts.
pub fn check_and_notify() -> usize {
    let alerts = run_checks();
    notify(&alerts)
}

// ---------------------------------------------------------------------------
// User systemd timer (hourly `nova-ai --check`)
// ---------------------------------------------------------------------------

fn systemd_user_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".config")
        .join("systemd")
        .join("user")
}

fn service_path() -> PathBuf {
    systemd_user_dir().join("nova-ai-watch.service")
}

fn timer_path() -> PathBuf {
    systemd_user_dir().join("nova-ai-watch.timer")
}

pub fn timer_installed() -> bool {
    timer_path().is_file()
}

/// Install + enable the hourly timer. The service runs this same
/// binary with `--check`.
pub fn install_timer() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("{e}"))?;
    let dir = systemd_user_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("impossibile creare {dir:?}: {e}"))?;
    fs::write(
        service_path(),
        format!(
            "[Unit]\nDescription=NovaAI: controlli periodici (aggiornamenti, batteria, disco)\n\n[Service]\nType=oneshot\nExecStart={} --check\n",
            exe.display()
        ),
    )
    .map_err(|e| format!("impossibile scrivere il servizio: {e}"))?;
    fs::write(
        timer_path(),
        "[Unit]\nDescription=NovaAI: controlli ogni ora\n\n[Timer]\nOnCalendar=hourly\nPersistent=true\n\n[Install]\nWantedBy=timers.target\n",
    )
    .map_err(|e| format!("impossibile scrivere il timer: {e}"))?;
    let reload = Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .output();
    match reload {
        Ok(o) if o.status.success() => {}
        _ => {
            return Err(
                "systemctl --user daemon-reload fallito (systemd utente non attivo?)".to_string(),
            );
        }
    }
    let enable = Command::new("systemctl")
        .args(["--user", "enable", "--now", "nova-ai-watch.timer"])
        .output();
    match enable {
        Ok(o) if o.status.success() => Ok(()),
        _ => Err("systemctl --user enable nova-ai-watch.timer fallito".to_string()),
    }
}

/// Disable the timer and remove the unit files.
pub fn remove_timer() -> Result<(), String> {
    let _ = Command::new("systemctl")
        .args(["--user", "disable", "--now", "nova-ai-watch.timer"])
        .output();
    let _ = fs::remove_file(service_path());
    let _ = fs::remove_file(timer_path());
    let _ = Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .output();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const UPOWER_LOW: &str = "Device: /org/freedesktop/UPower/devices/battery_BAT0\n  native-path:          BAT0\n  state:               discharging\n  percentage:          15%\n\nDevice: /org/freedesktop/UPower/devices/line_power_AC\n  state:               charging\n  percentage:          100%\n";

    const UPOWER_CHARGING: &str = "Device: /org/freedesktop/UPower/devices/battery_BAT0\n  state:               charging\n  percentage:          15%\n";

    const UPOWER_OK: &str = "Device: /org/freedesktop/UPower/devices/battery_BAT0\n  state:               discharging\n  percentage:          80%\n";

    const DF_FULL: &str = "Filesystem 1K-blocks Used Available Use% Mounted on\n/dev/mapper/rv 100000 95000 5000 95% /\n";

    const DF_OK: &str = "Filesystem 1K-blocks Used Available Use% Mounted on\n/dev/mapper/rv 100000 50000 50000 50% /\n";

    const RPM_UPDATE: &str = "AvailableUpdate:\n\tVersion: 45.20261127\n\tCommit: abcdef\n";

    const RPM_NONE: &str = "No updates available.\n";

    #[test]
    fn battery_thresholds() {
        let a = battery_alert(UPOWER_LOW).unwrap();
        assert!(a.contains("15%"), "{a}");
        assert!(battery_alert(UPOWER_CHARGING).is_none());
        assert!(battery_alert(UPOWER_OK).is_none());
        assert!(battery_alert("").is_none());
    }

    #[test]
    fn disk_thresholds() {
        let a = disk_alert(DF_FULL).unwrap();
        assert!(a.contains("95%"), "{a}");
        assert!(disk_alert(DF_OK).is_none());
    }

    #[test]
    fn updates_detection() {
        let a = updates_alert(RPM_UPDATE).unwrap();
        assert!(a.contains("45.20261127"), "{a}");
        assert!(updates_alert(RPM_NONE).is_none());
    }
}
