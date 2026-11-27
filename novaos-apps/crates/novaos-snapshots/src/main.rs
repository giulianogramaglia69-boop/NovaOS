//! novaos-snapshots — elenco dei deployment rpm-ostree come snapshot.
//!
//! Finestra libadwaita (`org.novaos.Snapshots`, single-instance):
//! una riga per deployment con badge (Avviato / In attesa di riavvio /
//! Pinnato) e azioni Avvia / Pin / Rimuovi pin, più "Torna al
//! precedente" (rollback) e "Riavvia ora" dopo deploy/rollback riusciti.
//!
//! Tutte le operazioni privilegiate passano da pkexec e girano in
//! worker thread; i risultati tornano in UI su un `async_channel`
//! (stesso pattern di `nova-ai-ui`: worker thread + canale verso il
//! main loop). L'output dei comandi finisce nel registro in fondo.
//!
//! La UI si può solo type-checkare su questa VM (niente GTK reale):
//! vedi README.md.

use novaos_snapshots::rows;

use std::cell::RefCell;
use std::rc::Rc;

use adw::Application;
use adw::prelude::*;
use gio::ApplicationFlags;
use novaos_common::notify::notify;
use novaos_common::ostree::{self, Deployment};
use novaos_common::system::{self, run_streaming};

/// Operazione privilegiata in corso (per log, notifiche e UI).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpKind {
    Deploy,
    Pin,
    Unpin,
    Rollback,
    Reboot,
}

impl OpKind {
    /// Nome dell'operazione (per i messaggi di errore).
    fn label(self) -> &'static str {
        match self {
            OpKind::Deploy => "Avvio dello snapshot",
            OpKind::Pin => "Pin",
            OpKind::Unpin => "Rimozione del pin",
            OpKind::Rollback => "Rollback",
            OpKind::Reboot => "Riavvio",
        }
    }

    /// Riga di log a operazione riuscita.
    fn done_label(self) -> &'static str {
        match self {
            OpKind::Deploy => "Deploy riuscito.",
            OpKind::Pin => "Pin applicato.",
            OpKind::Unpin => "Pin rimosso.",
            OpKind::Rollback => "Rollback riuscito.",
            OpKind::Reboot => "Riavvio avviato.",
        }
    }

    /// Testo della notifica a operazione riuscita.
    fn success_note(self) -> &'static str {
        match self {
            OpKind::Deploy => "Snapshot pronto: riavvia per applicarlo.",
            OpKind::Rollback => "Rollback pronto: riavvia per applicarlo.",
            OpKind::Pin => "Snapshot pinnato.",
            OpKind::Unpin => "Pin rimosso.",
            OpKind::Reboot => "Riavvio in corso.",
        }
    }
}

/// Eventi dai worker thread verso il main loop.
enum UiEvent {
    /// `rpm-ostree status` completato (lista o errore).
    Loaded(Result<Vec<Deployment>, String>),
    /// Una riga di output di un comando privilegiato.
    LogLine(String),
    /// Un'operazione privilegiata è terminata.
    OpFinished {
        kind: OpKind,
        code: Option<i32>,
        error: Option<String>,
    },
    /// L'indice ostree del deployment non è stato trovato:
    /// il pin non viene eseguito (mai indovinare l'indice).
    PinIndexNotFound { checksum: String },
}

/// Stato condiviso della finestra.
struct Ui {
    win: adw::ApplicationWindow,
    tx: async_channel::Sender<UiEvent>,
    list: gtk4::ListBox,
    status_label: gtk4::Label,
    log_buffer: gtk4::TextBuffer,
    log_scroll: gtk4::ScrolledWindow,
    refresh_btn: gtk4::Button,
    rollback_btn: gtk4::Button,
    reboot_btn: gtk4::Button,
    deployments: Vec<Deployment>,
    busy: bool,
}

