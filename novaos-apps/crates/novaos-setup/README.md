# novaos-setup

Benvenuto al primo avvio di NovaOS (`org.novaos.Setup`).

```
novaos-setup [--first-run]
```

Con `--first-run`, se il primo avvio è già stato visto esce subito
(codice 0). Senza flag l'app si apre normalmente (riavviabile dal
menu) con gli stati già installati.

## Cosa fa

Tre schede con checkbox, descrizione e stato (`Non installato` /
`Installato` / `Installazione…` / `Errore`):

| Gruppo | Cosa installa | Riavvio |
|---|---|---|
| Codec multimediali | RPM Fusion free + `ffmpeg`, `gstreamer1-plugins-bad-freeworld`, `gstreamer1-plugins-ugly`, `gstreamer1-plugin-libav` | sì |
| Gaming | Steam (Flatpak), `mangohud` + `mangohud.i686`, `gamemode` + `gamemode.i686` | sì (solo per MangoHUD/GameMode; Steam no) |
| Strumenti dev | rustup (utente), VS Code (Flatpak), `git` | sì (solo per git) |

`Installa selezionati` chiede conferma, poi esegue i passi in un
worker thread con log in streaming (TextView monospace, autoscroll).
Gli errori sono sempre visibili: log + etichetta di stato + dialogo
con la coda dell'output. A successo i flag vengono segnati
(`setup/codec`, `setup/gaming`, `setup/dev`) e, se serve il riavvio,
compare il pulsante `Riavvia ora` (conferma → `pkexec systemctl
reboot`). `Salta` e `Chiudi`/`Fine` segnano il primo avvio come visto
e chiudono.

I passi già soddisfatti vengono saltati (controlli `rpm -q`,
`flatpak list --system --app --columns=application`,
`~/.cargo/bin/rustup`).

## Comandi usati

- Codec — repo: `rpm-ostree install https://download1.rpmfusion.org/free/fedora/rpmfusion-free-release-45.noarch.rpm`
  (solo RPM Fusion **free**; nonfree non serve per i codec);
  codec: `rpm-ostree install ffmpeg gstreamer1-plugins-bad-freeworld gstreamer1-plugins-ugly gstreamer1-plugin-libav`.
  `ffmpeg` confligge con `ffmpeg-free`: se `rpm -q ffmpeg-free`
  dice che è presente si usa subito
  `rpm-ostree override remove ffmpeg-free --install …`; se
  l'installazione normale fallisce citando `ffmpeg-free`, si riprova
  con l'override.
- Gaming — `flatpak install --system -y flathub com.valvesoftware.Steam`
  (via pkexec); `rpm-ostree install mangohud mangohud.i686 gamemode gamemode.i686` (via pkexec).
- Dev — rustup ufficiale come **utente** (mai con pkexec):
  `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable`;
  `flatpak install --system -y flathub com.visualstudio.code` (via
  pkexec); `rpm-ostree install git` (via pkexec). `code-oss` non è
  nei repo ufficiali F45 (solo COPR) e non viene usato.
- Tutti i comandi privilegiati passano per `pkexec`. Senza agente
  polkit attivo pkexec fallisce subito: l'app mostra l'errore e
  suggerisce il terminale.

La logica dei comandi (argv, skip-check, fallback `ffmpeg-free`) è
in `src/steps.rs`, con unit test che non toccano rete né UI
(`Runner` finto). La UI in `src/main.rs` segue il pattern di
nova-ai-ui: `adw::Application` single-instance, worker thread +
`async_channel` verso il main loop.

## Verifiche

```sh
export PATH="$HOME/.rust-standalone/bin:$PATH"
export PKG_CONFIG_PATH="$HOME/tmp/fake-pkgconfig"   # stub .pc per il type-check
export RUSTFLAGS="-L $HOME/tmp/fake-libs -C link-args=-Wl,--allow-shlib-undefined"
export LD_LIBRARY_PATH="$HOME/tmp/fake-libs"        # stub .so per il link dei test
cd ~/workspace/NovaOS/novaos-apps
cargo test -p novaos-setup --offline
cargo check -p novaos-setup --offline
cargo-clippy clippy -p novaos-setup --offline --all-targets -- -D warnings
rustfmt --edition 2024 --check crates/novaos-setup/src/*.rs
```

Note sull'ambiente VM (26 nov 2026):

- `cargo clippy` (sottocomando) fallisce con `rustup could not choose
  a version of cargo-clippy`: va invocato il binario direttamente
  (`cargo-clippy clippy …`, stessa toolchain).
- I file in `~/tmp/fake-libs` sono stub vuoti per `libgtk-4`,
  `libgdk-4`, `libadwaita-1`, `libgraphene-gobject-1.0` (più copie
  delle `.so.0` reali di graphene e gdk-pixbuf): servono solo a far
  linkare il binario di test su questa VM, dove GTK reale non esiste.
  I test di `steps.rs` non chiamano mai codice GTK.
- **Refuso nello stub `gdk-pixbuf-2.0.pc`**: dice
  `-lgdk-pixbuf-2.0` (trattino) ma la libreria vera è
  `libgdk_pixbuf-2.0` (underscore). Per questo in `~/tmp/fake-libs`
  c'è anche `libgdk-pixbuf-2.0.so` (copia della libreria reale).
  Lo stub `.pc` non è stato corretto perché è condiviso con altri
  progetti (nova-ai-ui); chi lo corregge risolve il link per tutti.

## Limiti / TODO

- **UI solo type-checkata su questa VM** (niente GTK reale, niente
  display): l'esecuzione vera — finestra, dialoghi, log in
  streaming, pkexec — va verificata su Fedora reale.
- **Comportamento `ffmpeg-free` da verificare su Fedora reale**: non
  è verificato se `ffmpeg-free` sia davvero installato nell'immagine;
  il codice controlla con `rpm -q` e riprova con l'override se
  l'errore lo cita (TODO marcato anche in `src/steps.rs`).
- **pkexec su COSMIC**: l'agente polkit `lxpolkit` è già
  nell'immagine sui tiling e l'integrazione lo avvierà ovunque; su
  GNOME c'è l'agente di gnome-shell. Su COSMIC va verificato che
  l'agente parta davvero, altrimenti pkexec fallisce e l'utente deve
  usare il terminale.
- Dopo rustup, il `PATH` della shell va aggiornato (`source
  ~/.cargo/env` o nuovo terminale): la nota è mostrata nella scheda
  Dev e nel log.
