//! First-run setup: provider picker, API key, model, Save + Test.
//! Shown instead of the chat when no key is configured. Only after a
//! successful test call does the chat open.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use nova_ai_core::agent::Agent;
use nova_ai_core::config::Config;
use nova_ai_core::llm::LlmClient;
use nova_ai_core::providers::{ProviderKind, provider_defaults};

use crate::AppState;
use crate::chat;

pub fn show_setup(
    app: &adw::Application,
    state: Rc<RefCell<AppState>>,
    _tab_win: gtk4::ApplicationWindow,
) {
    let win = adw::ApplicationWindow::builder()
        .application(app)
        .title("NovaAI — Configurazione")
        .default_width(420)
        .default_height(520)
        .build();

    let header = adw::HeaderBar::new();
    let title = gtk4::Label::new(Some("✦ Configura NovaAI"));
    title.add_css_class("title");
    header.set_title_widget(Some(&title));
    win.set_titlebar(Some(&header));

    let list = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    list.set_margin_top(16);
    list.set_margin_bottom(16);
    list.set_margin_start(16);
    list.set_margin_end(16);

    let intro = gtk4::Label::new(Some(
        "Per funzionare NovaAI ha bisogno di un provider AI e di una chiave API (tranne per il provider locale Ollama, che non ne usa). \
         La chiave viene salvata nel portachiavi di sistema (libsecret); se non è \
         disponibile, nel file ~/.config/nova-ai/config.toml (permessi 0600).",
    ));
    intro.set_wrap(true);
    intro.set_xalign(0.0);
    list.append(&intro);

    // Provider picker (DropDown + StringList: ComboBoxText is
    // deprecated since GTK 4.10).
    let provider_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    provider_row.append(&gtk4::Label::new(Some("Provider")));
    let provider_names: Vec<&str> = ProviderKind::all()
        .iter()
        .map(|k| k.display_name())
        .collect();
    let provider_combo = gtk4::DropDown::builder()
        .model(&gtk4::StringList::new(&provider_names))
        .selected(0) // Groq first (free tier)
        .hexpand(true)
        .build();
    provider_row.append(&provider_combo);
    list.append(&provider_row);

    // Base URL.
    let url_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    url_row.append(&gtk4::Label::new(Some("Base URL")));
    let url_entry = gtk4::Entry::new();
    url_entry.set_hexpand(true);
    url_row.append(&url_entry);
    list.append(&url_row);

    // Model.
    let model_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    model_row.append(&gtk4::Label::new(Some("Modello")));
    let model_entry = gtk4::Entry::new();
    model_entry.set_hexpand(true);
    model_row.append(&model_entry);
    list.append(&model_row);

    // API key.
    let key_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    key_row.append(&gtk4::Label::new(Some("Chiave API")));
    let key_entry = gtk4::PasswordEntry::new();
    key_entry.set_hexpand(true);
    key_entry.set_placeholder_text(Some("sk-… / gsk-…"));
    key_row.append(&key_entry);
    list.append(&key_row);

    // Local (Ollama) note, shown only for the Local provider.
    let local_note = gtk4::Label::new(Some(
        "Locale (Ollama): nessuna chiave API. Ollama deve essere installato e in esecuzione su questo PC (da ollama.com — non è incluso in NovaOS: troppo grosso); scarica il modello con `ollama pull qwen2.5:7b`.",
    ));
    local_note.set_wrap(true);
    local_note.set_xalign(0.0);
    local_note.set_visible(false);
    list.append(&local_note);

    // Wake word (opt-in, default OFF). The mic only listens locally;
    // the audio never leaves the device until the phrase is heard.
    #[cfg(feature = "wakeword")]
    let wake_check = gtk4::CheckButton::with_label("Attiva “Ehi Nova” (wake word)");
    #[cfg(feature = "wakeword")]
    list.append(&wake_check);
    #[cfg(feature = "wakeword")]
    {
        let explain = gtk4::Label::new(Some(
            "Se attivo, il microfono resta in ascolto in locale: il riconoscimento della parola “Ehi Nova” avviene tutto sul dispositivo e l'audio non lascia mai NovaOS. Solo il comando che dici dopo l'attivazione viene inviato all'API Whisper del provider per la trascrizione. Al primo uso scarica il modello italiano (47 MB).",
        ));
        explain.set_wrap(true);
        explain.set_xalign(0.0);
        list.append(&explain);
    }
    #[cfg(feature = "wakeword")]
    let wake_progress = gtk4::ProgressBar::new();
    #[cfg(feature = "wakeword")]
    {
        wake_progress.set_visible(false);
        list.append(&wake_progress);
    }

    // Spoken answers (opt-in, default OFF). The flag is only persisted
    // here; Piper + the voice download lazily on the first spoken
    // answer (~55 MB).
    let tts_check = gtk4::CheckButton::with_label("Leggi ad alta voce le risposte (voce offline)");
    list.append(&tts_check);
    {
        let explain = gtk4::Label::new(Some(
            "Se attivo, NovaAI legge le risposte con una voce italiana offline (Piper). La voce e il sintetizzatore (circa 55 MB) si scaricano da soli alla prima risposta parlata.",
        ));
        explain.set_wrap(true);
        explain.set_xalign(0.0);
        list.append(&explain);
    }

    // Prefill from the selected provider.
    let apply_provider_defaults = {
        let url_entry = url_entry.clone();
        let model_entry = model_entry.clone();
        let key_entry = key_entry.clone();
        let local_note = local_note.clone();
        let provider_combo = provider_combo.clone();
        Rc::new(move || {
            let kind = ProviderKind::all()
                .get(provider_combo.selected() as usize)
                .copied()
                .unwrap_or(ProviderKind::Groq);
            let (url, model) = provider_defaults(kind);
            url_entry.set_text(url);
            model_entry.set_text(model);
            // Local needs no key: hide the key field, show the note.
            let local = kind == ProviderKind::Local;
            key_entry.set_sensitive(!local);
            local_note.set_visible(local);
        })
    };
    apply_provider_defaults();
    {
        let apply = apply_provider_defaults.clone();
        provider_combo.connect_selected_notify(move |_| apply());
    }

    // Status + save button.
    let status = gtk4::Label::new(None);
    status.set_wrap(true);
    status.set_xalign(0.0);
    list.append(&status);
    let save_btn = gtk4::Button::with_label("Salva e verifica");
    save_btn.add_css_class("suggested-action");
    list.append(&save_btn);

    let scroll = gtk4::ScrolledWindow::builder()
        .vexpand(true)
        .child(&list)
        .build();
    win.set_content(Some(&scroll));

    {
        let state = state.clone();
        let app = app.clone();
        let win = win.clone();
        let provider_combo = provider_combo.clone();
        let url_entry = url_entry.clone();
        let model_entry = model_entry.clone();
        let key_entry = key_entry.clone();
        let status = status.clone();
        let save_btn = save_btn.clone();
        #[cfg(feature = "wakeword")]
        let wake_check = wake_check.clone();
        #[cfg(feature = "wakeword")]
        let wake_progress = wake_progress.clone();
        let tts_check = tts_check.clone();
        // Clone for the closure: connect_clicked borrows the button
        // while the closure moves its own handle in, so the receiver
        // and the captured handle must be different clones.
        let save_btn_c = save_btn.clone();
        save_btn.connect_clicked(move |_| {
            let kind = ProviderKind::all()
                .get(provider_combo.selected() as usize)
                .copied()
                .unwrap_or(ProviderKind::Groq);
            let base_url = url_entry.text().trim().to_string();
            let model = model_entry.text().trim().to_string();
            let key = key_entry.text().to_string();
            #[cfg(feature = "wakeword")]
            let wake_requested = wake_check.is_active();
            #[cfg(not(feature = "wakeword"))]
            let wake_requested = false;
            let tts_requested = tts_check.is_active();
            // Local (Ollama) needs no API key.
            if kind != ProviderKind::Local && key.trim().is_empty() {
                status.set_text("Inserisci la chiave API.");
                return;
            }
            if model.is_empty() {
                status.set_text("Inserisci il nome del modello.");
                return;
            }
            if kind == ProviderKind::Custom && base_url.is_empty() {
                status.set_text("Per il provider Custom serve la Base URL.");
                return;
            }
            let mut cfg = Config::for_provider(kind);
            if !base_url.is_empty() {
                cfg.base_url = base_url;
            }
            cfg.model = model;
            cfg.tts = tts_requested;
            status.set_text(if kind == ProviderKind::Local {
                "Verifico Ollama…"
            } else {
                "Verifico la chiave…"
            });
            save_btn_c.set_sensitive(false);

            let state = state.clone();
            let app = app.clone();
            let win = win.clone();
            let status = status.clone();
            let save_btn = save_btn_c.clone();
            let key = key.clone();
            // Result channel back to the main loop (spawn_local must be
            // set up on the main thread, not inside the worker). The test
            // thread gets its own cloned config/key: `cfg` and `key`
            // move into the spawn_local future below.
            let (tx, rx) = async_channel::bounded::<Result<String, String>>(1);
            let test_cfg = {
                let mut c = cfg.clone();
                c.api_key = Some(key.clone());
                c
            };
            {
                let state = state.clone();
                let app = app.clone();
                let win = win.clone();
                let status = status.clone();
                let save_btn = save_btn.clone();
                #[cfg(feature = "wakeword")]
                let wake_check = wake_check.clone();
                #[cfg(feature = "wakeword")]
                let wake_progress = wake_progress.clone();
                glib::MainContext::default().spawn_local(async move {
                    let Ok(result) = rx.recv().await else { return };
                    match result {
                        Ok(reply) => {
                            // Store the key (keyring first, config fallback).
                            // Local needs no key: nothing to store.
                            let mut cfg2 = cfg.clone();
                            let stored: Result<(), String> = if kind == ProviderKind::Local {
                                Ok(())
                            } else {
                                cfg2.store_key(&key).map(|_| ())
                            };
                            match stored {
                                Ok(_) => {
                                    let _ = cfg2.save_without_key().or_else(|_| cfg2.save());
                                    state.borrow_mut().config = cfg2.clone();
                                    match Agent::new(&cfg2) {
                                        Ok(agent) => {
                                            state.borrow_mut().agent =
                                                Some(std::sync::Arc::new(std::sync::Mutex::new(
                                                    agent,
                                                )));
                                            if wake_requested {
                                                // Wake word: model already
                                                // here? Start listening.
                                                // Otherwise download it
                                                // first (with progress),
                                                // then open the chat.
                                                #[cfg(feature = "wakeword")]
                                                {
                                                    if nova_ai_core::wakeword::model_present() {
                                                        let mut cfg3 =
                                                            state.borrow().config.clone();
                                                        cfg3.wake_word = true;
                                                        let _ = cfg3.save();
                                                        state.borrow_mut().config = cfg3;
                                                        let _ =
                                                            crate::wake::ensure_listener(&state);
                                                        win.close();
                                                        chat::show_chat(&app, state.clone());
                                                    } else {
                                                        start_model_download(
                                                            &app,
                                                            &state,
                                                            &win,
                                                            &status,
                                                            &wake_progress,
                                                            &save_btn,
                                                            &wake_check,
                                                        );
                                                    }
                                                }
                                                #[cfg(not(feature = "wakeword"))]
                                                {
                                                    win.close();
                                                    chat::show_chat(&app, state.clone());
                                                }
                                            } else {
                                                win.close();
                                                chat::show_chat(&app, state.clone());
                                            }
                                        }
                                        Err(e) => {
                                            status.set_text(&format!(
                                                "Chiave valida (risposta: {reply}) ma l'agente non parte: {e}"
                                            ));
                                            save_btn.set_sensitive(true);
                                        }
                                    }
                                }
                                Err(e) => {
                                    status.set_text(&format!("Impossibile salvare la chiave: {e}"));
                                    save_btn.set_sensitive(true);
                                }
                            }
                        }
                        Err(e) => {
                            status.set_text(&format!("Verifica fallita: {e}"));
                            save_btn.set_sensitive(true);
                        }
                    }
                });
            }
            std::thread::spawn(move || {
                let client = LlmClient::from_config(&test_cfg);
                let test = client.test_connection().map_err(|e| format!("{e}"));
                let _ = tx.send_blocking(test);
            });
        });
    }

    win.present();
}

