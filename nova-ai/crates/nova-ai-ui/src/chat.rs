//! The chat window: message list, input, attachments, voice, and the
//! confirmation flow for mutating actions. All AI calls run in worker
//! threads; results come back over an async-channel into the main loop.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use adw::prelude::*;
use base64::Engine;
use nova_ai_core::agent::{Agent, ChatOutcome};
use nova_ai_core::attachments::{Attachment, AttachmentKind};
use nova_ai_core::llm::LlmClient;
use nova_ai_core::voice;

use crate::AppState;

pub(crate) enum UiEvent {
    AgentReply(ChatOutcome),
    Transcript(Result<String, String>),
    /// A short note bubble (wake word only).
    #[cfg(feature = "wakeword")]
    Note(String),
    /// A hands-free turn finished; the listener may resume.
    #[cfg(feature = "wakeword")]
    WakeTurnDone,
}

fn add_bubble(list: &gtk4::Box, scroll: &gtk4::ScrolledWindow, text: &str, user: bool) {
    let label = gtk4::Label::new(Some(text));
    label.set_wrap(true);
    label.set_xalign(0.0);
    label.set_selectable(true);
    label.add_css_class(if user {
        "novaai-user"
    } else {
        "novaai-assistant"
    });
    // Right-align user messages.
    let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    if user {
        let spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        row.append(&spacer);
    }
    row.append(&label);
    list.append(&row);
    scroll_to_bottom(scroll);
}

fn add_note(list: &gtk4::Box, scroll: &gtk4::ScrolledWindow, text: &str) {
    let label = gtk4::Label::new(Some(text));
    label.set_wrap(true);
    label.set_xalign(0.0);
    label.add_css_class("novaai-note");
    list.append(&label);
    scroll_to_bottom(scroll);
}

fn scroll_to_bottom(scroll: &gtk4::ScrolledWindow) {
    let adj = scroll.vadjustment();
    adj.set_value(adj.upper() - adj.page_size());
}

/// Downscale an image to max 1024px on the long edge and return it as a
/// base64 data URL (JPEG, quality 85).
fn load_image_attachment(path: &std::path::Path, name: &str) -> Result<Attachment, String> {
    let pix = gdk_pixbuf::Pixbuf::from_file(path).map_err(|e| format!("{e}"))?;
    let (w, h) = (pix.width(), pix.height());
    let max = 1024;
    let (nw, nh) = if w.max(h) > max {
        if w >= h {
            (max, h * max / w)
        } else {
            (w * max / h, max)
        }
    } else {
        (w, h)
    };
    let scaled = pix
        .scale_simple(nw, nh, gdk_pixbuf::InterpType::Bilinear)
        .ok_or_else(|| "ridimensionamento immagine fallito".to_string())?;
    let bytes = scaled
        .save_to_bufferv("jpeg", &[("quality", "85")])
        .map_err(|e| format!("{e}"))?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok(Attachment::Image {
        name: name.to_string(),
        data_url: format!("data:image/jpeg;base64,{b64}"),
    })
}

