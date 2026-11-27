//! Wake word "Ehi Nova": UI wiring for the core listener.
//!
//! The listener thread lives in nova-ai-core (feature `wakeword`).
//! This module owns its handle in [`AppState`], forwards its events
//! to the main loop over an async channel, and drives the tab
//! indicator. The mic is only captured while the feature is enabled
//! and the listener is running; detection is fully offline.

use std::cell::RefCell;
use std::rc::Rc;

use nova_ai_core::wakeword::{WakeEvent, WakeWordHandle, model_dir, model_present};

use crate::AppState;
use crate::chat;
use crate::tab;

/// Start the listener (model must already be downloaded). Shows the
/// mic dot on the tab. Returns a user-facing error when the model is
/// missing or the mic cannot be opened.
pub fn start_listener(state: &Rc<RefCell<AppState>>) -> Result<(), String> {
    let tx = state
        .borrow()
        .wake_tx
        .clone()
        .ok_or_else(|| "canale wake non pronto".to_string())?;
    let dir = model_dir();
    if !model_present() {
        return Err("modello 'Ehi Nova' mancante: scaricalo dalle Impostazioni".to_string());
    }
    if state.borrow().wake.is_some() {
        return Ok(());
    }
    let handle = WakeWordHandle::start(&dir, move |ev| {
        let _ = tx.try_send(ev);
    })?;
    state.borrow_mut().wake = Some(handle);
    tab::set_listening(state, true);
    Ok(())
}

/// Stop the listener (if running) and clear the tab indicator.
pub fn stop_listener(state: &Rc<RefCell<AppState>>) {
    if let Some(handle) = state.borrow_mut().wake.take() {
        handle.stop();
    }
    tab::set_listening(state, false);
}

/// Pause detection (the mic thread keeps running but audio is
/// discarded). Used while a voice turn is in progress.
pub fn pause_listener(state: &Rc<RefCell<AppState>>) {
    if let Some(handle) = state.borrow().wake.as_ref() {
        handle.pause();
    }
}

/// Resume detection after a pause. No-op when not paused.
pub fn resume_listener(state: &Rc<RefCell<AppState>>) {
    if let Some(handle) = state.borrow().wake.as_ref() {
        handle.resume();
    }
}

/// Is the listener running right now?
pub fn is_listening(state: &Rc<RefCell<AppState>>) -> bool {
    state.borrow().wake.is_some()
}

/// Start the listener if the config asks for it and the model is
/// present. Called at startup and after setup/settings enable the
/// feature. Returns an error when it could not start (model missing,
/// no agent yet, mic unavailable).
pub fn ensure_listener(state: &Rc<RefCell<AppState>>) -> Result<(), String> {
    if !state.borrow().config.wake_word {
        return Ok(());
    }
    // No agent yet (first run, no key): nothing a wake turn could do.
    if state.borrow().agent.is_none() {
        return Ok(());
    }
    if is_listening(state) {
        return Ok(());
    }
    start_listener(state)
}

/// Install the main-loop receiver for wake events. Call once, at
/// startup, with the receiving end of the channel created in main.
pub fn install_receiver(
    app: &adw::Application,
    state: Rc<RefCell<AppState>>,
    rx: async_channel::Receiver<WakeEvent>,
) {
    // The async block is 'static: it needs an owned app, not the
    // borrowed reference.
    let app = app.clone();
    glib::MainContext::default().spawn_local(async move {
        while let Ok(ev) = rx.recv().await {
            match ev {
                WakeEvent::Detected => {
                    chat::show_chat(&app, state.clone());
                    chat::note(&state, "Ti ascolto… parla ora.");
                }
                WakeEvent::CommandReady(samples) => {
                    chat::wake_command_turn(state.clone(), samples);
                }
                WakeEvent::Error(e) => {
                    chat::note(&state, &format!("Ehi Nova in errore: {e}"));
                    stop_listener(&state);
                }
            }
        }
    });
}
