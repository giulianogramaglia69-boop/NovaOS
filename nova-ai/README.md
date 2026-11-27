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
| Locale (Ollama) | http://localhost:11434/v1 | `qwen2.5:7b` | se il nome contiene `vision`/`vl`/`llava` | no |
| Custom | a scelta | a scelta | euristica sul nome | no |

Groq è il default a config mancante: tier gratuito, senza carta.
**Locale (Ollama, v3)**: nessun account e nessuna chiave — Ollama deve
essere installato e in esecuzione sul PC (da ollama.com; non è incluso
in NovaOS: troppo grosso) con il modello scaricato
(`ollama pull qwen2.5:7b`). La schermata di setup mostra la voce
"Locale (Ollama)" con la nota, e salta la richiesta della chiave. Il
fallback testuale già esistente copre i modelli locali che non
supportano il function calling nativo.

La chiave API va nel portachiavi di sistema via libsecret (servizio
`NovaAI`, attributo `provider=<nome>`; implementato con il CLI
`secret-tool`, senza crate D-Bus). Se il portachiavi non è disponibile,
la chiave finisce in `~/.config/nova-ai/config.toml` con permessi 0600.
La chiave non viene mai stampata nei log.

## Agent loop e conferme

```
utente → LLM → tool call → esegui → risultato → LLM → risposta
```

Max 8 iterazioni di tool per turno (20 quando un piano è attivo, v3:
il piano vive in `~/.local/share/nova-ai/plans/` e la risposta mostra
l'avanzamento). I tool di sola lettura
(`list_apps`, `open_app`, `get_*`, `system_info`, `shell` in sola
lettura, `files` in lettura, `web`) girano subito. I tool che
modificano il sistema prima restituiscono un'azione in sospeso: la UI
mostra la proposta con Approva / Rifiuta, e solo dopo l'approvazione
core esegue. La macchina a stati vive in core, non nella UI, quindi
la regola non dipende dalla UI.

**Riassunto progressivo del contesto (v3)**: la storia viene stimata
in token come caratteri/4 (budget 100k, immagini contate dal base64).
Quando supera il budget, l'agente chiede al modello di riassumere in
italiano la metà più vecchia e la sostituisce con un messaggio di
sistema "Riassunto della conversazione finora: …". Il riassunto viene
salvato in memoria (`memory/summary.txt`) a fine sessione (chiusura
chat) e ricaricato nel system prompt all'avvio successivo. Se la
chiamata di riassunto fallisce, i messaggi vecchi vengono scartati
senza riassunto (niente blocco).

**Function calling nativo**: il client invia l'array `tools` (schema
OpenAI) e legge `tool_calls` dalla risposta; tutti i tool (vecchi e
nuovi) sono registrati in un unico registry in `tools.rs`
(`tool_definitions`, schema JSON + descrizione + handler). **Fallback**:
se il provider rifiuta il parametro `tools` (HTTP 400, endpoint custom
o minimali), il client ritenta una volta senza `tools` e istruisce il
modello a rispondere con un blocco JSON `{"tool": ..., "arguments": ...}`
(`parse_text_tool_call`); il loop agente funziona identico.