/// Aggiorna la sensibilità dei pulsanti in base a `busy` e alla lista.
fn set_busy(ui: &Rc<RefCell<Ui>>, busy: bool) {
    let mut guard = ui.borrow_mut();
    guard.busy = busy;
    guard.refresh_btn.set_sensitive(!busy);
    guard.reboot_btn.set_sensitive(!busy);
    guard
        .rollback_btn
        .set_sensitive(!busy && rows::can_rollback(&guard.deployments));
}

/// Mostra (o nasconde, se vuoto) il messaggio di stato sopra la lista.
fn set_status(ui: &Rc<RefCell<Ui>>, text: &str) {
    let guard = ui.borrow();
    guard.status_label.set_text(text);
    guard.status_label.set_visible(!text.is_empty());
}

/// Aggiunge una riga al registro e scorre in fondo.
fn append_log(ui: &Rc<RefCell<Ui>>, line: &str) {
    let guard = ui.borrow();
    guard
        .log_buffer
        .insert(&mut guard.log_buffer.end_iter(), &format!("{line}\n"));
    let adj = guard.log_scroll.vadjustment();
    adj.set_value(adj.upper() - adj.page_size());
}

/// Ricarica la lista da `rpm-ostree status --json` (worker thread).
fn refresh(ui: &Rc<RefCell<Ui>>) {
    if ui.borrow().busy {
        return;
    }
    set_busy(ui, true);
    set_status(ui, "Caricamento degli snapshot…");
    append_log(ui, "$ rpm-ostree status --json");
    let tx = ui.borrow().tx.clone();
    std::thread::spawn(move || {
        let result = if !system::command_exists("rpm-ostree") {
            Err("rpm-ostree non trovato (comando assente)".to_string())
        } else {
            ostree::status()
        };
        let _ = tx.send_blocking(UiEvent::Loaded(result));
    });
}

/// Ricostruisce le righe della lista dai deployment correnti.
fn rebuild_list(ui: &Rc<RefCell<Ui>>) {
    let list = ui.borrow().list.clone();
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
    for dep in ui.borrow().deployments.clone() {
        list.append(&build_row(ui, &dep));
    }
}

/// Una riga deployment: titolo, sottotitolo, badge e pulsanti azione.
fn build_row(ui: &Rc<RefCell<Ui>>, dep: &Deployment) -> gtk4::ListBoxRow {
    let view = rows::row_view(dep);

    let title = gtk4::Label::new(Some(&view.title));
    title.set_xalign(0.0);
    title.add_css_class("title-4");
    let subtitle = gtk4::Label::new(Some(&view.subtitle));
    subtitle.set_xalign(0.0);
    subtitle.add_css_class("dim-label");
    subtitle.set_wrap(true);

    let text = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
    text.set_hexpand(true);
    text.append(&title);
    text.append(&subtitle);
    if !view.badges.is_empty() {
        let badges = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        badges.set_margin_top(4);
        for badge in &view.badges {
            let label = gtk4::Label::new(Some(badge.label()));
            label.add_css_class("snap-badge");
            badges.append(&label);
        }
        text.append(&badges);
    }

    let row_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    row_box.set_margin_top(8);
    row_box.set_margin_bottom(8);
    row_box.set_margin_start(8);
    row_box.set_margin_end(8);
    row_box.append(&text);

    let win = ui.borrow().win.clone();

    if view.can_deploy {
        let dep = dep.clone();
        let ui = ui.clone();
        let btn = gtk4::Button::with_label("Avvia");
        btn.add_css_class("suggested-action");
        btn.connect_clicked(move |_| {
            if ui.borrow().busy {
                return;
            }
            let checksum = dep.checksum.clone();
            let label = dep.label();
            let ui = ui.clone();
            confirm(
                &win,
                "Avvia questo snapshot?",
                &format!("Il sistema si avvierà con lo snapshot «{label}» al prossimo riavvio."),
                "Avvia",
                false,
                move || {
                    run_privileged_streaming(
                        &ui,
                        OpKind::Deploy,
                        vec!["rpm-ostree".to_string(), "deploy".to_string(), checksum],
                    );
                },
            );
        });
        row_box.append(&btn);
    }

    {
        let dep = dep.clone();
        let ui = ui.clone();
        let btn = gtk4::Button::with_label(view.pin_label);
        btn.set_tooltip_text(Some(if dep.pinned {
            "Rimuove la protezione dalla pulizia automatica"
        } else {
            "Protegge lo snapshot dalla pulizia automatica"
        }));
        btn.connect_clicked(move |_| {
            start_pin(&ui, &dep, dep.pinned);
        });
        row_box.append(&btn);
    }

    let row = gtk4::ListBoxRow::new();
    row.set_child(Some(&row_box));
    row
}

