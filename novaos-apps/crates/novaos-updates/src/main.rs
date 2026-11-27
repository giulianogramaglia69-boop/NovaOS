//! novaos-updates — aggiornamenti di sistema e cambio desktop.
//!
//! Finestra libadwaita con quattro sezioni: sistema corrente, controllo
//! e applicazione degli aggiornamenti (rpm-ostree / bootc / Flatpak),
//! cambio desktop (`bootc switch`, fallback `rpm-ostree rebase`) e log.
//!
//! Tutte le operazioni lunghe girano in worker thread; i risultati
//! tornano in UI su un canale async-channel, come in nova-ai-ui
//! (adw::Application single-instance, worker + async_channel).
//!
//! Nota ambiente: su questa VM l'UI si può solo type-checkare
//! (niente GTK reale): `cargo check` con i .pc stub in
//! `$HOME/tmp/fake-pkgconfig`. Niente di tutto questo è mai stato
//! eseguito su un sistema NovaOS reale: i flussi pkexec, il check
//! bootc e il parsing di `flatpak remote-ls --updates` vanno
//! confermati su Fedora reale (vedi README.md).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use adw::prelude::*;
use gio::ApplicationFlags;
use novaos_common::notify::notify;
use novaos_common::{de, osrelease, ostree, system};
use novaos_updates::checks::{CheckReport, FlatpakCheck, Verdict};

/// Eventi dai worker thread verso il main loop GTK.
enum UiEvent {
    /// Una riga da aggiungere al log.
    Log(String),
    /// Esito del controllo aggiornamenti.
    CheckDone(CheckReport),
    /// Esito dell'aggiornamento di sistema.
    ApplyDone {
        success: bool,
        used_fallback: bool,
        message: String,
    },
    /// Esito dell'aggiornamento Flatpak, per ambito.
    FlatpakDone {
        system: ScopeOutcome,
        user: ScopeOutcome,
    },
    /// Esito di `bootc switch`.
    SwitchDone {
        de_label: String,
        success: bool,
        message: String,
    },
    /// Esito del fallback `rpm-ostree rebase`.
    RebaseDone { success: bool, message: String },
    /// Esito del riavvio (arriva solo se il riavvio NON è partito).
    RebootDone { success: bool, message: String },
}

/// Esito di un singolo comando in streaming.
struct ScopeOutcome {
    code: Option<i32>,
    error: Option<String>,
}

impl ScopeOutcome {
    fn succeeded(&self) -> bool {
        self.code == Some(0) && self.error.is_none()
    }

    fn describe(&self, what: &str) -> String {
        match (&self.code, &self.error) {
            (_, Some(error)) => format!("{what}: errore di avvio: {error}"),
            (Some(0), None) => format!("{what}: completato"),
            (Some(code), None) => format!("{what}: fallito (codice di uscita {code})"),
            (None, None) => format!("{what}: terminato da un segnale"),
        }
    }
}

/// Widget + stato condiviso della finestra (Rc, main thread only).
struct AppUi {
    win: adw::ApplicationWindow,
    log_buffer: gtk4::TextBuffer,
    log_scroll: gtk4::ScrolledWindow,
    check_btn: gtk4::Button,
    apply_btn: gtk4::Button,
    flatpak_btn: gtk4::Button,
    reboot_btn: gtk4::Button,
    fallback_btn: gtk4::Button,
    rpm_row: adw::ActionRow,
    bootc_row: adw::ActionRow,
    flatpak_row: adw::ActionRow,
    combined_label: gtk4::Label,
    switch_btns: HashMap<String, gtk4::Button>,
    current_de: Option<String>,
    /// `true` se esiste già un deployment staged (applica = riavvia).
    staged: bool,
    last_check: RefCell<Option<CheckReport>>,
    /// (etichetta DE, ref immagine) dell'ultimo switch fallito.
    pending_switch: RefCell<Option<(String, String)>>,
    busy: RefCell<bool>,
}

fn main() {
    let app = adw::Application::builder()
        .application_id("org.novaos.Updates")
        .flags(ApplicationFlags::empty())
        .build();
    app.connect_activate(build_ui);
    app.run();
}

// ---------------------------------------------------------------------------
// Costruzione UI
// ---------------------------------------------------------------------------