/// Download the wake-word model with progress in the setup window.
/// On success the toggle is persisted, the listener starts and the
/// chat opens. On failure the error is explained, the wake option is
/// unchecked, and the save button becomes "continue without Ehi Nova"
/// — the chat and the voice button keep working.
#[cfg(feature = "wakeword")]
fn start_model_download(
    app: &adw::Application,
    state: &Rc<RefCell<AppState>>,
    win: &adw::ApplicationWindow,
    status: &gtk4::Label,
    progress: &gtk4::ProgressBar,
    save_btn: &gtk4::Button,
    wake_check: &gtk4::CheckButton,
) {
    use crate::DownloadEvent;

    status.set_text("Scarico il modello “Ehi Nova” (47 MB)…");
    progress.set_visible(true);
    progress.set_fraction(0.0);
    save_btn.set_sensitive(false);

    let (tx, rx) = async_channel::bounded::<DownloadEvent>(16);
    let state = state.clone();
    let app = app.clone();
    let win = win.clone();
    let status = status.clone();
    let progress = progress.clone();
    let save_btn = save_btn.clone();
    let wake_check = wake_check.clone();
    glib::MainContext::default().spawn_local(async move {
        while let Ok(ev) = rx.recv().await {
            match ev {
                DownloadEvent::Progress(got, total) => {
                    if total > 0 {
                        progress.set_fraction(got as f64 / total as f64);
                    }
                }
                DownloadEvent::Done(Ok(_)) => {
                    let mut cfg = state.borrow().config.clone();
                    cfg.wake_word = true;
                    let _ = cfg.save();
                    state.borrow_mut().config = cfg;
                    let _ = crate::wake::ensure_listener(&state);
                    win.close();
                    chat::show_chat(&app, state.clone());
                    break;
                }
                DownloadEvent::Done(Err(e)) => {
                    progress.set_visible(false);
                    wake_check.set_active(false);
                    status.set_text(&format!(
                        "Download del modello fallito: {e} Puoi continuare senza “Ehi Nova” e riprovare dopo dalle Impostazioni."
                    ));
                    save_btn.set_label("Continua senza “Ehi Nova”");
                    save_btn.set_sensitive(true);
                    break;
                }
            }
        }
    });
    std::thread::spawn(move || {
        let r = nova_ai_core::wakeword::download_model(|got, total| {
            let _ = tx.send_blocking(DownloadEvent::Progress(got, total));
        });
        let _ = tx.send_blocking(DownloadEvent::Done(r));
    });
}