/// Dialogo di conferma (adw::AlertDialog). `on_confirm` gira solo se
/// l'utente sceglie la risposta di conferma.
fn confirm(
    parent: &adw::ApplicationWindow,
    heading: &str,
    body: &str,
    confirm_label: &str,
    destructive: bool,
    on_confirm: impl FnOnce() + 'static,
) {
    let dialog = adw::AlertDialog::builder().build();
    dialog.set_heading(Some(heading));
    dialog.set_body(body);
    dialog.add_response("annulla", "Annulla");
    dialog.add_response("conferma", confirm_label);
    dialog.set_default_response(Some("annulla"));
    dialog.set_close_response("annulla");
    dialog.set_response_appearance(
        "conferma",
        if destructive {
            adw::ResponseAppearance::Destructive
        } else {
            adw::ResponseAppearance::Suggested
        },
    );
    dialog.choose(parent, None::<&gio::Cancellable>, move |resp| {
        if resp.as_str() == "conferma" {
            on_confirm();
        }
    });
}

/// Dialogo di errore a risposta singola.
fn show_error(parent: &adw::ApplicationWindow, heading: &str, body: &str) {
    let dialog = adw::AlertDialog::builder().build();
    dialog.set_heading(Some(heading));
    dialog.set_body(body);
    dialog.add_response("chiudi", "Chiudi");
    dialog.set_default_response(Some("chiudi"));
    dialog.set_close_response("chiudi");
    dialog.choose(parent, None::<&gio::Cancellable>, |_| {});
}

/// Esegue `argv` con pkexec in streaming (worker thread): ogni riga
/// va nel registro, l'esito torna come `OpFinished`.
fn run_privileged_streaming(ui: &Rc<RefCell<Ui>>, kind: OpKind, argv: Vec<String>) {
    if ui.borrow().busy {
        return;
    }
    set_busy(ui, true);
    append_log(ui, &format!("$ pkexec {}", argv.join(" ")));
    let tx = ui.borrow().tx.clone();
    std::thread::spawn(move || {
        let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
        let tx_lines = tx.clone();
        let on_line = |line: &str| {
            let _ = tx_lines.send_blocking(UiEvent::LogLine(line.to_string()));
        };
        let result = run_streaming(&refs, true, &on_line);
        let (code, error) = match result {
            Ok(code) => (code, None),
            Err(err) => (None, Some(err)),
        };
        let _ = tx.send_blocking(UiEvent::OpFinished { kind, code, error });
    });
}

/// Legge `ostree admin status` (prima senza privilegi, poi con
/// pkexec) per risolvere l'indice del deployment da pinnare.
fn read_admin_status(tx: &async_channel::Sender<UiEvent>) -> Option<String> {
    match system::run(&["ostree", "admin", "status"]) {
        Ok(out) if out.success() => return Some(out.stdout),
        Ok(out) => {
            let _ = tx.send_blocking(UiEvent::LogLine(format!(
                "`ostree admin status` senza privilegi fallito (codice {:?}), riprovo con pkexec…",
                out.code
            )));
        }
        Err(err) => {
            let _ = tx.send_blocking(UiEvent::LogLine(format!(
                "`ostree admin status` senza privilegi fallito: {err}, riprovo con pkexec…"
            )));
        }
    }
    match system::run_privileged(&["ostree", "admin", "status"]) {
        Ok(out) if out.success() => Some(out.stdout),
        Ok(out) => {
            let _ = tx.send_blocking(UiEvent::LogLine(format!(
                "`ostree admin status` con pkexec fallito (codice {:?}).",
                out.code
            )));
            None
        }
        Err(err) => {
            let _ = tx.send_blocking(UiEvent::LogLine(format!(
                "`ostree admin status` con pkexec fallito: {err}."
            )));
            None
        }
    }
}