## Tool (v2)

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
| `shell` | `bash -c` con timeout (default 30 s, max 300 s); stdout ≤24 KiB, stderr ≤8 KiB. Vedi "Sicurezza shell" sotto |
| `files` | `read` (cap 64 KiB), `write`, `mkdir`, `list`, `search` (ricorsiva, salta `.git`/`target`/`node_modules`, max 50 risultati), `move`, `delete` (sempre con conferma) |
| `screenshot` | `grim -s 0.5` (Niri/Hyprland/MangoWC) o `gnome-screenshot` (GNOME); il PNG (≤3 MB) torna al modello come `image_url` |
| `windows` | `list`/`focus`/`close`/`move`/`workspace` via `niri msg` (JSON) o `hyprctl`; su GNOME/COSMIC errore onesto "non supportato" |
| `system` | `os_update_check` (`bootc upgrade --check`, fallback `rpm-ostree`), `os_update` (`pkexec bootc upgrade`, con conferma), servizi `systemctl --user`, profili `powerprofilesctl`, Bluetooth `bluetoothctl`, audio `wpctl status`/`set-default` |
| `distrobox` | `list`, `create -i <immagine> -n <nome>` (conferma), `run -n <nome> -- <cmd>` (classificazione shell sul comando interno), `rm` (conferma) |
| `install_system_package` | `pkexec rpm-ostree install <pacchetto>` (conferma + avviso layering: rallenta gli aggiornamenti, serve riavvio) |
| `web` | `search` (DuckDuckGo HTML, max 8 risultati), `fetch` (testo, max 100 KiB), `download` (in `~/Downloads`, max 50 MB) — senza chiave API |

## Tool (v3) — "agente esperto"

| Tool | Come funziona |
|---|---|
| `memory` | `remember` (salva un fatto con data), `recall` (ricerca per parola chiave, max 20), `forget` (cancella i fatti che contengono la parola). Fatti in `~/.local/share/nova-ai/memory/facts.jsonl` (append-only), riassunto sessione in `summary.txt`. Niente vector DB: ricerca per sottostringa. All'avvio i fatti e l'ultimo riassunto entrano nel system prompt (disattivabile in Impostazioni) |
| `plan` | `create`/`list`/`show`/`update`/`complete`/`delete`. Piani in `~/.local/share/nova-ai/plans/<id>.json` (titolo, passi fatto/da fare). Con un piano attivo il loop agente sale a 20 iterazioni e la risposta mostra l'avanzamento (`Piano "X" (n/m): ✓ … ○ …`); all'apertura della chat una nota propone di riprendere i piani incompiuti (max 3) |
| `calendar` | Sola lettura da ICS: URL (link privato Google Calendar) o file `.ics` (Impostazioni), cache in `~/.local/share/nova-ai/calendar.ics`, refresh max ogni 15 min. Azioni: `today`, `tomorrow`, `date` (YYYY-MM-DD), `search`. Parser ICS scritto a mano (VEVENT, DTSTART/DTEND/SUMMARY/LOCATION, righe piegate, `TZID`/`VALUE=DATE`/`Z`) |
| `contacts` | Rubrica locale in `~/.local/share/nova-ai/contacts.vcf` (vCard: FN/EMAIL/TEL), ricerca per nome |
| `browser` | Firefox headless via WebDriver HTTP (geckodriver scaricato al primo uso in `~/.local/share/nova-ai/geckodriver/`, v0.37.1, stesso pattern lazy di Piper/Vosk). Azioni: `navigate`, `find` (CSS selector), `click`, `type`, `text`, `screenshot` (PNG → visione), `close`. Click e type chiedono conferma come le altre azioni mutanti. **Niente valutazione JavaScript** (scelta deliberata) |

Gli errori sono onesti e specifici (es. "nessun risultato su Flathub",
"remote Flathub non configurato", "comando non trovato"): niente
successi finti.

### Sicurezza shell

Ogni comando passa per una classificazione in tre classi, per segmento
di pipeline (i `|` dentro le virgolette non separano):

- **Rifiutato** (mai eseguito, nemmeno con conferma): `rm -rf` su `/`,
  `~`, `$HOME`, `/home`, `/usr`, `/etc`, `/var`, `/boot`, `/opt`;
  `mkfs*`/`mkswap`/`fdisk`/`parted`; `dd … of=/dev/*`; fork bomb
  `:(){:|:&};:`; reindirizzamento su `/dev/*`; `shred` su `/dev/*`.
  Anche dietro `sudo`/`pkexec` (`sudo rm -rf /` è comunque rifiutato).
