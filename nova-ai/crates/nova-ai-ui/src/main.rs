//! nova-ai — the NovaAI chat app.
//!
//! A small tab anchored bottom-right (gtk4-layer-shell) opens the chat
//! window. First run shows the setup screen (provider + API key).
//! All AI work happens in nova-ai-core, called from worker threads.

mod chat;
mod settings;
mod setup;
mod tab;
#[cfg(feature = "wakeword")]
mod wake;

use std::sync::{Arc, Mutex};

use adw::Application;
use adw::prelude::*;
use gio::ApplicationFlags;

use nova_ai_core::agent::Agent;
use nova_ai_core::config::Config;

/// Shared state: the agent (behind a mutex, used from worker threads)
/// and the config snapshot. The chat window is created lazily on the
/// first tab click and then reused.
pub struct AppState {
    pub agent: Option<Arc<Mutex<Agent>>>,
    pub config: Config,
    pub chat_win: Option<adw::ApplicationWindow>,
    /// Channel into the chat window (posted by the wake-word flow).
    pub(crate) chat_tx: Option<async_channel::Sender<chat::UiEvent>>,
    /// The tab button (for the wake-listening indicator).
    pub tab_btn: Option<gtk4::Button>,
    /// Running wake-word listener, if any.
    #[cfg(feature = "wakeword")]
    pub wake: Option<nova_ai_core::wakeword::WakeWordHandle>,
    /// Channel the listener sends its events to.
    #[cfg(feature = "wakeword")]
    pub wake_tx: Option<async_channel::Sender<nova_ai_core::wakeword::WakeEvent>>,
}

fn main() {
    let app = Application::builder()
        .application_id("org.novaos.NovaAI")
        .flags(ApplicationFlags::empty())
        .build();

    app.connect_activate(|app| {
        // Minimal dark-friendly CSS for the tab and message bubbles.
        let css = gtk4::CssProvider::new();
        css.load_from_string(
            ".novaai-tab { border-radius: 16px 0 0 16px; padding: 6px 14px; \
             background: alpha(@accent_bg_color, 0.92); color: @accent_fg_color; \
             font-weight: bold; border: none; } \
             .novaai-user { background: alpha(@accent_bg_color, 0.25); \
             border-radius: 12px; padding: 8px 10px; } \
             .novaai-assistant { background: alpha(@view_fg_color, 0.07); \
             border-radius: 12px; padding: 8px 10px; } \
             .novaai-note { color: @warning_color; font-size: 0.9em; }",
        );
        gtk4::style_context_add_provider_for_display(
            &gtk4::gdk::Display::default().expect("no display"),
            &css,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        let cfg = Config::load()
            .unwrap_or_else(|_| Config::for_provider(nova_ai_core::providers::ProviderKind::Groq));
        let state = std::rc::Rc::new(std::cell::RefCell::new(AppState {
            agent: None,
            config: cfg,
            chat_win: None,
            chat_tx: None,
            tab_btn: None,
            #[cfg(feature = "wakeword")]
            wake: None,
            #[cfg(feature = "wakeword")]
            wake_tx: None,
        }));

        // Wake-word event channel (main-loop receiver + sender kept in
        // state for listeners started later from setup/settings).
        #[cfg(feature = "wakeword")]
        {
            let (wake_tx, wake_rx) =
                async_channel::bounded::<nova_ai_core::wakeword::WakeEvent>(16);
            state.borrow_mut().wake_tx = Some(wake_tx);
            wake::install_receiver(app, state.clone(), wake_rx);
        }

        // The tab is always there; the chat window is created on demand.
        let tab_win = tab::build_tab(app, state.clone());

        if state.borrow().config.has_key() {
            match Agent::new(&state.borrow().config) {
                Ok(agent) => {
                    state.borrow_mut().agent = Some(Arc::new(Mutex::new(agent)));
                    // Resume wake listening if the user opted in (the
                    // model must be present; otherwise the chat explains
                    // how to download it on first open).
                    #[cfg(feature = "wakeword")]
                    let _ = wake::ensure_listener(&state);
                }
                Err(e) => {
                    eprintln!("NovaAI: cannot start agent: {e}");
                }
            }
        } else {
            // First run: setup screen instead of the chat.
            setup::show_setup(app, state.clone(), tab_win.clone());
        }
        tab_win.present();
    });

    app.run();
}