/// Pin / rimozione del pin: risolve l'indice via `ostree admin
/// status`, poi `pkexec ostree admin pin [--unpin] <indice>`.
/// Se l'indice non si trova, niente azione (mai indovinarlo).
fn start_pin(ui: &Rc<RefCell<Ui>>, dep: &Deployment, unpin: bool) {
    if ui.borrow().busy {
        return;
    }
    set_busy(ui, true);
    let checksum = dep.checksum.clone();
    append_log(
        ui,
        &format!(
            "Risoluzione dell'indice ostree per {}…",
            dep.short_checksum()
        ),
    );
    let tx = ui.borrow().tx.clone();
    let kind = if unpin { OpKind::Unpin } else { OpKind::Pin };
    std::thread::spawn(move || {
        let index = read_admin_status(&tx)
            .as_deref()
            .and_then(|text| rows::pin_index(text, &checksum));
        let Some(index) = index else {
            let _ = tx.send_blocking(UiEvent::PinIndexNotFound { checksum });
            return;
        };
        let mut argv: Vec<String> =
            vec!["ostree".to_string(), "admin".to_string(), "pin".to_string()];
        if unpin {
            argv.push("--unpin".to_string());
        }
        argv.push(index.to_string());
        let _ = tx.send_blocking(UiEvent::LogLine(format!("$ pkexec {}", argv.join(" "))));
        let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
        let tx_lines = tx.clone();
        let on_line = |line: &str| {
            let _ = tx_lines.send_blocking(UiEvent::LogLine(line.to_string()));
        };
        let result = run_streaming(&refs, true, &on_line);
        let (code, error) = match result {
            Ok(code) => (code, None),
            Err(err) => (None, Some(err)),
        };
        let _ = tx.send_blocking(UiEvent::OpFinished { kind, code, error });
    });
}

/// Rollback: `pkexec rpm-ostree rollback` (dopo conferma).
fn start_rollback(ui: &Rc<RefCell<Ui>>) {
    run_privileged_streaming(
        ui,
        OpKind::Rollback,
        vec!["rpm-ostree".to_string(), "rollback".to_string()],
    );
}

/// Riavvio: `pkexec systemctl reboot` (dopo conferma, worker thread).
fn start_reboot(ui: &Rc<RefCell<Ui>>) {
    if ui.borrow().busy {
        return;
    }
    set_busy(ui, true);
    append_log(ui, "$ pkexec systemctl reboot");
    let tx = ui.borrow().tx.clone();
    std::thread::spawn(move || {
        let (code, error) = match system::run_privileged(&["systemctl", "reboot"]) {
            Ok(out) => {
                for line in out.combined().lines() {
                    let _ = tx.send_blocking(UiEvent::LogLine(line.to_string()));
                }
                (out.code, None)
            }
            Err(err) => (None, Some(err)),
        };
        let _ = tx.send_blocking(UiEvent::OpFinished {
            kind: OpKind::Reboot,
            code,
            error,
        });
    });
}