- **Modifica il sistema** (serve conferma in chat): prima parola in
  `rm, mv, cp, mkdir, touch, ln, chmod, systemctl, rpm-ostree, bootc,
  flatpak, dnf, distrobox, podman, kill, git, sed, curl, wget, …`
  (interpreti e VCS chiedono sempre, anche per letture: possono
  scrivere); qualsiasi reindirizzamento `>`/`>>`; pipe verso shell.
  In dubbio, un comando è "modifica" (default sicuro).
- **Sola lettura**: tutto il resto (`ls`, `cat`, `df`, `grep`, …),
  eseguito subito.

La denylist viene ricontrollata anche al momento dell'esecuzione
(dopo l'approvazione): un comando rifiutato non passa mai.

**Sandbox bubblewrap (v3)**: i comandi di sola lettura girano sotto
`bwrap --ro-bind / / --dev /dev --proc /proc --tmpfs /tmp --ro-bind
$HOME $HOME --unshare-net` — home leggibile ma non scrivibile, niente
rete, solo `/tmp` scrivibile. I comandi mutanti confermati girano
FUORI dalla sandbox (l'utente li ha approvati e devono poter
scrivere); i negati restano negati. Se `bwrap` manca, il comando gira
senza sandbox ma con una nota una-tantum in chat ("sandbox non
disponibile: installa bubblewrap"). Nota ambiente: su questa VM
`--unshare-net` fallisce (`bwrap: loopback: Failed RTM_NEWADDR`), e
l'implementazione riprova senza `--unshare-net` quando bwrap stesso
fallisce così — la sandbox resta (home sola lettura), cade solo
l'isolamento di rete.

### Avvisi proattivi (v3)

`nova-ai --check` controlla (a) aggiornamenti di sistema disponibili
(`rpm-ostree upgrade --check`), (b) batteria bassa (`upower -d`:
in scarica e sotto il 20%), (c) disco quasi pieno (`df -P /`, uso
≥90%) e manda una notifica desktop via `notify-send` quando scatta
una soglia (stdout se `notify-send` manca). Dalle Impostazioni
l'utente può attivare un timer systemd utente
(`nova-ai-watch.timer`, ogni ora, `Persistent=true`) che lancia
`nova-ai --check`; lo stesso interruttore lo spegne. La logica delle
soglie è pura e testata con fixture (niente notifiche reali nei test).

### File: regole sui percorsi

I percorsi relativi si risolvono contro `$HOME`. Leggere, elencare e
cercare sotto `~` è libero; scrivere o creare directory fuori da `~`
chiede conferma; spostare un file fuori da `~` chiede conferma;
cancellare chiede sempre conferma e rimuove solo file o directory
vuote (mai ricorsivo).

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
  "Ehi Nova" (opt-in, vedi sotto) e le risposte parlate (TTS, opt-in).
- **Impostazioni**: pulsante ⚙ nel header della chat. Mostra provider e
  modello, l'interruttore "Ehi Nova" con download del modello e stato,
  l'interruttore "Risposte parlate" con download di Piper + voce e
  stato, e (v3) l'interruttore "Memoria", gli "Avvisi proattivi"
  (timer orario) e la configurazione del calendario (URL ICS o file
  .ics locale).
- **Autostart**: la tab parte al login su tutti e 5 i DE. Meccanismo per
  DE (l'app è single-instance via D-Bus — `org.novaos.NovaAI` — quindi
  un doppio avvio attiva solo la tab già aperta):
  - GNOME, COSMIC: `/etc/xdg/autostart/nova-ai.desktop`
    (`X-GNOME-Autostart-enabled=true`).
  - Niri: `spawn-at-startup "nova-ai"` in `config.kdl` (Niri non legge
    l'autostart XDG).
  - Hyprland: `hl.exec_cmd("nova-ai")` in `hyprland.lua` (idem).
  - MangoWC: `exec_once=nova-ai` aggiunto in build a
    `/etc/mango/config.conf` (il default del pacchetto non ha righe di
    autostart).
  La voce nel menu applicazioni è
  `/usr/share/applications/nova-ai.desktop`.

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

## Risposte parlate — TTS offline con Piper (opt-in, default OFF)

Se attivo, la chat legge ad alta voce le risposte dell'assistente
(troncate a 800 caratteri) con una voce italiana **offline**:

- **Sintesi con Piper** (`piper` 2023.11.14-2, release ufficiale
  `rhasspy/piper`), voce **`it_IT-riccardo-x_low`** (HuggingFace
  `rhasspy/piper-voices`, ~28 MB onnx + 4 KiB json). La scelta è
  verificata: le altre voci italiane (`paola/medium`, `serena/medium`,
  `serena/high`) **non funzionano** con i binari piper rilasciati — la
  loro mappa dei fonemi contiene dittonghi multi-codepoint (`aɪ`, `aʊ`,
  …) che piper rifiuta al caricamento ("Phonemes must be one
  codepoint"). Riccardo x_low sintetizza correttamente (test reale:
  "Ciao mondo, sono NovaAI e tutto funziona." → WAV 16 kHz mono
  16-bit, 2,66 s).
- **Download al primo uso** in `~/.local/share/nova-ai/piper/`
  (binario ~26 MB + voce ~28 MB, totale ~55 MB; stesso pattern del
  modello Vosk: cartella temporanea, spostamento solo a download
  completo). Da setup (checkbox) si salva solo il flag; il download
  parte alla prima risposta parlata o dall'interruttore in
  Impostazioni (con barra di avanzamento). Persiste in config
  (`tts: true/false`, default `false`).
- **Riproduzione con `pw-play`** (PipeWire). Per sintetizzare non
  serve hardware audio; per sentire la voce serve un server audio
  attivo, altrimenti la riproduzione fallisce con un errore chiaro
  (la chat resta funzionante).

## Cosa v3 NON fa

- **Niente TTS in streaming**: la voce parte solo a risposta
  completa, e solo i primi 800 caratteri.
- **Niente gestione finestre su GNOME/COSMIC**: `windows` funziona
  solo su Niri e Hyprland (niente CLI affidabile sugli altri DE).
- **Niente screenshot senza modello vision**: se il modello
  configurato non supporta la visione, `screenshot` restituisce un
  errore che lo spiega invece di un'immagine inutile.
- **Niente azioni da root** e niente modifiche a file di sistema
  (gli aggiornamenti OS e il layering passano da `pkexec`, con il
  dialogo di autenticazione di sistema).
- **Niente tema nativo COSMIC**: `set_theme` passa da gsettings
  (GNOME); su COSMIC e sui tiling è best-effort.
- **Niente email**: v3 legge calendario e contatti, ma non la posta.
  Vedi "Email — disegno previsto" sotto.
- **Browser senza JavaScript**: il tool `browser` non valuta JS
  (niente `execute_script`): naviga, trova elementi via CSS, clicca,
  scrive e legge testo/screenshot. Le pagine che richiedono JS per
  mostrare contenuti non sono usabili.
- **Calendario con orari wall-clock**: gli orari ICS sono presi come
  scritti (`TZID` ignorato, `Z` finale tolta). I link privati Google
  Calendar esportano nell'ora del calendario (per un utente in
  Europe/Rome è esatto); eventi UTC (`Z`) da altre fonti possono
  essere sfasati dell'offset UTC.
- **Sandbox senza rete solo dove il kernel lo permette**: se
  `--unshare-net` fallisce (come su questa VM), il comando gira
  comunque in sandbox ma con la rete attiva — la home resta sola
  lettura.

### Email — disegno previsto (fuori da v3)

L'email è rimasta fuori da questo round: leggere Gmail richiede
OAuth 2.0 con account Google dell'utente, token in libsecret e
chiamate all'API Gmail — un lavoro a sé, con setup account, non un
tool in più. Il disegno previsto:

1. Setup in NovaAI (o da `nova-ai --gmail-auth`): flusso OAuth con
   `google.com/device` (stesso schema usato altrove in casa), token
   refresh salvato in libsecret (mai in file), scope sola lettura
   (`gmail.readonly`) in v1.
2. Tool `mail` in sola lettura: `search` (query Gmail), `read`
   (ultimi N, mittente/oggetto/anteprima), `unread` (conteggio).
   Niente invio in v1 (serve conferma in chat + scope `gmail.send`).
3. Cache breve in `~/.local/share/nova-ai/mail/` (max 15 min, come il
   calendario), niente download di allegati in v1.
4. La mattina, il riassunto del briefing può attingere a `mail`
   come fa oggi per il calendario.

Finché non c'è, NovaAI risponde onestamente "non ho accesso alla
posta" invece di inventare email.

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
   `wpctl`), `NetworkManager` (`nmcli`), `distrobox`, `grim`,
   `power-profiles-daemon`, `firefox` (tool `browser`), `bubblewrap`
   (sandbox shell), `upower` + `libnotify` (watcher `nova-ai --check`).
   BoxBuddy si preinstalla da Flathub con
   `build/50-flatpaks.sh` (step RUN dopo 40-nova-ai).
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

## Stato della verifica (su questa VM, 26 nov 2026)

- `nova-ai-core`: compila, `cargo test` verde (**68 test lib + 10
  integrazione**, +2 manuali `#[ignore]`), clippy + fmt puliti,
  anche con `--no-default-features` (68 lib + 10 integrazione anche
  lì). **Wake word verificata qui**: `vosk`
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
- **v2 verificata qui**: function calling nativo con mock HTTP locale
  (incluso il fallback 400 → chiamata testuale, e il loop agente che
  manda lo screenshot come `image_url` con un `grim` finto in PATH);
  classificazione shell (negati/mutanti/sola lettura, incluso
  `sudo rm -rf /`); file su directory temporanee; parsing fixture di
  `niri msg`/`hyprctl` e dei risultati DuckDuckGo. **TTS verificato
  qui**: sintesi reale offline con piper 2023.11.14-2 + voce
  `it_IT-riccardo-x_low` ("Ciao mondo, sono NovaAI e tutto
  funziona." → WAV valido 16 kHz mono, 2,66 s).
- **v3 verificata qui**: memoria (round-trip remember/recall/forget,
  caricamento in system prompt), piani (CRUD + riga di avanzamento),
  watcher (soglie batteria/disco/aggiornamenti con fixture),
  calendario (fixture ICS con TZID/VALUE=DATE/Z, eventi odierni e
  multi-giorno), contatti (vCard), browser WebDriver contro mock HTTP
  locale (navigate/find/click/type/text/screenshot/close + conferma
  per click/type), provider Local (default + niente chiave, mock),
  compattazione del contesto con mock LLM (storia da 500k caratteri
  → riassunto salvato in memoria), sandbox bwrap reale (echo in
  sandbox + scrittura in home bloccata con "Read-only file system";
  `--unshare-net` fallisce su questa VM e viene riprovato senza).
  **Quirk ambiente**: `liblzma.so` (dev) mancava su questa VM dopo
  un riavvio — il link dei test ha richiesto un symlink in
  `~/tmp/liblink/` (`liblzma.so → liblzma.so.5`); sull'immagine
  Fedora il problema non esiste (dipendenza dei `-devel`).
  **Non verificabile in questa VM**: esecuzione reale su Fedora
  (schermo reale,
  `grim`/`niri msg`/`hyprctl` veri, install Flatpak in build,
  `pkexec`/`bootc upgrade`, `pw-play` con server audio) — tutto
  rimandato a CI/laptop. Per v3 in più: Firefox + geckodriver reali
  (mai scaricati qui), timer systemd utente (mai installato qui),
  `notify-send`/`upower` reali, Ollama reale, `nova-ai --check` con
  `rpm-ostree` vero.