fn build_ui(app: &adw::Application) {
    // --- Dati di sistema (letture sincrone, comandi locali veloci) ---
    let os_info = osrelease::read().unwrap_or_default();
    let pretty = os_info
        .get("PRETTY_NAME")
        .cloned()
        .unwrap_or_else(|| "NovaOS".to_string());
    let variant = os_info.get("VARIANT_ID").cloned().unwrap_or_default();
    let current_de = osrelease::current_de(&os_info);
    let deployments = ostree::status();
    let (booted, staged, status_error) = match &deployments {
        Ok(deps) => (
            ostree::booted(deps).cloned(),
            ostree::staged(deps).cloned(),
            None,
        ),
        Err(error) => (None, None, Some(error.clone())),
    };
    let has_staged = staged.is_some();

    // --- Finestra ---
    let win = adw::ApplicationWindow::builder()
        .application(app)
        .title("Aggiornamenti NovaOS")
        .default_width(640)
        .default_height(720)
        .build();
    let header = adw::HeaderBar::new();
    let title = gtk4::Label::new(Some("Aggiornamenti NovaOS"));
    title.add_css_class("title");
    header.set_title_widget(Some(&title));
    win.set_titlebar(Some(&header));

    let content = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    content.set_margin_top(12);
    content.set_margin_bottom(12);
    content.set_margin_start(12);
    content.set_margin_end(12);

    // --- 1. Sistema corrente ---
    let system_group = adw::PreferencesGroup::builder()
        .title("Sistema corrente")
        .build();
    let system_subtitle = if variant.is_empty() {
        pretty.clone()
    } else {
        format!("{pretty} ({variant})")
    };
    let system_row = adw::ActionRow::builder()
        .title("Sistema")
        .subtitle(&system_subtitle)
        .build();
    system_group.add(&system_row);

    let booted_subtitle = match (&booted, &status_error) {
        (Some(dep), _) => format!(
            "Versione {} — {} — checksum {}",
            dep.label(),
            dep.date_string(),
            dep.short_checksum()
        ),
        (None, Some(error)) => format!("Non determinabile: {error}"),
        (None, None) => "Nessun deployment avviato trovato.".to_string(),
    };
    let booted_row = adw::ActionRow::builder()
        .title("Deployment avviato")
        .subtitle(&booted_subtitle)
        .build();
    system_group.add(&booted_row);

    let staged_reboot_btn = gtk4::Button::with_label("Riavvia per applicare");
    staged_reboot_btn.add_css_class("suggested-action");
    let staged_row = adw::ActionRow::builder()
        .title("Aggiornamento già preparato")
        .subtitle(match &staged {
            Some(dep) => format!(
                "In attesa di riavvio — versione {} del {}",
                dep.label(),
                dep.date_string()
            ),
            None => "In attesa di riavvio.".to_string(),
        })
        .build();
    staged_row.add_suffix(&staged_reboot_btn);
    staged_row.set_visible(has_staged);
    system_group.add(&staged_row);
    content.append(&system_group);

    // --- 2. Controllo aggiornamenti ---
    let check_group = adw::PreferencesGroup::builder()
        .title("Aggiornamenti")
        .build();
    let rpm_row = adw::ActionRow::builder()
        .title("rpm-ostree")
        .subtitle("Non ancora controllato.")
        .build();
    let bootc_row = adw::ActionRow::builder()
        .title("bootc")
        .subtitle("Non ancora controllato.")
        .build();
    let flatpak_row = adw::ActionRow::builder()
        .title("Flatpak")
        .subtitle("Non ancora controllato.")
        .build();
    check_group.add(&rpm_row);
    check_group.add(&bootc_row);
    check_group.add(&flatpak_row);
    content.append(&check_group);

    let combined_label = gtk4::Label::new(Some("Premi «Controlla aggiornamenti» per verificare."));
    combined_label.set_wrap(true);
    combined_label.set_xalign(0.0);
    content.append(&combined_label);

    let check_btn = gtk4::Button::with_label("Controlla aggiornamenti");
    check_btn.add_css_class("suggested-action");
    let apply_btn = gtk4::Button::with_label("Applica aggiornamento di sistema");
    apply_btn.set_sensitive(false);
    apply_btn.set_tooltip_text(Some(
        "Controlla prima gli aggiornamenti: si attiva solo se ce ne sono.",
    ));
    let flatpak_btn = gtk4::Button::with_label("Aggiorna Flatpak");
    let actions = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    actions.append(&check_btn);
    actions.append(&apply_btn);
    actions.append(&flatpak_btn);
    content.append(&actions);

    // --- 5. Cambio desktop ---
    let de_group = adw::PreferencesGroup::builder()
        .title("Cambio desktop")
        .description("Il cambio scarica la nuova immagine e si applica al riavvio.")
        .build();
    let mut switch_btns: HashMap<String, gtk4::Button> = HashMap::new();
    let mut de_buttons: Vec<(String, String, String, gtk4::Button)> = Vec::new();
    for (id, _name) in de::DES {
        let label = de::label(id);
        let image_ref = de::image_ref(id).unwrap_or_default();
        let row = adw::ActionRow::builder()
            .title(label)
            .subtitle(&image_ref)
            .build();
        if current_de.as_deref() == Some(id) {
            let badge = gtk4::Label::new(Some("In uso"));
            badge.add_css_class("success");
            row.add_suffix(&badge);
        } else {
            let btn = gtk4::Button::with_label(&format!("Passa a {label}"));
            row.add_suffix(&btn);
            de_buttons.push((id.to_string(), label.to_string(), image_ref, btn.clone()));
            switch_btns.insert(id.to_string(), btn);
        }
        de_group.add(&row);
    }
    content.append(&de_group);

    // --- 6. Log ---
    let log_heading = gtk4::Label::new(Some("Log"));
    log_heading.add_css_class("title-4");
    log_heading.set_xalign(0.0);
    content.append(&log_heading);
    let log_view = gtk4::TextView::builder()
        .editable(false)
        .cursor_visible(false)
        .monospace(true)
        .wrap_mode(gtk4::WrapMode::WordChar)
        .build();
    let log_buffer = log_view.buffer();
    let log_scroll = gtk4::ScrolledWindow::builder()
        .vexpand(true)
        .child(&log_view)
        .build();
    log_scroll.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
    log_scroll.set_min_content_height(180);
    content.append(&log_scroll);

    let reboot_btn = gtk4::Button::with_label("Riavvia ora");
    reboot_btn.add_css_class("suggested-action");
    reboot_btn.set_visible(has_staged);
    let fallback_btn = gtk4::Button::with_label("Prova con rpm-ostree rebase");
    fallback_btn.set_visible(false);
    let log_actions = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    log_actions.append(&reboot_btn);
    log_actions.append(&fallback_btn);
    content.append(&log_actions);

    let scroll = gtk4::ScrolledWindow::builder()
        .vexpand(true)
        .child(&content)
        .build();
    scroll.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
    win.set_content(Some(&scroll));

    let ui = Rc::new(AppUi {
        win: win.clone(),
        log_buffer,
        log_scroll,
        check_btn: check_btn.clone(),
        apply_btn: apply_btn.clone(),
        flatpak_btn: flatpak_btn.clone(),
        reboot_btn: reboot_btn.clone(),
        fallback_btn: fallback_btn.clone(),
        rpm_row: rpm_row.clone(),
        bootc_row: bootc_row.clone(),
        flatpak_row: flatpak_row.clone(),
        combined_label: combined_label.clone(),
        switch_btns,
        current_de,
        staged: has_staged,
        last_check: RefCell::new(None),
        pending_switch: RefCell::new(None),
        busy: RefCell::new(false),
    });

    // Canale worker -> UI.
    let (tx, rx) = async_channel::bounded::<UiEvent>(64);

    // --- Cablaggio pulsanti ---
    {
        let ui = ui.clone();
        let tx = tx.clone();
        check_btn.connect_clicked(move |_| {
            set_busy(&ui, true);
            spawn_check(tx.clone());
        });
    }
    {
        let ui = ui.clone();
        let tx = tx.clone();
        apply_btn.connect_clicked(move |_| {
            let ui = ui.clone();
            let tx = tx.clone();
            let win = ui.win.clone();
            confirm_dialog(
                &win,
                "Applicare l'aggiornamento di sistema?",
                "Il sistema scarica e prepara l'aggiornamento; si applica al riavvio.",
                "Applica",
                move || {
                    set_busy(&ui, true);
                    spawn_apply(tx.clone());
                },
            );
        });
    }
    {
        let ui = ui.clone();
        let tx = tx.clone();
        flatpak_btn.connect_clicked(move |_| {
            set_busy(&ui, true);
            spawn_flatpak_update(tx.clone());
        });
    }
    for btn in [&reboot_btn, &staged_reboot_btn] {
        let ui = ui.clone();
        let tx = tx.clone();
        btn.connect_clicked(move |_| {
            let ui = ui.clone();
            let tx = tx.clone();
            let win = ui.win.clone();
            confirm_dialog(
                &win,
                "Riavviare ora?",
                "Il sistema si riavvia subito. Salva il lavoro aperto prima di continuare.",
                "Riavvia",
                move || {
                    set_busy(&ui, true);
                    spawn_reboot(tx.clone());
                },
            );
        });
    }
    {
        let ui = ui.clone();
        let tx = tx.clone();
        fallback_btn.connect_clicked(move |_| {
            let Some((label, image_ref)) = ui.pending_switch.borrow().clone() else {
                return;
            };
            let ui = ui.clone();
            let tx = tx.clone();
            let win = ui.win.clone();
            confirm_dialog(
                &win,
                "Provare con rpm-ostree rebase?",
                &format!(
                    "Esegue: pkexec rpm-ostree rebase ostree-unverified-registry:{image_ref}\n\
                     Il cambio si applica al riavvio."
                ),
                "Prova il fallback",
                move || {
                    set_busy(&ui, true);
                    spawn_rebase(tx.clone(), label, image_ref);
                },
            );
        });
    }
    for (_id, label, image_ref, btn) in de_buttons {
        let ui = ui.clone();
        let tx = tx.clone();
        btn.connect_clicked(move |_| {
            let ui = ui.clone();
            let tx = tx.clone();
            let win = ui.win.clone();
            let body = format!(
                "Il sistema scarica l'immagine {image_ref} e la prepara; \
                 il cambio si applica al riavvio.\n\n\
                 I Flatpak e i file nella home restano. I pacchetti layered \
                 (installati con rpm-ostree install, es. codec/gaming/dev) \
                 potrebbero dover essere reinstallati dopo il cambio."
            );
            let heading = format!("Passare a {label}?");
            let confirm_label = format!("Passa a {label}");
            // Copie per la closure di conferma (FnOnce): la closure del
            // click è Fn e non può cedere `label` / `image_ref`.
            let label2 = label.clone();
            let ref2 = image_ref.clone();
            confirm_dialog(&win, &heading, &body, &confirm_label, move || {
                // Il ref serve al pulsante di fallback se lo switch fallisce.
                *ui.pending_switch.borrow_mut() = Some((label2.clone(), ref2.clone()));
                set_busy(&ui, true);
                spawn_switch(tx.clone(), label2, ref2);
            });
        });
    }

    // --- Ricevitore eventi (main loop) ---
    {
        let ui = ui.clone();
        glib::MainContext::default().spawn_local(async move {
            while let Ok(event) = rx.recv().await {
                handle_event(&ui, event);
            }
        });
    }

    append_log(
        &ui,
        "NovaOS Aggiornamenti — pronto. Premi «Controlla aggiornamenti» per iniziare.",
    );
    if has_staged {
        append_log(&ui, "Aggiornamento già preparato: riavvia per applicarlo.");
    }
    if let Some(error) = status_error {
        append_log(&ui, &format!("rpm-ostree status non riuscito: {error}"));
    }
    win.present();
}

