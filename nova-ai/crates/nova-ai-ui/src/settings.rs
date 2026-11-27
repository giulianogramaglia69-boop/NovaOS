//! Settings window: provider/model info + the "Ehi Nova" wake-word
//! switch (opt-in, immediate effect) with model download.
//!
//! The wake-word section is behind the `wakeword` cargo feature; a
//! build without it shows a note instead of the switch. Memory,
//! proactive watchers and the read-only calendar live here too (v3).

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;

use crate::AppState;

pub fn show_settings(app: &adw::Application, state: Rc<RefCell<AppState>>) {
    let win = adw::ApplicationWindow::builder()
        .application(app)
        .title("NovaAI — Impostazioni")
        .default_width(420)
        .default_height(480)
        .build();

    let header = adw::HeaderBar::new();
    let title = gtk4::Label::new(Some("✦ Impostazioni NovaAI"));
    title.add_css_class("title");
    header.set_title_widget(Some(&title));
    win.set_titlebar(Some(&header));

    let list = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    list.set_margin_top(16);
    list.set_margin_bottom(16);
    list.set_margin_start(16);
    list.set_margin_end(16);

    let (provider, model) = {
        let cfg = &state.borrow().config;
        (
            cfg.provider_kind().display_name().to_string(),
            cfg.model.clone(),
        )
    };
    let info = gtk4::Label::new(Some(&format!("Provider: {provider}\nModello: {model}")));
    info.set_xalign(0.0);
    list.append(&info);

    #[cfg(feature = "wakeword")]
    build_wake_section(&list, state.clone());
    #[cfg(not(feature = "wakeword"))]
    {
        let note = gtk4::Label::new(Some(
            "La wake word “Ehi Nova” non è disponibile in questa build (manca il supporto nativo Vosk).",
        ));
        note.set_wrap(true);
        note.set_xalign(0.0);
        list.append(&note);
    }

    build_tts_section(&list, state.clone());
    build_memory_section(&list, state.clone());
    build_watchers_section(&list, state.clone());
    build_calendar_section(&list, state.clone());

    let scroll = gtk4::ScrolledWindow::builder()
        .vexpand(true)
        .child(&list)
        .build();
    win.set_content(Some(&scroll));
    win.present();
}

/// Persist the wake-word toggle in config.
#[cfg(feature = "wakeword")]
fn persist_wake(state: &Rc<RefCell<AppState>>, on: bool, status: &gtk4::Label) {
    let mut cfg = state.borrow().config.clone();
    cfg.wake_word = on;
    match cfg.save() {
        Ok(()) => {
            state.borrow_mut().config = cfg;
        }
        Err(e) => status.set_text(&format!("Impossibile salvare la configurazione: {e}")),
    }
}

/// Memory section (v3): persistent facts + last-session summary.
fn build_memory_section(list: &gtk4::Box, state: Rc<RefCell<AppState>>) {
    let title = gtk4::Label::new(Some("Memoria"));
    title.add_css_class("title-4");
    title.set_xalign(0.0);
    list.append(&title);

    let explain = gtk4::Label::new(Some(
        "Se attiva, NovaAI ricorda i fatti che le dici (“ricorda che…”) e il riassunto dell'ultima sessione, e li usa nelle risposte. I dati restano sul tuo PC, in ~/.local/share/nova-ai/memory/.",
    ));
    explain.set_wrap(true);
    explain.set_xalign(0.0);
    list.append(&explain);

    let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    row.append(&gtk4::Label::new(Some("Ricorda fatti e riassunti")));
    let spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    row.append(&spacer);
    let switch = gtk4::Switch::new();
    switch.set_active(state.borrow().config.memory);
    switch.set_halign(gtk4::Align::End);
    row.append(&switch);
    list.append(&row);

    let status = gtk4::Label::new(None);
    status.set_wrap(true);
    status.set_xalign(0.0);
    list.append(&status);

    switch.connect_active_notify(move |sw| {
        let on = sw.is_active();
        let mut cfg = state.borrow().config.clone();
        cfg.memory = on;
        match cfg.save() {
            Ok(()) => {
                state.borrow_mut().config = cfg;
                status.set_text(if on {
                    "Memoria attiva."
                } else {
                    "Memoria disattivata (i fatti già salvati restano sul disco)."
                });
            }
            Err(e) => status.set_text(&format!("Impossibile salvare la configurazione: {e}")),
        }
    });
}

