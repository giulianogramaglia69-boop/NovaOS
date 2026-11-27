//! novaos-common — shared helpers for the NovaOS system apps.
//!
//! No GUI code lives here: the three apps (novaos-setup, novaos-updates,
//! novaos-snapshots) are GTK front-ends over these helpers, so the logic
//! is unit-testable with plain `cargo test`.
//!
//! Modules:
//! - `system`: run commands, with or without pkexec, captured or streamed.
//! - `ostree`: `rpm-ostree status --json` parsing + `ostree admin pin` index.
//! - `osrelease`: /etc/os-release parsing, current desktop detection.
//! - `de`: the five NovaOS desktops and their GHCR image refs.
//! - `flags`: per-app "done" flags (first-run seen, package installed).
//! - `dates`: epoch → Italian local date/time (pure Rust, no chrono).
//! - `notify`: `notify-send` wrapper (best effort).

pub mod dates;
pub mod de;
pub mod flags;
pub mod notify;
pub mod osrelease;
pub mod ostree;
pub mod system;