// ---------------------------------------------------------------------------
// Gestione eventi in arrivo dai worker
// ---------------------------------------------------------------------------

fn handle_event(ui: &Rc<AppUi>, event: UiEvent) {
    match event {
        UiEvent::Log(line) => append_log(ui, &line),
        UiEvent::CheckDone(report) => {
            ui.rpm_row
                .set_subtitle(&format!("rpm-ostree — {}", report.rpm_ostree.describe()));
            ui.bootc_row
                .set_subtitle(&format!("bootc — {}", report.bootc.describe()));
            ui.flatpak_row.set_subtitle(&format!(
                "Sistema: {}; utente: {}",
                report.flatpak_system.detail(),
                report.flatpak_user.detail()
            ));
            ui.combined_label.set_text(&format!(
                "{} — {}",
                report.combined().label(),
                report.combined_detail()
            ));
            append_log(
                ui,
                &format!(
                    "Controllo completato: {} — {}",
                    report.combined().label(),
                    report.combined_detail()
                ),
            );
            if report.flatpak_available() {
                append_log(
                    ui,
                    "Aggiornamenti Flatpak disponibili: usa «Aggiorna Flatpak».",
                );
            }
            *ui.last_check.borrow_mut() = Some(report);
            set_busy(ui, false);
        }
        UiEvent::ApplyDone {
            success,
            used_fallback,
            message,
        } => {
            if success {
                if used_fallback {
                    append_log(ui, "Aggiornamento applicato via rpm-ostree (fallback).");
                }
                notify(
                    "NovaOS Aggiornamenti",
                    "Aggiornamento di sistema preparato. Riavvia per applicarlo.",
                );
                ui.reboot_btn.set_visible(true);
            }
            append_log(ui, &message);
            set_busy(ui, false);
        }
        UiEvent::FlatpakDone { system, user } => {
            append_log(ui, &system.describe("Flatpak (sistema)"));
            append_log(ui, &user.describe("Flatpak (utente)"));
            set_busy(ui, false);
        }
        UiEvent::SwitchDone {
            de_label,
            success,
            message,
        } => {
            if success {
                notify(
                    "NovaOS Aggiornamenti",
                    &format!("Passaggio a {de_label} preparato. Riavvia per applicarlo."),
                );
                ui.reboot_btn.set_visible(true);
                ui.fallback_btn.set_visible(false);
            } else {
                ui.fallback_btn.set_visible(true);
            }
            append_log(ui, &message);
            set_busy(ui, false);
        }
        UiEvent::RebaseDone { success, message } => {
            if success {
                notify(
                    "NovaOS Aggiornamenti",
                    "Cambio desktop preparato. Riavvia per applicarlo.",
                );
                ui.reboot_btn.set_visible(true);
                ui.fallback_btn.set_visible(false);
                *ui.pending_switch.borrow_mut() = None;
            }
            append_log(ui, &message);
            set_busy(ui, false);
        }
        UiEvent::RebootDone { success, message } => {
            if !success {
                append_log(ui, &message);
            }
            set_busy(ui, false);
        }
    }
}