/// Gestisce un evento arrivato dai worker thread (main loop).
fn handle_event(ui: &Rc<RefCell<Ui>>, event: UiEvent) {
    match event {
        UiEvent::Loaded(result) => {
            match result {
                Ok(deployments) => {
                    set_status(ui, "");
                    ui.borrow_mut().deployments = deployments;
                    rebuild_list(ui);
                    if ui.borrow().deployments.is_empty() {
                        set_status(ui, "Nessun deployment trovato.");
                    }
                }
                Err(detail) => {
                    ui.borrow_mut().deployments.clear();
                    rebuild_list(ui);
                    let msg = rows::atomic_error_message(&detail);
                    set_status(ui, &msg);
                    append_log(ui, &msg);
                }
            }
            set_busy(ui, false);
        }
        UiEvent::LogLine(line) => append_log(ui, &line),
        UiEvent::PinIndexNotFound { checksum } => {
            set_busy(ui, false);
            let short = &checksum[..checksum.len().min(12)];
            let msg = format!(
                "Impossibile trovare l'indice ostree per lo snapshot {short}: il formato di \
                 `ostree admin status` su questo sistema non è quello atteso. Pin non eseguito: \
                 l'indice non viene mai indovinato."
            );
            append_log(ui, &msg);
            set_status(ui, &msg);
            let win = ui.borrow().win.clone();
            show_error(&win, "Pin non disponibile", &msg);
        }
        UiEvent::OpFinished { kind, code, error } => {
            set_busy(ui, false);
            match (code, error) {
                (Some(0), _) => {
                    append_log(ui, kind.done_label());
                    notify("NovaOS Snapshot", kind.success_note());
                    if matches!(kind, OpKind::Deploy | OpKind::Rollback) {
                        ui.borrow().reboot_btn.set_visible(true);
                        set_status(ui, "Snapshot pronto: riavvia per applicarlo.");
                    }
                    refresh(ui);
                }
                (Some(code), _) => {
                    let msg = format!(
                        "{} fallito (codice {code}). Vedi il registro. Se pkexec non trova \
                         l'agente polkit, esegui il comando da terminale.",
                        kind.label()
                    );
                    append_log(ui, &msg);
                    set_status(ui, &msg);
                    notify("NovaOS Snapshot", "Operazione fallita: vedi il registro.");
                }
                (None, Some(err)) => {
                    let msg = format!(
                        "{} non avviato: {err}. Se pkexec non trova l'agente polkit, esegui il \
                         comando da terminale.",
                        kind.label()
                    );
                    append_log(ui, &msg);
                    set_status(ui, &msg);
                    notify("NovaOS Snapshot", "Operazione fallita: vedi il registro.");
                }
                (None, None) => {
                    let msg = format!(
                        "{} interrotto (processo terminato da un segnale).",
                        kind.label()
                    );
                    append_log(ui, &msg);
                    set_status(ui, &msg);
                }
            }
        }
    }
}