pub fn show_chat(app: &adw::Application, state: Rc<RefCell<AppState>>) {
    if let Some(win) = &state.borrow().chat_win {
        win.present();
        return;
    }
    let Some(agent) = state.borrow().agent.clone() else {
        return; // setup not done yet
    };
    let config = state.borrow().config.clone();

    let win = adw::ApplicationWindow::builder()
        .application(app)
        .title("NovaAI")
        .default_width(420)
        .default_height(620)
        .build();

    // Header: title + model, quit button.
    let header = adw::HeaderBar::new();
    let title = gtk4::Label::new(Some("✦ NovaAI"));
    title.add_css_class("title");
    let model_label = gtk4::Label::new(Some(
        &agent
            .lock()
            .map(|a| a.model_label())
            .unwrap_or_else(|e| e.into_inner().model_label()),
    ));
    model_label.add_css_class("subtitle");
    let title_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    title_box.append(&title);
    title_box.append(&model_label);
    header.set_title_widget(Some(&title_box));
    let quit_btn = gtk4::Button::with_label("Esci");
    let app_quit = app.clone();
    quit_btn.connect_clicked(move |_| app_quit.quit());
    header.pack_end(&quit_btn);
    let settings_btn = gtk4::Button::with_label("⚙");
    settings_btn.set_tooltip_text(Some("Impostazioni"));
    {
        let app = app.clone();
        let state = state.clone();
        settings_btn.connect_clicked(move |_| {
            crate::settings::show_settings(&app, state.clone());
        });
    }
    header.pack_end(&settings_btn);
    win.set_titlebar(Some(&header));

    // Message list.
    let list = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    list.set_margin_top(8);
    list.set_margin_bottom(8);
    list.set_margin_start(8);
    list.set_margin_end(8);
    let scroll = gtk4::ScrolledWindow::builder()
        .vexpand(true)
        .child(&list)
        .build();
    scroll.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);

    // Attachments state.
    let attachments: Rc<RefCell<Vec<Attachment>>> = Rc::new(RefCell::new(Vec::new()));
    let attach_label = gtk4::Label::new(None);
    attach_label.set_xalign(0.0);

    // Bottom bar: attach, entry, voice, send.
    let entry = gtk4::Entry::new();
    entry.set_placeholder_text(Some("Scrivi a NovaAI…"));
    entry.set_hexpand(true);
    let attach_btn = gtk4::Button::with_label("📎");
    attach_btn.set_tooltip_text(Some("Allega file o foto"));
    let voice_btn = gtk4::Button::with_label("🎤");
    let send_btn = gtk4::Button::with_label("➤");
    send_btn.add_css_class("suggested-action");

    // Voice availability depends on the provider's STT support.
    let stt_ok = agent
        .lock()
        .map(|a| a.stt_model().is_some())
        .unwrap_or_else(|e| e.into_inner().stt_model().is_some());
    if !stt_ok {
        voice_btn.set_sensitive(false);
        voice_btn.set_tooltip_text(Some(
            "La trascrizione vocale non è disponibile con questo provider (serve OpenAI o Groq)",
        ));
    } else {
        voice_btn.set_tooltip_text(Some("Tieni premuto per parlare"));
    }
    let vision_ok = agent
        .lock()
        .map(|a| a.vision_supported())
        .unwrap_or_else(|e| e.into_inner().vision_supported());

    let bottom = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    bottom.set_margin_top(6);
    bottom.set_margin_bottom(8);
    bottom.set_margin_start(8);
    bottom.set_margin_end(8);
    bottom.append(&attach_btn);
    bottom.append(&entry);
    bottom.append(&voice_btn);
    bottom.append(&send_btn);

    let content = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    content.append(&scroll);
    content.append(&attach_label);
    content.append(&bottom);
    win.set_content(Some(&content));

    // Close hides the window instead of quitting (the tab reopens it).
    win.connect_close_request(|win| {
        win.set_visible(false);
        glib::Propagation::Stop
    });

    // Event channel from worker threads.
    let (tx, rx) = async_channel::bounded::<UiEvent>(8);
    // Kept in AppState so the wake-word flow can post into the chat.
    state.borrow_mut().chat_tx = Some(tx.clone());

    // --- send flow (shared by the send button and Enter) ---
    let do_send: Rc<dyn Fn()> = {
        let agent = agent.clone();
        let tx = tx.clone();
        let list = list.clone();
        let scroll = scroll.clone();
        let entry = entry.clone();
        let send_btn = send_btn.clone();
        let attachments = attachments.clone();
        let attach_label = attach_label.clone();
        Rc::new(move || {
            let text = entry.text().trim().to_string();
            if text.is_empty() {
                return;
            }
            let atts = std::mem::take(&mut *attachments.borrow_mut());
            attach_label.set_text("");
            add_bubble(&list, &scroll, &text, true);
            entry.set_text("");
            send_btn.set_sensitive(false);
            let agent = agent.clone();
            let tx = tx.clone();
            std::thread::spawn(move || {
                let outcome = agent
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .send(&text, &atts);
                let _ = tx.send_blocking(UiEvent::AgentReply(outcome));
            });
        })
    };
    {
        let do_send = do_send.clone();
        send_btn.connect_clicked(move |_| do_send());
    }
    {
        let do_send = do_send.clone();
        entry.connect_activate(move |_| do_send());
    }

    // --- attachments ---
    {
        let attachments = attachments.clone();
        let attach_label = attach_label.clone();
        let list = list.clone();
        let scroll = scroll.clone();
        let win_weak = win.clone();
        attach_btn.connect_clicked(move |_| {
            let dialog = gtk4::FileDialog::builder()
                .title("Allega file o foto")
                .build();
            let attachments = attachments.clone();
            let attach_label = attach_label.clone();
            let list = list.clone();
            let scroll = scroll.clone();
            dialog.open_multiple(
                Some(&win_weak),
                None::<&gio::Cancellable>,
                move |res| {
                    let Ok(files) = res else { return };
                    let mut added = 0;
                    for file in files.iter::<gio::File>().flatten() {
                        let Some(path) = file.path() else { continue };
                        let name = path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("file")
                            .to_string();
                        let Ok(bytes) = std::fs::read(&path) else { continue };
                        match Attachment::classify(&name, &bytes) {
                            AttachmentKind::Text => {
                                if let Ok(content) = String::from_utf8(bytes) {
                                    attachments.borrow_mut().push(Attachment::TextFile {
                                        name,
                                        content,
                                    });
                                    added += 1;
                                }
                            }
                            AttachmentKind::Image => {
                                match load_image_attachment(&path, &name) {
                                    Ok(att) => {
                                        attachments.borrow_mut().push(att);
                                        added += 1;
                                        if !vision_ok {
                                            add_note(&list, &scroll, "Il modello corrente non supporta la visione: l'immagine verrà inviata solo come nome.");
                                        }
                                    }
                                    Err(e) => {
                                        add_note(&list, &scroll, &format!("Immagine {name} non allegata: {e}"));
                                    }
                                }
                            }
                            AttachmentKind::Other => {
                                attachments.borrow_mut().push(Attachment::Other { name });
                                added += 1;
                            }
                        }
                    }
                    if added > 0 {
                        let n = attachments.borrow().len();
                        attach_label.set_text(&format!("📎 {n} allegato/i (verranno inviati col prossimo messaggio)"));
                    }
                },
            );
        });
    }

    // --- voice: hold to record, release to transcribe ---
    {
        let recording: Rc<RefCell<Option<voice::Recording>>> = Rc::new(RefCell::new(None));
        let tx = tx.clone();
        let config = config.clone();
        let voice_btn_weak = voice_btn.clone();
        #[cfg(feature = "wakeword")]
        let state_v = state.clone();
        let gesture = gtk4::GestureClick::new();
        {
            let recording = recording.clone();
            let voice_btn_weak = voice_btn_weak.clone();
            #[cfg(feature = "wakeword")]
            let state_v = state_v.clone();
            gesture.connect_pressed(move |_, _, _, _| {
                // While the user talks into the button, the wake listener
                // must not hear "ehi nova" in their own speech.
                #[cfg(feature = "wakeword")]
                crate::wake::pause_listener(&state_v);
                match voice::Recording::start() {
                    Ok(rec) => {
                        *recording.borrow_mut() = Some(rec);
                        voice_btn_weak.add_css_class("destructive-action");
                    }
                    Err(e) => {
                        voice_btn_weak
                            .set_tooltip_text(Some(&format!("Registrazione fallita: {e}")));
                        #[cfg(feature = "wakeword")]
                        crate::wake::resume_listener(&state_v);
                    }
                }
            });
        }
        {
            let recording = recording.clone();
            let voice_btn_weak = voice_btn_weak.clone();
            #[cfg(feature = "wakeword")]
            let state_v = state_v.clone();
            gesture.connect_released(move |_, _, _, _| {
                voice_btn_weak.remove_css_class("destructive-action");
                if let Some(rec) = recording.borrow_mut().take() {
                    let tx = tx.clone();
                    let config = config.clone();
                    std::thread::spawn(move || {
                        let result = rec.stop().and_then(|wav| {
                            LlmClient::from_config(&config)
                                .transcribe(&wav)
                                .map_err(|e| format!("{e}"))
                        });
                        let _ = tx.send_blocking(UiEvent::Transcript(result));
                    });
                } else {
                    #[cfg(feature = "wakeword")]
                    crate::wake::resume_listener(&state_v);
                }
            });
        }
        voice_btn.add_controller(gesture);
    }

    // --- event receiver (main loop) ---
    {
        let list = list.clone();
        let scroll = scroll.clone();
        let entry = entry.clone();
        let send_btn = send_btn.clone();
        let agent = agent.clone();
        let tx = tx.clone();
        #[cfg(feature = "wakeword")]
        let state = state.clone();
        glib::MainContext::default().spawn_local(async move {
            while let Ok(ev) = rx.recv().await {
                match ev {
                    UiEvent::AgentReply(outcome) => {
                        send_btn.set_sensitive(true);
                        match outcome {
                            ChatOutcome::Answer(text) => {
                                add_bubble(&list, &scroll, &text, false);
                            }
                            ChatOutcome::Error(err) => {
                                add_note(&list, &scroll, &format!("Errore: {err}"));
                            }
                            ChatOutcome::NeedsConfirmation(action) => {
                                add_confirmation_row(&list, &scroll, &agent, &tx, action);
                            }
                        }
                    }
                    UiEvent::Transcript(result) => {
                        match result {
                            Ok(text) => entry.set_text(&text),
                            Err(e) => {
                                add_note(&list, &scroll, &format!("Trascrizione fallita: {e}"))
                            }
                        }
                        #[cfg(feature = "wakeword")]
                        crate::wake::resume_listener(&state);
                    }
                    #[cfg(feature = "wakeword")]
                    UiEvent::Note(text) => {
                        add_note(&list, &scroll, &text);
                    }
                    #[cfg(feature = "wakeword")]
                    UiEvent::WakeTurnDone => {
                        crate::wake::resume_listener(&state);
                    }
                }
            }
        });
    }

    add_bubble(
        &list,
        &scroll,
        "Ciao, sono NovaAI. Posso aprire app, installare software, regolare volume, luminosità, Wi-Fi e tema. Cosa facciamo?",
        false,
    );
    #[cfg(feature = "wakeword")]
    if state.borrow().config.wake_word && !nova_ai_core::wakeword::model_present() {
        add_note(
            &list,
            &scroll,
            "“Ehi Nova” è attivo ma il modello non è scaricato: aprilo dalle Impostazioni per scaricarlo.",
        );
    }

    state.borrow_mut().chat_win = Some(win.clone());
    win.present();
}

