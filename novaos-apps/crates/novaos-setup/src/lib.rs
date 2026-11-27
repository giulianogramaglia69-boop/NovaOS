//! novaos-setup — libreria: logica dei passi di installazione testabile
//! senza GTK (il binario in `main.rs` è la UI libadwaita).
//!
//! La separazione esiste perché sulla VM l'UI si può solo
//! type-checkare (niente librerie GTK reali per il link): i test di
//! `steps` girano invece con `cargo test --lib`, senza GTK.

pub mod steps;