/// Proactive watchers section (v3): hourly systemd user timer that
/// runs `nova-ai --check` (updates, battery, disk) with desktop
/// notifications.
fn build_watchers_section(list: &gtk4::Box, state: Rc<RefCell<AppState>>) {
    use nova_ai_core::watchers::{install_timer, remove_timer, timer_installed};

    let title = gtk4::Label::new(Some("Avvisi proattivi"));
    title.add_css_class("title-4");
    title.set_xalign(0.0);
    list.append(&title);

    let explain = gtk4::Label::new(Some(
        "Se attivi, NovaAI controlla ogni ora aggiornamenti di sistema disponibili, batteria bassa (sotto il 20% in scarica) e disco quasi pieno (oltre il 90%), e ti avvisa con una notifica. Il controllo gira con un timer systemd utente (nova-ai-watch.timer).",
    ));
    explain.set_wrap(true);
    explain.set_xalign(0.0);
    list.append(&explain);

    let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    row.append(&gtk4::Label::new(Some("Avvisi ogni ora")));
    let spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    row.append(&spacer);
    let switch = gtk4::Switch::new();
    switch.set_active(state.borrow().config.watchers);
    switch.set_halign(gtk4::Align::End);
    row.append(&switch);
    list.append(&row);

    let status = gtk4::Label::new(None);
    status.set_wrap(true);
    status.set_xalign(0.0);
    status.set_text(if timer_installed() {
        "Timer attivo (controllo ogni ora)."
    } else {
        "Timer non attivo."
    });
    list.append(&status);

    switch.connect_active_notify(move |sw| {
        let on = sw.is_active();
        if !on {
            let _ = remove_timer();
            let mut cfg = state.borrow().config.clone();
            cfg.watchers = false;
            match cfg.save() {
                Ok(()) => {
                    state.borrow_mut().config = cfg;
                    status.set_text("Avvisi proattivi disattivati.");
                }
                Err(e) => status.set_text(&format!("Impossibile salvare la configurazione: {e}")),
            }
            return;
        }
        match install_timer() {
            Ok(()) => {
                let mut cfg = state.borrow().config.clone();
                cfg.watchers = true;
                match cfg.save() {
                    Ok(()) => {
                        state.borrow_mut().config = cfg;
                        status.set_text("Timer attivo: controllo ogni ora.");
                    }
                    Err(e) => {
                        status.set_text(&format!("Impossibile salvare la configurazione: {e}"))
                    }
                }
            }
            Err(e) => {
                sw.set_active(false);
                status.set_text(&format!("Impossibile attivare il timer: {e}"));
            }
        }
    });
}

/// Calendar section (v3): read-only ICS source (URL or local file).
fn build_calendar_section(list: &gtk4::Box, state: Rc<RefCell<AppState>>) {
    let title = gtk4::Label::new(Some("Calendario (sola lettura)"));
    title.add_css_class("title-4");
    title.set_xalign(0.0);
    list.append(&title);

    let explain = gtk4::Label::new(Some(
        "Link privato ICS del calendario (Google Calendar: Impostazioni → Integra calendario → link iCalendar privato) oppure un file .ics locale. NovaAI legge solo gli eventi: niente scrittura, niente email. Il link viene scaricato al massimo ogni 15 minuti.",
    ));
    explain.set_wrap(true);
    explain.set_xalign(0.0);
    list.append(&explain);

    let url_entry = gtk4::Entry::new();
    url_entry.set_placeholder_text(Some(
        "https://calendar.google.com/calendar/ical/…/private-….ics",
    ));
    url_entry.set_text(&state.borrow().config.calendar_url);
    list.append(&gtk4::Label::new(Some("URL ICS")));
    list.append(&url_entry);

    let file_entry = gtk4::Entry::new();
    file_entry.set_placeholder_text(Some("~/.local/share/nova-ai/calendario.ics"));
    file_entry.set_text(&state.borrow().config.calendar_file);
    list.append(&gtk4::Label::new(Some("oppure file .ics locale")));
    list.append(&file_entry);

    let status = gtk4::Label::new(None);
    status.set_wrap(true);
    status.set_xalign(0.0);
    list.append(&status);

    let save_btn = gtk4::Button::with_label("Salva calendario");
    list.append(&save_btn);
    {
        let url_entry = url_entry.clone();
        let file_entry = file_entry.clone();
        let status = status.clone();
        save_btn.connect_clicked(move |_| {
            let mut cfg = state.borrow().config.clone();
            cfg.calendar_url = url_entry.text().trim().to_string();
            cfg.calendar_file = file_entry.text().trim().to_string();
            match cfg.save() {
                Ok(()) => {
                    state.borrow_mut().config = cfg;
                    status.set_text("Calendario salvato.");
                }
                Err(e) => status.set_text(&format!("Impossibile salvare la configurazione: {e}")),
            }
        });
    }
}