// ---------------------------------------------------------------------------
// Helper UI
// ---------------------------------------------------------------------------

/// Aggiunge una riga al log e scorre in fondo.
fn append_log(ui: &AppUi, text: &str) {
    let buffer = &ui.log_buffer;
    let mut end = buffer.end_iter();
    buffer.insert(&mut end, &format!("{text}\n"));
    let adj = ui.log_scroll.vadjustment();
    adj.set_value(adj.upper() - adj.page_size());
}

/// Abilita/disabilita i pulsanti di azione mentre un lavoro è in corso.
/// "Applica" resta abilitato solo se l'ultimo controllo ha trovato
/// aggiornamenti di sistema e non c'è già uno staged.
fn set_busy(ui: &AppUi, busy: bool) {
    *ui.busy.borrow_mut() = busy;
    let system_available = ui
        .last_check
        .borrow()
        .as_ref()
        .map(CheckReport::system_available)
        .unwrap_or(false);
    ui.check_btn.set_sensitive(!busy);
    ui.apply_btn
        .set_sensitive(!busy && system_available && !ui.staged);
    ui.flatpak_btn.set_sensitive(!busy);
    for (id, btn) in &ui.switch_btns {
        let is_current = ui.current_de.as_deref() == Some(id.as_str());
        btn.set_sensitive(!busy && !is_current);
    }
    ui.fallback_btn.set_sensitive(!busy);
}

