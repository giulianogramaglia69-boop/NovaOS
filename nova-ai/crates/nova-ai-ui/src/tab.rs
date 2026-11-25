//! The bottom-right tab: a small notch on the screen edge that opens
//! the chat. On Wayland compositors with layer-shell (Niri, Hyprland,
//! MangoWC, COSMIC) it floats over content in the Overlay layer with
//! exclusive zone 0. On compositors without layer-shell (GNOME/mutter)
//! it degrades to a normal small undecorated window (GTK4 has no
//! always-on-top / skip-taskbar API for the client).

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;

use crate::AppState;
use crate::chat;

pub fn build_tab(app: &adw::Application, state: Rc<RefCell<AppState>>) -> gtk4::ApplicationWindow {
    let win = gtk4::ApplicationWindow::builder()
        .application(app)
        .title("NovaAI")
        .decorated(false)
        .resizable(false)
        .build();

    if gtk4_layer_shell::is_supported() {
        use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
        win.init_layer_shell();
        win.set_layer(Layer::Overlay);
        win.set_anchor(Edge::Bottom, true);
        win.set_anchor(Edge::Right, true);
        win.set_exclusive_zone(0);
        win.set_keyboard_mode(KeyboardMode::None);
        win.set_margin(Edge::Bottom, 0);
        win.set_margin(Edge::Right, 0);
    } else {
        // Fallback (GNOME/mutter): a normal small undecorated window.
        // GTK4 offers no always-on-top / skip-taskbar control from the
        // client side (gtk_window_set_keep_above does not exist in GTK4),
        // so on GNOME the tab is just a small window; the chat window
        // still opens from it.
    }

    let btn = gtk4::Button::with_label("✦ NovaAI");
    btn.add_css_class("novaai-tab");
    btn.set_tooltip_text(Some("Apri NovaAI"));
    let app_clone = app.clone();
    let state_c = state.clone();
    btn.connect_clicked(move |_| {
        chat::show_chat(&app_clone, state_c.clone());
    });
    win.set_child(Some(&btn));
    state.borrow_mut().tab_btn = Some(btn);
    win
}

/// Update the tab indicator: when the wake listener is active the tab
/// shows a small mic dot (●) and the tooltip explains it.
/// Wake word only (feature `wakeword`).
#[cfg(feature = "wakeword")]
pub fn set_listening(state: &Rc<RefCell<AppState>>, listening: bool) {
    let Some(btn) = state.borrow().tab_btn.clone() else {
        return;
    };
    if listening {
        btn.set_label("✦ NovaAI ●");
        btn.set_tooltip_text(Some("Ehi Nova in ascolto — di' “Ehi Nova”"));
    } else {
        btn.set_label("✦ NovaAI");
        btn.set_tooltip_text(Some("Apri NovaAI"));
    }
}