#[cfg(feature = "wakeword")]
fn build_wake_section(list: &gtk4::Box, state: Rc<RefCell<AppState>>) {
    use nova_ai_core::wakeword::{download_model, model_present};

    use crate::DownloadEvent;
    use crate::wake;

    let title = gtk4::Label::new(Some("Ehi Nova (wake word)"));
    title.add_css_class("title-4");
    title.set_xalign(0.0);
    list.append(&title);

    let explain = gtk4::Label::new(Some(
        "Se attivo, il microfono resta in ascolto in locale: il riconoscimento della parola “Ehi Nova” avviene tutto sul dispositivo e l'audio non lascia mai NovaOS. Solo il comando che dici dopo l'attivazione viene inviato all'API Whisper del provider per la trascrizione.",
    ));
    explain.set_wrap(true);
    explain.set_xalign(0.0);
    list.append(&explain);

    // Switch row: label left, switch right.
    let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    row.append(&gtk4::Label::new(Some("Attiva “Ehi Nova”")));
    let spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    row.append(&spacer);
    let switch = gtk4::Switch::new();
    switch.set_active(state.borrow().config.wake_word);
    switch.set_halign(gtk4::Align::End);
    row.append(&switch);
    list.append(&row);

    let status = gtk4::Label::new(None);
    status.set_wrap(true);
    status.set_xalign(0.0);
    if model_present() {
        status.set_text("Modello scaricato.");
    } else {
        status.set_text("Modello non scaricato (47 MB, solo al primo uso).");
    }
    list.append(&status);

    let progress = gtk4::ProgressBar::new();
    progress.set_visible(false);
    list.append(&progress);

    let download_btn = gtk4::Button::with_label("Scarica il modello (47 MB)");
    download_btn.set_visible(!model_present());
    list.append(&download_btn);
    {
        let switch = switch.clone();
        download_btn.connect_clicked(move |_| {
            switch.set_active(true);
        });
    }

    // Toggling ON downloads the model first (with progress), then
    // starts the listener. Toggling OFF stops it immediately.
    {
        let state = state.clone();
        let status = status.clone();
        let progress = progress.clone();
        let download_btn = download_btn.clone();
        switch.connect_active_notify(move |sw| {
            let on = sw.is_active();
            if !on {
                wake::stop_listener(&state);
                persist_wake(&state, false, &status);
                status.set_text("“Ehi Nova” disattivato.");
                return;
            }
            if model_present() {
                match wake::start_listener(&state) {
                    Ok(()) => {
                        persist_wake(&state, true, &status);
                        status.set_text("“Ehi Nova” attivo: di' “Ehi Nova” e parla.");
                    }
                    Err(e) => {
                        // set_active(false) first: its handler runs
                        // synchronously and would overwrite the status.
                        sw.set_active(false);
                        status.set_text(&format!("Impossibile avviare l'ascolto: {e}"));
                    }
                }
                return;
            }
            // First use: download the model (~47 MB) with progress.
            status.set_text("Scarico il modello “Ehi Nova” (47 MB)…");
            progress.set_visible(true);
            progress.set_fraction(0.0);
            sw.set_sensitive(false);
            let (tx, rx) = async_channel::bounded::<DownloadEvent>(16);
            let state2 = state.clone();
            let status2 = status.clone();
            let progress2 = progress.clone();
            let download_btn2 = download_btn.clone();
            let switch2 = sw.clone();
            glib::MainContext::default().spawn_local(async move {
                while let Ok(ev) = rx.recv().await {
                    match ev {
                        DownloadEvent::Progress(got, total) => {
                            if total > 0 {
                                progress2.set_fraction(got as f64 / total as f64);
                            }
                        }
                        DownloadEvent::Done(Ok(_)) => {
                            progress2.set_visible(false);
                            download_btn2.set_visible(false);
                            switch2.set_sensitive(true);
                            match wake::start_listener(&state2) {
                                Ok(()) => {
                                    persist_wake(&state2, true, &status2);
                                    status2
                                        .set_text("Modello scaricato — “Ehi Nova” attivo.");
                                }
                                Err(e) => {
                                    // set_active(false) first: its handler
                                    // runs synchronously and would
                                    // overwrite the status below.
                                    switch2.set_active(false);
                                    status2.set_text(&format!(
                                        "Modello scaricato ma ascolto fallito: {e}"
                                    ));
                                }
                            }
                            break;
                        }
                        DownloadEvent::Done(Err(e)) => {
                            progress2.set_visible(false);
                            switch2.set_sensitive(true);
                            switch2.set_active(false);
                            status2.set_text(&format!(
                                "Download fallito: {e} Riprova quando vuoi: la chat e il pulsante vocale restano funzionanti."
                            ));
                            break;
                        }
                    }
                }
            });
            std::thread::spawn(move || {
                let r = download_model(|got, total| {
                    let _ = tx.send_blocking(DownloadEvent::Progress(got, total));
                });
                let _ = tx.send_blocking(DownloadEvent::Done(r));
            });
        });
    }
}