/// Post a short note into the chat (no-op when the chat is closed).
/// Wake word only: used by the listener flow.
#[cfg(feature = "wakeword")]
pub(crate) fn note(state: &Rc<RefCell<AppState>>, text: &str) {
    if let Some(tx) = state.borrow().chat_tx.clone() {
        let _ = tx.try_send(UiEvent::Note(text.to_string()));
    }
}

/// Run a hands-free turn from wake-word audio: transcribe it, then
/// send the transcript to the agent like a typed message. Posts
/// `WakeTurnDone` when finished so the listener can resume. If a
/// confirmation is pending, the turn is skipped (a hands-free turn
/// must never overwrite it) and the user is told why.
#[cfg(feature = "wakeword")]
pub(crate) fn wake_command_turn(state: Rc<RefCell<AppState>>, samples: Vec<i16>) {
    let Some(tx) = state.borrow().chat_tx.clone() else {
        return;
    };
    let Some(agent) = state.borrow().agent.clone() else {
        return;
    };
    let config = state.borrow().config.clone();
    std::thread::spawn(move || {
        let wav = nova_ai_core::wakeword::samples_to_wav(&samples);
        match LlmClient::from_config(&config).transcribe(&wav) {
            Ok(text) if !text.trim().is_empty() => {
                let text = text.trim().to_string();
                let outcome = {
                    let mut guard = agent.lock().unwrap_or_else(|e| e.into_inner());
                    if guard.has_pending_action() {
                        let _ = tx.try_send(UiEvent::Note(
                            "C'è un'azione in attesa di conferma: approva o rifiuta prima di continuare."
                                .to_string(),
                        ));
                        let _ = tx.try_send(UiEvent::WakeTurnDone);
                        return;
                    }
                    guard.send(&text, &[])
                };
                let _ = tx.try_send(UiEvent::AgentReply(outcome));
                let _ = tx.try_send(UiEvent::WakeTurnDone);
            }
            Ok(_) => {
                let _ = tx.try_send(UiEvent::Note(
                    "Non ho sentito niente. Riprova con “Ehi Nova”.".to_string(),
                ));
                let _ = tx.try_send(UiEvent::WakeTurnDone);
            }
            Err(e) => {
                let _ = tx.try_send(UiEvent::Note(format!("Trascrizione fallita: {e}")));
                let _ = tx.try_send(UiEvent::WakeTurnDone);
            }
        }
    });
}

