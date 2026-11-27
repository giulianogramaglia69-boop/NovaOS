//! novaos-setup — benvenuto al primo avvio di NovaOS.
//!
//! `novaos-setup [--first-run]`: con `--first-run`, se il primo avvio
//! è già stato visto (`flags::is_seen("setup", "first-run")`) esce
//! subito con codice 0. Altrimenti mostra tre schede (codec
//! multimediali, gaming, strumenti dev) con installazione in worker
//! thread e log in streaming.
//!
//! Pattern copiato da nova-ai-ui: `adw::Application` single-instance,
//! worker thread + `async_channel` per gli eventi verso la UI (la UI
//! non viene mai toccata dal worker). Su questa VM l'UI si può solo
//! type-checkare (niente GTK reale): l'esecuzione vera va verificata
//! su Fedora.

use std::rc::Rc;

use adw::prelude::*;
use gio::ApplicationFlags;
use novaos_common::flags;
use novaos_common::system;
use novaos_setup::steps::{GroupId, GroupState, RealRunner, check_installed, run_group};

/// Eventi dal worker thread verso il main loop GTK.
enum UiEvent {
    /// Una riga di log (comando `$ …` o output del passo).
    LogLine(String),
    /// Nuovo stato di un gruppo.
    GroupState(GroupId, GroupState),
    /// Tutti i gruppi selezionati sono terminati.
    Finished {
        all_ok: bool,
        needs_reboot: bool,
        errors: Vec<(GroupId, String)>,
    },
}

/// Widget di una scheda-gruppo.
struct GroupWidgets {
    id: GroupId,
    check: gtk4::CheckButton,
    status: gtk4::Label,
}

fn main() {
    let first_run = std::env::args().any(|a| a == "--first-run");
    if first_run && flags::is_seen("setup", "first-run") {
        return;
    }

    let app = adw::Application::builder()
        .application_id("org.novaos.Setup")
        .flags(ApplicationFlags::empty())
        .build();
    app.connect_activate(build_ui);
    app.run();
}