/// Costruisce la finestra principale e installa il ricevitore eventi.
fn build_window(app: &Application) -> Rc<RefCell<Ui>> {
    let win = adw::ApplicationWindow::builder()
        .application(app)
        .title("Snapshot di sistema")
        .default_width(680)
        .default_height(600)
        .build();

    let header = adw::HeaderBar::new();
    let title = gtk4::Label::new(Some("Snapshot di sistema"));
    title.add_css_class("title");
    header.set_title_widget(Some(&title));
    let refresh_btn = gtk4::Button::with_label("Aggiorna");
    refresh_btn.set_tooltip_text(Some("Ricarica l'elenco degli snapshot"));
    header.pack_end(&refresh_btn);
    win.set_titlebar(Some(&header));

    let status_label = gtk4::Label::new(None);
    status_label.set_xalign(0.0);
    status_label.set_wrap(true);
    status_label.set_visible(false);

    let list = gtk4::ListBox::new();
    list.set_selection_mode(gtk4::SelectionMode::None);
    let list_scroll = gtk4::ScrolledWindow::builder()
        .vexpand(true)
        .child(&list)
        .build();
    list_scroll.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);

    let rollback_btn = gtk4::Button::with_label("Torna al precedente");
    rollback_btn.add_css_class("destructive-action");
    rollback_btn.set_tooltip_text(Some(
        "Torna al deployment precedente al prossimo riavvio (rpm-ostree rollback)",
    ));
    let reboot_btn = gtk4::Button::with_label("Riavvia ora");
    reboot_btn.add_css_class("suggested-action");
    reboot_btn.set_visible(false);
    let bottom = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    bottom.append(&rollback_btn);
    let spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    bottom.append(&spacer);
    bottom.append(&reboot_btn);

    let log_label = gtk4::Label::new(Some("Registro"));
    log_label.set_xalign(0.0);
    log_label.add_css_class("heading");
    let log_text = gtk4::TextView::builder()
        .editable(false)
        .cursor_visible(false)
        .monospace(true)
        .build();
    let log_buffer = log_text.buffer();
    let log_scroll = gtk4::ScrolledWindow::builder().child(&log_text).build();
    log_scroll.set_min_content_height(140);
    log_scroll.set_policy(gtk4::PolicyType::Automatic, gtk4::PolicyType::Automatic);

    let content = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    content.set_margin_top(12);
    content.set_margin_bottom(12);
    content.set_margin_start(12);
    content.set_margin_end(12);
    content.append(&status_label);
    content.append(&list_scroll);
    content.append(&bottom);
    content.append(&log_label);
    content.append(&log_scroll);
    win.set_content(Some(&content));

    let (tx, rx) = async_channel::bounded::<UiEvent>(32);
    let ui = Rc::new(RefCell::new(Ui {
        win: win.clone(),
        tx,
        list,
        status_label,
        log_buffer,
        log_scroll,
        refresh_btn: refresh_btn.clone(),
        rollback_btn: rollback_btn.clone(),
        reboot_btn: reboot_btn.clone(),
        deployments: Vec::new(),
        busy: false,
    }));

    {
        let ui = ui.clone();
        refresh_btn.connect_clicked(move |_| refresh(&ui));
    }
    {
        let ui = ui.clone();
        let win = win.clone();
        rollback_btn.connect_clicked(move |_| {
            if ui.borrow().busy {
                return;
            }
            let ui = ui.clone();
            confirm(
                &win,
                "Tornare al deployment precedente?",
                "Il sistema tornerà allo snapshot precedente al prossimo riavvio.",
                "Torna al precedente",
                true,
                move || start_rollback(&ui),
            );
        });
    }
    {
        let ui = ui.clone();
        let win = win.clone();
        reboot_btn.connect_clicked(move |_| {
            if ui.borrow().busy {
                return;
            }
            let ui = ui.clone();
            confirm(
                &win,
                "Riavviare ora?",
                "Il sistema si riavvia subito con lo snapshot selezionato.",
                "Riavvia ora",
                true,
                move || start_reboot(&ui),
            );
        });
    }

    // Ricevitore eventi (main loop), come in nova-ai-ui.
    {
        let ui = ui.clone();
        glib::MainContext::default().spawn_local(async move {
            while let Ok(event) = rx.recv().await {
                handle_event(&ui, event);
            }
        });
    }

    append_log(&ui, "Pronto. Caricamento degli snapshot in corso…");
    win.present();
    ui
}

fn main() {
    let app = Application::builder()
        .application_id("org.novaos.Snapshots")
        .flags(ApplicationFlags::empty())
        .build();

    // Single-instance: se la finestra esiste già, la ripresenta.
    let ui_holder: Rc<RefCell<Option<Rc<RefCell<Ui>>>>> = Rc::new(RefCell::new(None));
    app.connect_activate(move |app| {
        if let Some(ui) = ui_holder.borrow().as_ref() {
            ui.borrow().win.present();
            return;
        }
        let css = gtk4::CssProvider::new();
        css.load_from_string(
            ".snap-badge { background: alpha(@accent_bg_color, 0.18); \
             border-radius: 999px; padding: 2px 8px; font-size: 0.85em; }",
        );
        if let Some(display) = gtk4::gdk::Display::default() {
            gtk4::style_context_add_provider_for_display(
                &display,
                &css,
                gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
        let ui = build_window(app);
        refresh(&ui);
        *ui_holder.borrow_mut() = Some(ui);
    });

    app.run();
}
