# NovaAI — l'agente AI di NovaOS

NovaAI è l'assistente di sistema di NovaOS: una piccola tab in basso a
destra apre una chat da cui gestire il sistema in italiano — aprire app,
installare software, regolare volume, luminosità, Wi-Fi e tema.

Licenza: GPLv3. Rust edition 2024.

## Architettura

Workspace Cargo con due crate:

```
nova-ai/
├── crates/nova-ai-core/   # LLM client, agent loop, tool, config, provider (NO GTK)
└── crates/nova-ai-ui/     # chat app GTK4 + libadwaita + gtk4-layer-shell
```

- **`nova-ai-core`** non dipende da GTK: compila e si testa ovunque, anche
  su questa VM. HTTP sincrono via `ureq` (niente tokio): il client LLM è
  bloccante e la UI lo chiama da thread worker. Scelta deliberata per v1:
  un solo percorso di codice, niente runtime async, test con un mock HTTP
  locale in puro std.
- **`nova-ai-ui`** è uno strato sottile sopra core: tab, chat, setup,
  allegati, voce, wake word. Compila nell'immagine Fedora (tutti i
  `-devel` esistono lì); su questa VM le librerie GTK di sviluppo non
  sono installabili via apt, ma la UI fa `cargo check` + clippy puliti
  anche qui grazie a file `.pc` stub in `/tmp/fake-pkgconfig`
  (vedi "Stato della verifica"): il type-check è reale, mancano solo
  link ed esecuzione.

## Provider

Tutti parlano l'API OpenAI-compatible `chat/completions`: un solo client.

| Provider | Base URL | Modello di default | Visione | Voce (Whisper) |
|---|---|---|---|---|
| Groq | https://api.groq.com/openai/v1 | `llama-3.3-70b-versatile` | solo modelli `*-vision-preview` | `whisper-large-v3` |
| OpenAI | https://api.openai.com/v1 | `gpt-4o-mini` | sì (`gpt-4o*`) | `whisper-1` |
| OpenRouter | https://openrouter.ai/api/v1 | `anthropic/claude-haiku-4.5` | sì (Claude, `gpt-4o*`) | no |
| Custom | a scelta | a scelta | euristica sul nome | no |

Groq è il default a config mancante: tier gratuito, senza carta.

La chiave API va nel portachiavi di sistema via libsecret (servizio
`NovaAI`, attributo `provider=<nome>`; implementato con il CLI
`secret-tool`, senza crate D-Bus). Se il portachiavi non è disponibile,
la chiave finisce in `~/.config/nova-ai/config.toml` con permessi 0600.
La chiave non viene mai stampata nei log.

## Agent loop e conferme

```
utente → LLM → tool call → esegui → risultato → LLM → risposta
```

Max 8 iterazioni di tool per turno. I tool di sola lettura
(`list_apps`, `open_app`, `get_*`, `system_info`) girano subito. I tool
che modificano il sistema (`install_app`, `remove_app`, `set_volume`,
`set_brightness`, `set_wifi`, `set_theme`) prima restituiscono un'azione
in sospeso: la UI mostra la proposta con Approva / Rifiuta, e solo dopo
l'approvazione core esegue. La macchina a stati vive in core, non nella
UI, quindi la regola non dipende dalla UI.

## Tool (v1)

| Tool | Come funziona |
|---|---|
| `list_apps` | Legge i `.desktop` di `/usr/share/applications`, `~/.local/share/applications`, `/var/lib/flatpak` e `~/.local/share/flatpak` (Name, Exec, Icon, Categories; salta `NoDisplay`/`Hidden`) |
| `open_app` | Ricerca approssimativa sul nome, poi `gtk-launch <id>` (fallback: riga Exec) |
| `install_app` | `flatpak search` per risolvere l'id, poi `flatpak install --user -y flathub <id>` (dopo conferma) |
| `remove_app` | `flatpak uninstall --user -y <id>` (dopo conferma; solo Flatpak) |
| `get/set_volume` | `wpctl` su `@DEFAULT_AUDIO_SINK@` (PipeWire) |
| `get/set_brightness` | `brightnessctl` |
| `get/set_wifi` | `nmcli radio wifi` |
| `set_theme` | `gsettings set org.gnome.desktop.interface color-scheme` (GNOME; su COSMIC/tiling best-effort) |
| `system_info` | `/usr/lib/os-release`, `$XDG_CURRENT_DESKTOP`, `/proc/uptime`, `df /` |