fn build_ui(app: &adw::Application) {
    // Stato iniziale: i gruppi già installati mostrano "Installato"
    // e la loro checkbox è disabilitata.
    let runner = RealRunner;
    let installed: Vec<(GroupId, bool)> = GroupId::all()
        .iter()
        .map(|g| (*g, check_installed(*g, &runner)))
        .collect();

    let win = adw::ApplicationWindow::builder()
        .application(app)
        .title("Benvenuto in NovaOS")
        .default_width(640)
        .default_height(660)
        .build();

    let header = adw::HeaderBar::new();
    let title = gtk4::Label::new(Some("Benvenuto in NovaOS"));
    title.add_css_class("title");
    header.set_title_widget(Some(&title));
    win.set_titlebar(Some(&header));

    let content = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    content.set_margin_top(16);
    content.set_margin_bottom(16);
    content.set_margin_start(16);
    content.set_margin_end(16);

    let welcome = gtk4::Label::new(Some(
        "Benvenuto in NovaOS! Scegli cosa installare: puoi farlo ora \
         oppure dopo, riaprendo NovaOS Setup dal menu.",
    ));
    welcome.set_wrap(true);
    welcome.set_xalign(0.0);
    content.append(&welcome);

    // --- schede dei tre gruppi ---
    let groups_box = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    let mut groups: Vec<GroupWidgets> = Vec::new();
    for (id, is_installed) in &installed {
        let card = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
        card.set_margin_top(6);
        card.set_margin_bottom(6);
        card.set_margin_start(6);
        card.set_margin_end(6);
        card.add_css_class("card");

        let head = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        let check = gtk4::CheckButton::with_label(id.title());
        check.set_sensitive(!is_installed);
        head.append(&check);
        let status = gtk4::Label::new(Some(if *is_installed {
            GroupState::Installed.label()
        } else {
            GroupState::NotInstalled.label()
        }));
        status.set_xalign(0.0);
        status.set_hexpand(true);
        // Stato a destra, titolo a sinistra.
        head.append(&status);
        card.append(&head);

        let desc = gtk4::Label::new(Some(id.description()));
        desc.set_wrap(true);
        desc.set_xalign(0.0);
        desc.add_css_class("dim-label");
        card.append(&desc);

        if *id == GroupId::Dev {
            let note = gtk4::Label::new(Some(
                "Dopo l'installazione di rustup apri un nuovo terminale \
                 oppure esegui `source ~/.cargo/env`.",
            ));
            note.set_wrap(true);
            note.set_xalign(0.0);
            note.add_css_class("dim-label");
            card.append(&note);
        }

        groups_box.append(&card);
        groups.push(GroupWidgets {
            id: *id,
            check,
            status,
        });
    }
    content.append(&groups_box);

    // --- log in streaming ---
    let log_title = gtk4::Label::new(Some("Avanzamento"));
    log_title.add_css_class("heading");
    log_title.set_xalign(0.0);
    content.append(&log_title);
    let log_view = gtk4::TextView::new();
    log_view.set_monospace(true);
    log_view.set_editable(false);
    log_view.set_cursor_visible(false);
    log_view.set_wrap_mode(gtk4::WrapMode::None);
    let log_scroll = gtk4::ScrolledWindow::builder()
        .vexpand(true)
        .child(&log_view)
        .build();
    log_scroll.set_policy(gtk4::PolicyType::Automatic, gtk4::PolicyType::Automatic);
    content.append(&log_scroll);

    // --- pulsanti ---
    let buttons = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    let reboot_btn = gtk4::Button::with_label("Riavvia ora");
    reboot_btn.add_css_class("suggested-action");
    reboot_btn.set_visible(false);
    reboot_btn.set_tooltip_text(Some(
        "Riavvia per applicare i pacchetti di sistema installati",
    ));
    let spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    let skip_btn = gtk4::Button::with_label("Salta");
    let install_btn = gtk4::Button::with_label("Installa selezionati");
    install_btn.add_css_class("suggested-action");
    let close_btn = gtk4::Button::with_label("Chiudi");
    buttons.append(&reboot_btn);
    buttons.append(&spacer);
    buttons.append(&skip_btn);
    buttons.append(&install_btn);
    buttons.append(&close_btn);
    content.append(&buttons);

    win.set_content(Some(&content));

    let groups = Rc::new(groups);
    let app_quit = app.clone();

    // "Installa selezionati" è attivo solo se almeno un gruppo
    // installabile è spuntato.
    let refresh_install: Rc<dyn Fn()> = {
        let groups = groups.clone();
        let install_btn = install_btn.clone();
        Rc::new(move || {
            let any = groups
                .iter()
                .any(|g| g.check.is_active() && g.check.is_sensitive());
            install_btn.set_sensitive(any);
        })
    };
    for g in groups.iter() {
        let refresh = refresh_install.clone();
        g.check.connect_toggled(move |_| refresh());
    }
    refresh_install();

    // Chiudere la finestra (anche con la X) conta come primo avvio visto.
    win.connect_close_request(|_| {
        let _ = flags::mark_seen("setup", "first-run");
        glib::Propagation::Proceed
    });

    // --- "Salta" e "Chiudi"/"Fine": segnano il primo avvio e chiudono. ---
    {
        let win = win.clone();
        let app_quit = app_quit.clone();
        skip_btn.connect_clicked(move |_| {
            let _ = flags::mark_seen("setup", "first-run");
            win.close();
            app_quit.quit();
        });
    }
    {
        let win = win.clone();
        let app_quit = app_quit.clone();
        close_btn.connect_clicked(move |_| {
            let _ = flags::mark_seen("setup", "first-run");
            win.close();
            app_quit.quit();
        });
    }

    // --- "Riavvia ora": conferma, poi `pkexec systemctl reboot`. ---
    {
        let win = win.clone();
        let reboot_btn_c = reboot_btn.clone();
        let log_view = log_view.clone();
        let log_scroll = log_scroll.clone();
        reboot_btn.connect_clicked(move |_| {
            let dialog = gtk4::AlertDialog::builder()
                .message("Riavviare ora?")
                .detail(
                    "Il sistema verrà riavviato per applicare i pacchetti \
                     installati.",
                )
                .buttons(["Annulla", "Riavvia"])
                .cancel_button(0)
                .default_button(1)
                .build();
            let reboot_btn_c = reboot_btn_c.clone();
            let log_view = log_view.clone();
            let log_scroll = log_scroll.clone();
            dialog.choose(Some(&win), None::<&gio::Cancellable>, move |resp| {
                if let Ok(1) = resp {
                    reboot_btn_c.set_sensitive(false);
                    // L'errore (se c'è) torna in UI sul canale, come
                    // nel flusso di installazione: i widget GTK non
                    // sono Send e non possono essere toccati dal
                    // worker thread.
                    let (tx, rx) = async_channel::bounded::<String>(1);
                    let log_view = log_view.clone();
                    let log_scroll = log_scroll.clone();
                    glib::MainContext::default().spawn_local(async move {
                        while let Ok(msg) = rx.recv().await {
                            append_log(&log_view, &log_scroll, &msg);
                        }
                    });
                    std::thread::spawn(move || {
                        // Il riavvio di solito riesce e il sistema si
                        // spegne; solo in caso di errore si torna in UI.
                        let result = system::run_privileged(&["systemctl", "reboot"]);
                        let msg = match result {
                            Ok(out) if out.success() => None,
                            Ok(out) => Some(format!(
                                "Riavvio fallito (codice {:?}): {}",
                                out.code,
                                out.combined()
                            )),
                            Err(e) => Some(format!(
                                "Riavvio fallito: {e} Se pkexec non trova \
                                     l'agente polkit, riavvia dal terminale con \
                                     `systemctl reboot`."
                            )),
                        };
                        if let Some(msg) = msg {
                            let _ = tx.send_blocking(msg);
                        }
                    });
                }
            });
        });
    }

    // --- "Installa selezionati": conferma, poi worker thread. ---
    {
        let win = win.clone();
        let groups = groups.clone();
        let install_btn_c = install_btn.clone();
        let skip_btn_c = skip_btn.clone();
        let close_btn_c = close_btn.clone();
        let reboot_btn_c = reboot_btn.clone();
        let log_view = log_view.clone();
        let log_scroll = log_scroll.clone();
        let refresh_install = refresh_install.clone();
        install_btn.connect_clicked(move |_| {
            let selected: Vec<GroupId> = groups
                .iter()
                .filter(|g| g.check.is_active() && g.check.is_sensitive())
                .map(|g| g.id)
                .collect();
            if selected.is_empty() {
                return;
            }
            let dialog = gtk4::AlertDialog::builder()
                .message("Installare i pacchetti selezionati?")
                .detail(
                    "Verranno installati i pacchetti selezionati. Alcuni \
                     richiedono il riavvio. Continuare?",
                )
                .buttons(["Annulla", "Installa"])
                .cancel_button(0)
                .default_button(1)
                .build();
            let groups = groups.clone();
            let install_btn_c = install_btn_c.clone();
            let skip_btn_c = skip_btn_c.clone();
            let close_btn_c = close_btn_c.clone();
            let reboot_btn_c = reboot_btn_c.clone();
            let log_view = log_view.clone();
            let log_scroll = log_scroll.clone();
            let refresh_install = refresh_install.clone();
            let win_cb = win.clone();
            dialog.choose(Some(&win), None::<&gio::Cancellable>, move |resp| {
                if let Ok(1) = resp {
                    start_install(
                        selected,
                        groups,
                        install_btn_c,
                        skip_btn_c,
                        close_btn_c,
                        reboot_btn_c,
                        log_view,
                        log_scroll,
                        refresh_install,
                        win_cb,
                    );
                }
            });
        });
    }

    win.present();
}