/// A row with the proposed action and Approve / Deny buttons.
fn add_confirmation_row(
    list: &gtk4::Box,
    scroll: &gtk4::ScrolledWindow,
    agent: &Arc<Mutex<Agent>>,
    tx: &async_channel::Sender<UiEvent>,
    action: nova_ai_core::tools::PendingAction,
) {
    let row = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
    row.add_css_class("novaai-assistant");
    let label = gtk4::Label::new(Some(&format!(
        "Confermi questa azione?\n{}",
        action.describe()
    )));
    label.set_wrap(true);
    label.set_xalign(0.0);
    let btns = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    let approve = gtk4::Button::with_label("Approva");
    approve.add_css_class("suggested-action");
    let deny = gtk4::Button::with_label("Rifiuta");
    deny.add_css_class("destructive-action");
    btns.append(&approve);
    btns.append(&deny);
    row.append(&label);
    row.append(&btns);
    list.append(&row);
    scroll_to_bottom(scroll);

    for (btn, ok) in [(approve.clone(), true), (deny.clone(), false)] {
        let agent = agent.clone();
        let tx = tx.clone();
        let row = row.clone();
        let approve_btn = approve.clone();
        let deny_btn = deny.clone();
        btn.connect_clicked(move |_| {
            approve_btn.set_sensitive(false);
            deny_btn.set_sensitive(false);
            row.set_visible(false);
            let agent = agent.clone();
            let tx = tx.clone();
            std::thread::spawn(move || {
                let outcome = agent.lock().unwrap_or_else(|e| e.into_inner()).confirm(ok);
                let _ = tx.send_blocking(UiEvent::AgentReply(outcome));
            });
        });
    }
}