/// Azione di conferma in attesa (presa una sola volta dal dialogo).
type ConfirmAction = Rc<RefCell<Option<Box<dyn FnOnce()>>>>;

/// Dialogo di conferma libadwaita (AdwMessageDialog, libadwaita 1.5+).
/// `on_confirm` gira solo se l'utente preme il pulsante di conferma.
fn confirm_dialog(
    win: &adw::ApplicationWindow,
    heading: &str,
    body: &str,
    confirm_label: &str,
    on_confirm: impl FnOnce() + 'static,
) {
    let dialog = adw::MessageDialog::builder()
        .heading(heading)
        .body(body)
        .build();
    dialog.add_response("cancel", "Annulla");
    dialog.add_response("confirm", confirm_label);
    dialog.set_response_appearance("confirm", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    let action: ConfirmAction = Rc::new(RefCell::new(Some(Box::new(on_confirm))));
    dialog.connect_response(None, move |dialog: &adw::MessageDialog, response: &str| {
        if response == "confirm"
            && let Some(run) = action.borrow_mut().take()
        {
            run();
        }
        dialog.close();
    });
    dialog.set_transient_for(Some(win));
    dialog.present();
}

// ---------------------------------------------------------------------------
// Worker thread: controllo, upgrade, switch, reboot
// ---------------------------------------------------------------------------

fn log_line(tx: &async_channel::Sender<UiEvent>, text: &str) {
    let _ = tx.send_blocking(UiEvent::Log(text.to_string()));
}

/// Esegue un comando in streaming, inoltrando ogni riga al log.
fn run_streamed(
    tx: &async_channel::Sender<UiEvent>,
    argv: &[&str],
    privileged: bool,
) -> ScopeOutcome {
    let on_line = |line: &str| {
        let _ = tx.send_blocking(UiEvent::Log(line.to_string()));
    };
    match system::run_streaming(argv, privileged, &on_line) {
        Ok(code) => ScopeOutcome { code, error: None },
        Err(error) => ScopeOutcome {
            code: None,
            error: Some(error),
        },
    }
}

fn spawn_check(tx: async_channel::Sender<UiEvent>) {
    std::thread::spawn(move || {
        log_line(&tx, "Controllo aggiornamenti in corso…");
        let report = run_checks(&tx);
        let _ = tx.send_blocking(UiEvent::CheckDone(report));
    });
}

/// Controllo delle quattro sorgenti (nessuna rete diretta: solo i
/// comandi di sistema; gli errori diventano "Non determinabile").
fn run_checks(tx: &async_channel::Sender<UiEvent>) -> CheckReport {
    let rpm_ostree = if !system::command_exists("rpm-ostree") {
        Verdict::Unknown("rpm-ostree non installato".to_string())
    } else {
        match system::run(&["rpm-ostree", "upgrade", "--check"]) {
            Ok(out) => novaos_updates::checks::verdict_rpm_ostree(&out),
            Err(error) => Verdict::Unknown(error),
        }
    };
    log_line(tx, &format!("rpm-ostree: {}", rpm_ostree.describe()));

    let bootc = if !system::command_exists("bootc") {
        Verdict::Unknown("bootc non installato".to_string())
    } else {
        match system::run(&["bootc", "upgrade", "--check"]) {
            Ok(out) => novaos_updates::checks::verdict_bootc(&out.combined()),
            Err(error) => Verdict::Unknown(error),
        }
    };
    log_line(tx, &format!("bootc: {}", bootc.describe()));

    let flatpak_system = flatpak_check("system");
    let flatpak_user = flatpak_check("user");
    log_line(
        tx,
        &format!("Flatpak (sistema): {}", flatpak_system.detail()),
    );
    log_line(tx, &format!("Flatpak (utente): {}", flatpak_user.detail()));

    CheckReport {
        rpm_ostree,
        bootc,
        flatpak_system,
        flatpak_user,
    }
}

/// Un ambito Flatpak: `flatpak remote-ls --<scope> --updates`.
/// Se il comando fallisce, l'esito è "non determinabile" (TODO:
/// confermare l'output sul sistema reale) ma il pulsante
/// "Aggiorna Flatpak" resta comunque disponibile.
fn flatpak_check(scope: &str) -> FlatpakCheck {
    if !system::command_exists("flatpak") {
        return FlatpakCheck {
            apps: vec![],
            error: Some("flatpak non installato".to_string()),
        };
    }
    let flag = format!("--{scope}");
    match system::run(&["flatpak", "remote-ls", &flag, "--updates"]) {
        Ok(out) => {
            if out.success() {
                FlatpakCheck {
                    apps: novaos_updates::checks::parse_flatpak_updates(&out.stdout),
                    error: None,
                }
            } else {
                FlatpakCheck {
                    apps: vec![],
                    error: Some(format!(
                        "flatpak remote-ls fallito (codice {:?}): {}",
                        out.code,
                        out.stderr.trim()
                    )),
                }
            }
        }
        Err(error) => FlatpakCheck {
            apps: vec![],
            error: Some(error),
        },
    }
}

fn spawn_apply(tx: async_channel::Sender<UiEvent>) {
    std::thread::spawn(move || {
        log_line(&tx, "$ pkexec bootc upgrade");
        let mut used_fallback = false;
        let mut code: Option<i32> = None;
        let mut spawn_error: Option<String> = None;
        if system::command_exists("bootc") {
            match run_streamed(&tx, &["bootc", "upgrade"], true) {
                outcome if outcome.error.is_none() => code = outcome.code,
                outcome => spawn_error = outcome.error,
            }
        } else {
            log_line(&tx, "bootc non trovato: passo a rpm-ostree.");
            used_fallback = true;
        }
        if code != Some(0) {
            if !used_fallback {
                log_line(
                    &tx,
                    "bootc upgrade non riuscito: passo a pkexec rpm-ostree upgrade.",
                );
            }
            used_fallback = true;
            log_line(&tx, "$ pkexec rpm-ostree upgrade");
            match run_streamed(&tx, &["rpm-ostree", "upgrade"], true) {
                outcome if outcome.error.is_none() => code = outcome.code,
                outcome => spawn_error = outcome.error,
            }
        }
        let success = code == Some(0);
        let message = if success {
            "Aggiornamento di sistema preparato. Si applica al riavvio.".to_string()
        } else if let Some(error) = spawn_error {
            format!("Aggiornamento fallito: errore di avvio: {error}")
        } else {
            format!("Aggiornamento fallito (codice di uscita {code:?}). Vedi il log.")
        };
        log_line(&tx, &message);
        let _ = tx.send_blocking(UiEvent::ApplyDone {
            success,
            used_fallback,
            message,
        });
    });
}

fn spawn_flatpak_update(tx: async_channel::Sender<UiEvent>) {
    std::thread::spawn(move || {
        log_line(&tx, "$ pkexec flatpak update --system -y");
        let system = run_streamed(&tx, &["flatpak", "update", "--system", "-y"], true);
        log_line(&tx, &system.describe("Flatpak (sistema)"));
        log_line(&tx, "$ flatpak update --user -y");
        let user = run_streamed(&tx, &["flatpak", "update", "--user", "-y"], false);
        log_line(&tx, &user.describe("Flatpak (utente)"));
        let _ = tx.send_blocking(UiEvent::FlatpakDone { system, user });
    });
}

fn spawn_switch(tx: async_channel::Sender<UiEvent>, de_label: String, image_ref: String) {
    std::thread::spawn(move || {
        log_line(&tx, &format!("$ pkexec bootc switch {image_ref}"));
        let outcome = run_streamed(&tx, &["bootc", "switch", image_ref.as_str()], true);
        let success = outcome.succeeded();
        let message = if success {
            format!("Passaggio a {de_label} preparato. Si applica al riavvio.")
        } else {
            format!(
                "bootc switch fallito: {}. Puoi provare il fallback rpm-ostree rebase.",
                outcome.describe("bootc switch")
            )
        };
        log_line(&tx, &message);
        let _ = tx.send_blocking(UiEvent::SwitchDone {
            de_label,
            success,
            message,
        });
    });
}

fn spawn_rebase(tx: async_channel::Sender<UiEvent>, de_label: String, image_ref: String) {
    std::thread::spawn(move || {
        let target = format!("ostree-unverified-registry:{image_ref}");
        log_line(&tx, &format!("$ pkexec rpm-ostree rebase {target}"));
        let outcome = run_streamed(&tx, &["rpm-ostree", "rebase", target.as_str()], true);
        let success = outcome.succeeded();
        let message = if success {
            format!(
                "Passaggio a {de_label} preparato (via rpm-ostree rebase). Si applica al riavvio."
            )
        } else {
            format!(
                "rpm-ostree rebase fallito: {}.",
                outcome.describe("rpm-ostree rebase")
            )
        };
        log_line(&tx, &message);
        let _ = tx.send_blocking(UiEvent::RebaseDone { success, message });
    });
}

fn spawn_reboot(tx: async_channel::Sender<UiEvent>) {
    std::thread::spawn(move || {
        log_line(&tx, "$ pkexec systemctl reboot");
        let outcome = run_streamed(&tx, &["systemctl", "reboot"], true);
        // Se il riavvio parte, il processo muore col sistema: questo
        // evento arriva solo se il comando fallisce.
        let success = outcome.succeeded();
        let message = if success {
            "Riavvio in corso…".to_string()
        } else {
            format!("Riavvio fallito: {}.", outcome.describe("systemctl reboot"))
        };
        let _ = tx.send_blocking(UiEvent::RebootDone { success, message });
    });
}