/// Persist the TTS toggle in config.
fn persist_tts(state: &Rc<RefCell<AppState>>, on: bool, status: &gtk4::Label) {
    let mut cfg = state.borrow().config.clone();
    cfg.tts = on;
    match cfg.save() {
        Ok(()) => {
            state.borrow_mut().config = cfg;
        }
        Err(e) => status.set_text(&format!("Impossibile salvare la configurazione: {e}")),
    }
}

/// TTS section: spoken answers with offline Piper (Italian voice).
/// Mirrors the wake-word section: toggling ON downloads Piper + the
/// voice first (~55 MB) with progress, then activates.
fn build_tts_section(list: &gtk4::Box, state: Rc<RefCell<AppState>>) {
    use nova_ai_core::tts::{download_all, tts_ready, tts_status};

    use crate::DownloadEvent;

    let title = gtk4::Label::new(Some("Risposte parlate (voce)"));
    title.add_css_class("title-4");
    title.set_xalign(0.0);
    list.append(&title);

    let explain = gtk4::Label::new(Some(
        "Se attivo, NovaAI legge ad alta voce le risposte con una voce italiana offline (Piper, voce Riccardo). La sintesi avviene sul dispositivo; al primo uso scarica il sintetizzatore e la voce (circa 55 MB). Le risposte lunghe vengono lette solo in parte (primi 800 caratteri).",
    ));
    explain.set_wrap(true);
    explain.set_xalign(0.0);
    list.append(&explain);

    // Switch row: label left, switch right.
    let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    row.append(&gtk4::Label::new(Some("Leggi ad alta voce le risposte")));
    let spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    row.append(&spacer);
    let switch = gtk4::Switch::new();
    switch.set_active(state.borrow().config.tts);
    switch.set_halign(gtk4::Align::End);
    row.append(&switch);
    list.append(&row);

    let status = gtk4::Label::new(None);
    status.set_wrap(true);
    status.set_xalign(0.0);
    status.set_text(&tts_status());
    list.append(&status);

    let progress = gtk4::ProgressBar::new();
    progress.set_visible(false);
    list.append(&progress);

    let download_btn = gtk4::Button::with_label("Scarica voce e Piper (~55 MB)");
    download_btn.set_visible(!tts_ready());
    list.append(&download_btn);
    {
        let switch = switch.clone();
        download_btn.connect_clicked(move |_| {
            switch.set_active(true);
        });
    }

    // Toggling ON downloads Piper + voice first (with progress),
    // then activates. Toggling OFF stops it immediately.
    {
        let state = state.clone();
        let status = status.clone();
        let progress = progress.clone();
        let download_btn = download_btn.clone();
        switch.connect_active_notify(move |sw| {
            let on = sw.is_active();
            if !on {
                persist_tts(&state, false, &status);
                status.set_text("Risposte parlate disattivate.");
                return;
            }
            if tts_ready() {
                persist_tts(&state, true, &status);
                status.set_text("Risposte parlate attive.");
                return;
            }
            // First use: download Piper + voice (~55 MB) with progress.
            status.set_text("Scarico Piper e la voce italiana (~55 MB)…");
            progress.set_visible(true);
            progress.set_fraction(0.0);
            sw.set_sensitive(false);
            let (tx, rx) = async_channel::bounded::<DownloadEvent>(16);
            let state2 = state.clone();
            let status2 = status.clone();
            let progress2 = progress.clone();
            let download_btn2 = download_btn.clone();
            let switch2 = sw.clone();
            glib::MainContext::default().spawn_local(async move {
                while let Ok(ev) = rx.recv().await {
                    match ev {
                        DownloadEvent::Progress(got, total) => {
                            if total > 0 {
                                progress2.set_fraction(got as f64 / total as f64);
                            }
                        }
                        DownloadEvent::Done(Ok(_)) => {
                            progress2.set_visible(false);
                            download_btn2.set_visible(false);
                            switch2.set_sensitive(true);
                            persist_tts(&state2, true, &status2);
                            status2.set_text("Voce scaricata — risposte parlate attive.");
                            break;
                        }
                        DownloadEvent::Done(Err(e)) => {
                            progress2.set_visible(false);
                            switch2.set_sensitive(true);
                            switch2.set_active(false);
                            status2.set_text(&format!(
                                "Download fallito: {e} Riprova quando vuoi: la chat resta funzionante."
                            ));
                            break;
                        }
                    }
                }
            });
            std::thread::spawn(move || {
                let r = download_all(|got, total| {
                    let _ = tx.send_blocking(DownloadEvent::Progress(got, total));
                });
                let _ = tx.send_blocking(DownloadEvent::Done(r));
            });
        });
    }
}