Gli errori sono onesti e specifici (es. "nessun risultato su Flathub",
"remote Flathub non configurato", "comando non trovato"): niente
successi finti.

## UI

- **Tab**: finestrella arrotondata in basso a destra con "✦ NovaAI".
  Su Wayland con layer-shell (Niri, Hyprland, MangoWC, COSMIC) usa
  `gtk4-layer-shell` (layer Overlay, anchor bottom+right, exclusive
  zone 0: galleggia sopra il contenuto). Su GNOME/mutter degrada a
  finestrella normale non decorata (GTK4 non offre controllo
  always-on-top / skip-taskbar dal client).
- **Chat**: lista messaggi, input, allegati (📎), voce (🎤, tieni premuto),
  indicatore provider/modello. Chiudere la chat la nasconde; la tab la
  riapre. "Esci" nel header chiude l'app.
- **Allegati**: i file di testo (≤100 KiB) vengono iniettati nel contesto;
  le immagini vengono ridimensionate a max 1024px sul lato lungo
  (GdkPixbuf) e inviate come `image_url` base64 solo se il modello
  corrente supporta la visione, altrimenti la UI avvisa e core aggiunge
  una nota nella risposta.
- **Voce**: registrazione microfono con `pw-record` (PipeWire, 16 kHz
  mono) mentre il pulsante è premuto; trascrizione via Whisper del
  provider (Groq `whisper-large-v3`, OpenAI `whisper-1`). Con OpenRouter
  o Custom il pulsante è disabilitato con tooltip che spiega perché.
  (Scelta `pw-record` al posto di cpal: niente dipendenze native extra,
  PipeWire c'è già su NovaOS.)
- **Primo avvio**: se non c'è provider/chiave, si apre la schermata di
  setup (provider, base URL, modello, chiave API) con "Salva e verifica"
  (una mini-chiamata "say hi"). Solo dopo un test riuscito si apre la
  chat. La schermata di setup chiede anche se attivare la wake word
  "Ehi Nova" (opt-in, vedi sotto).
- **Impostazioni**: pulsante ⚙ nel header della chat. Mostra provider e
  modello, e l'interruttore "Ehi Nova" con download del modello e stato.
- **Autostart**: `nova-ai.desktop` (`Exec=nova-ai`,
  `X-GNOME-Autostart-enabled=true`); l'integrazione nell'immagine lo
  copia in `/etc/xdg/autostart/`.

## Wake word "Ehi Nova" (opt-in, default OFF)

Dì "Ehi Nova" e la chat si apre da sola, ascolta il comando e lo
esegue come un messaggio vocale. Funziona così:

- **Rilevamento 100% offline con Vosk** (crate `vosk` 0.3.1,
  `vosk-sys` collega `libvosk.so`). Il modello italiano piccolo
  `vosk-model-small-it-0.22` (~47 MB) gira in locale con grammatica
  vincolata `["ehi nova"]`: CPU bassa e niente falsi positivi da
  discorsi normali. **L'audio non lascia mai il dispositivo** per il
  rilevamento; solo il comando dopo l'attivazione va all'API Whisper
  del provider per la trascrizione.
- **Attivazione esplicita**: la schermata di setup chiede "Vuoi
  attivare 'Ehi Nova'?" (sì/no) con la spiegazione sopra; la schermata
  impostazioni ha lo stesso interruttore, con effetto immediato in
  qualsiasi momento. Persiste in config (`wake_word: true/false`,
  default `false`).
- **Modello scaricato al primo uso** in
  `~/.local/share/nova-ai/models/vosk-model-small-it-0.22/` (download
  con barra di avanzamento da setup o da impostazioni; estrazione in
  cartella temporanea e spostamento solo a download completo, così un
  download fallito non lascia un modello rotto). Se offline o in
  errore, la UI spiega il problema e lascia riprovare dopo; chat e
  pulsante vocale restano funzionanti. **Il modello NON è nel repo**
  (47 MB): si scarica a runtime.
- **Flusso**: il listener (thread dedicato, microfono via cpal a
  16 kHz mono) rileva la frase → apre la chat e mostra "Ti ascolto…" →
  registra il comando (finestra fissa di 10 s) → trascrizione Whisper
  → turno agente normale. Durante il turno il listener è in pausa e
  riparte da solo alla fine. Se c'è un'azione in attesa di conferma,
  il turno hands-free viene saltato (non può sovrascriverla) e la chat
  lo dice. Mentre tieni premuto il pulsante vocale, il listener è in
  pausa per non sentire "ehi nova" nella tua stessa voce.
- **Indicatore**: quando l'ascolto è attivo la linguetta mostra un
  piccolo punto: "✦ NovaAI ●" (tooltip "Ehi Nova in ascolto").
- **Feature cargo `wakeword`** (default ON in workspace): la wake
  word vive dietro questa feature. `vosk-sys` non ha build script e
  non scarica nulla: si aspetta un `libvosk.so` di sistema sul
  percorso di link; cpal vuole gli header ALSA a build time. Senza
  quelle librerie native si compila con `--no-default-features` e il
  resto di NovaAI funziona, solo senza wake word (la schermata
  impostazioni lo dice).

Limiti v1: la finestra di comando è fissa a 10 s (niente rilevamento
di fine frase); il comando viene registrato dopo la wake word, quindi
serve una brevissima pausa dopo "Ehi Nova"; il modello è italiano
solo (niente wake word in altre lingue).

## Cosa v1 NON fa

- **Niente TTS** (risposte vocali): fuori scope v1.
- **Niente layering rpm-ostree** da chat (`rpm-ostree install`): le app
  si installano solo da Flathub; il resto va chiesto in un secondo
  momento (o via terminale).
- **Niente azioni da root** e niente modifiche a file di sistema.
- **Niente tema nativo COSMIC**: `set_theme` passa da gsettings
  (GNOME); su COSMIC e sui tiling è best-effort.
- **Niente gestione delle app RPM** (solo Flatpak per install/rimuovi).
- **Niente memoria persistente** tra sessioni: la cronologia vive solo
  nella sessione corrente.

## Sviluppo e test

```bash
cargo test -p nova-ai-core          # test core (mock HTTP locale, nessun API call reale)
cargo test -p nova-ai-core --no-default-features   # senza wake word (niente libvosk/cpal)
cargo clippy -p nova-ai-core -- -D warnings
cargo fmt --check                   # (o rustfmt --edition 2024)
cargo build -p nova-ai-ui           # richiede le -devel GTK (Fedora; non su questa VM)
```

Su questa VM la UI fa comunque `cargo check` + clippy puliti con
file `.pc` stub: i `-devel` GTK veri non ci sono, ma i build script
delle crate `-sys` vogliono solo `pkg-config`. Con
`PKG_CONFIG_PATH=/tmp/fake-pkgconfig` (stub per gtk4 4.16,
libadwaita-1 1.7, gtk4-layer-shell-0 1.3, glib/gio 2.82,
gdk-pixbuf-2.0 2.42, cairo, pango, graphene, …) `cargo check` compila
i binding e fa il type-check del codice UI — niente link (mancano le
`.so` vere) e niente esecuzione. I file stub sono in `/tmp` (non nel
repo): vanno ricreati se la VM viene sostituita.

Per compilare la wake word serve `libvosk.so` sul percorso di link:
`vosk-sys` 0.2.0 non ha build script e non scarica la libreria. In
questa VM è stato usato il prebuilt ufficiale
`vosk-linux-x86_64-0.3.45.zip` (da
`github.com/alphacep/vosk-api/releases`) con
`RUSTFLAGS="-L <dir-con-libvosk.so>"` e `LD_LIBRARY_PATH` per i test.
I due test `#[ignore]` (`download_real_model`, `vosk_silence_no_false_detection`)
scaricano il vero modello (47 MB) e verificano che Vosk lo carichi:
`cargo test -p nova-ai-core --lib -- --ignored`.

## Integrazione nell'immagine NovaOS (passi per dopo)

1. Nel `Containerfile`, prima del build di NovaAI, installare i
   pacchetti build: `rust cargo`, `gtk4-devel`, `libadwaita-devel`,
   `gtk4-layer-shell-devel`, `libsecret-devel` (serve `secret-tool` a
   runtime: pacchetto `libsecret`), `alsa-lib-devel` (cpal, wake word),
   più i runtime `brightnessctl`, `flatpak`, `pipewire-utils` (per
   `wpctl`), `NetworkManager` (`nmcli`).
2. **libvosk per la wake word** (Fedora non la pacchettizza): nel
   Containerfile scaricare il prebuilt ufficiale, es.
   `https://github.com/alphacep/vosk-api/releases/download/v0.3.45/vosk-linux-x86_64-0.3.45.zip`,
   estrarre `libvosk.so` in `/usr/lib64/` (link time + runtime) e
   lanciare `ldconfig`. In alternativa compilare con
   `cargo build --release --no-default-features` (senza wake word).
   Il modello Vosk **non** va nell'immagine: si scarica al primo uso
   (~47 MB in `~/.local/share/nova-ai/models/`).
3. Compilare in release (`cargo build --release`, feature `wakeword`
   attiva di default) e installare il binario in `/usr/bin/nova-ai`.
4. Copiare `nova-ai.desktop` in `/etc/xdg/autostart/` (o in
   `/usr/share/applications/` + autostart).
5. Verificare sull'immagine avviata: tab visibile su Niri/Hyprland/
   MangoWC/COSMIC (layer-shell) e su GNOME (fallback), setup al primo
   avvio con chiave Groq, `flatpak search` + install con conferma,
   voce con Groq Whisper, wake word "Ehi Nova" (download modello,
   punto ● sulla tab, comando vocale, pausa durante il turno).

## Stato della verifica (su questa VM, 25 nov 2026)

- `nova-ai-core`: compila, `cargo test` verde (**33 test**: 27 unit
  pass + 6 integrazione, +2 manuali `#[ignore]`), clippy + fmt puliti,
  anche con `--no-default-features`. **Wake word verificata qui**: `vosk`
  0.3.1 + `cpal` 0.15.3 compilano con la `libvosk.so` prebuilt
  ufficiale (0.3.45, scaricata da GitHub releases; `vosk-sys` non ha
  build script e non scarica nulla da sé — serve `libvosk.so` sul
  percorso di link). I test `#[ignore]` sono stati eseguiti: download
  reale del modello (47 MB, estrazione verificata) e caricamento del
  modello in Vosk con 1 s di silenzio (nessun falso rilevamento, il
  detector resta in ascolto). **Non verificabile in questa VM**: il
  listener con microfono reale (la VM non ha input audio:
  `default_input_device()` è None) e il riconoscimento della frase
  parlata vera (serve una voce reale); il percorso audio cpal è
  coperto da test di normalizzazione/resampling su dati sintetici.
- `nova-ai-ui`: **`cargo check` e clippy puliti qui** (0 warning, con
  e senza feature `wakeword`), usando file `.pc` stub in
  `/tmp/fake-pkgconfig` perché i `-devel` GTK veri non sono
  installabili via apt su questa VM. Link ed esecuzione restano per
  l'immagine Fedora. Il type-check ha trovato e fatto correggere
  errori reali che la sola revisione del codice aveva mancato:
  la crate si chiama `libadwaita`, non `adw` (serve il rename in
  `Cargo.toml`); gtk4 0.9 / libadwaita 0.7 / gtk4-layer-shell 0.4
  usano **glib/gio/gdk-pixbuf 0.20**, non 0.21 (due versioni di glib
  nel grafo rompevano tutti i tipi `Propagation`/`IsA`/
  `ApplicationFlags`); `FileDialog` vuole la feature gtk4 `v4_10` e
  `CssProvider::load_from_string` la `v4_12`; `transcribe` restituisce
  `LlmError`, non `String` (mappato); `save_btn` e `state` spostati
  due volte in due closure (servono clone separati); `add_controller`
  prende il controller per valore, non per riferimento; `ComboBoxText`
  è deprecato da GTK 4.10 (sostituito con `DropDown` + `StringList`).
  Restano da verificare solo su Fedora: link, avvio, tab su tutti i
  DE, setup, voce e wake word con microfono reale.