/// Avvia l'installazione dei gruppi selezionati in un worker thread.
/// La UI resta reattiva: il worker manda log e stati sul canale.
#[allow(clippy::too_many_arguments)]
fn start_install(
    selected: Vec<GroupId>,
    groups: Rc<Vec<GroupWidgets>>,
    install_btn: gtk4::Button,
    skip_btn: gtk4::Button,
    close_btn: gtk4::Button,
    reboot_btn: gtk4::Button,
    log_view: gtk4::TextView,
    log_scroll: gtk4::ScrolledWindow,
    refresh_install: Rc<dyn Fn()>,
    win: adw::ApplicationWindow,
) {
    // Disabilita tutto durante l'installazione.
    install_btn.set_sensitive(false);
    skip_btn.set_sensitive(false);
    close_btn.set_sensitive(false);
    for g in groups.iter() {
        g.check.set_sensitive(false);
    }
    for id in &selected {
        set_group_state(&groups, *id, GroupState::Installing);
    }

    let (tx, rx) = async_channel::bounded::<UiEvent>(64);

    // Ricevitore nel main loop (stesso pattern di nova-ai-ui/chat.rs).
    {
        let groups = groups.clone();
        let skip_btn = skip_btn.clone();
        let close_btn = close_btn.clone();
        let reboot_btn = reboot_btn.clone();
        let log_view = log_view.clone();
        let log_scroll = log_scroll.clone();
        let refresh_install = refresh_install.clone();
        let win = win.clone();
        glib::MainContext::default().spawn_local(async move {
            while let Ok(ev) = rx.recv().await {
                match ev {
                    UiEvent::LogLine(line) => {
                        append_log(&log_view, &log_scroll, &line);
                    }
                    UiEvent::GroupState(id, state) => {
                        set_group_state(&groups, id, state);
                        if state == GroupState::Installed {
                            // Gruppo fatto: checkbox spuntata e bloccata.
                            if let Some(g) = groups.iter().find(|g| g.id == id) {
                                g.check.set_active(false);
                            }
                        }
                    }
                    UiEvent::Finished {
                        all_ok,
                        needs_reboot,
                        errors,
                    } => {
                        for g in groups.iter() {
                            // I gruppi già installati restano bloccati;
                            // gli altri tornano selezionabili.
                            let done = g.status.text().as_str() == GroupState::Installed.label();
                            g.check.set_sensitive(!done);
                        }
                        skip_btn.set_sensitive(true);
                        close_btn.set_sensitive(true);
                        close_btn.set_label("Fine");
                        refresh_install();
                        if all_ok {
                            append_log(&log_view, &log_scroll, "Installazione completata.");
                            if needs_reboot {
                                reboot_btn.set_visible(true);
                            }
                        } else {
                            append_log(
                                &log_view,
                                &log_scroll,
                                "Installazione non riuscita: controlla il log.",
                            );
                        }
                        if !errors.is_empty() {
                            let detail = errors
                                .iter()
                                .map(|(id, e)| format!("{}:\n{e}", id.title()))
                                .collect::<Vec<_>>()
                                .join("\n\n");
                            let dialog = gtk4::AlertDialog::builder()
                                .message("Installazione non riuscita")
                                .detail(&detail)
                                .buttons(["Chiudi"])
                                .cancel_button(0)
                                .default_button(0)
                                .build();
                            dialog.choose(Some(&win), None::<&gio::Cancellable>, |_| {});
                        }
                        break;
                    }
                }
            }
        });
    }

    // Worker thread: esegue i gruppi in sequenza, poi chiude il canale.
    std::thread::spawn(move || {
        let runner = RealRunner;
        let mut all_ok = true;
        let mut needs_reboot = false;
        let mut errors: Vec<(GroupId, String)> = Vec::new();
        for group in selected {
            let _ = tx.send_blocking(UiEvent::GroupState(group, GroupState::Installing));
            let emit = |line: &str| {
                let _ = tx.send_blocking(UiEvent::LogLine(line.to_string()));
            };
            let outcome = run_group(group, &runner, &emit);
            if outcome.success {
                let _ = flags::mark_done("setup", group.flag());
                needs_reboot |= outcome.needs_reboot;
                let _ = tx.send_blocking(UiEvent::GroupState(group, GroupState::Installed));
            } else {
                all_ok = false;
                if let Some(err) = outcome.error {
                    errors.push((group, err));
                }
                let _ = tx.send_blocking(UiEvent::GroupState(group, GroupState::Error));
            }
        }
        let _ = tx.send_blocking(UiEvent::Finished {
            all_ok,
            needs_reboot,
            errors,
        });
    });
}

/// Aggiorna l'etichetta di stato di un gruppo.
fn set_group_state(groups: &[GroupWidgets], id: GroupId, state: GroupState) {
    if let Some(g) = groups.iter().find(|g| g.id == id) {
        g.status.set_text(state.label());
    }
}

/// Appende una riga al log e scorre in fondo (autoscroll).
fn append_log(view: &gtk4::TextView, scroll: &gtk4::ScrolledWindow, line: &str) {
    let buffer = view.buffer();
    let mut end = buffer.end_iter();
    buffer.insert(&mut end, line);
    buffer.insert(&mut end, "\n");
    let adj = scroll.vadjustment();
    adj.set_value(adj.upper() - adj.page_size());
}
