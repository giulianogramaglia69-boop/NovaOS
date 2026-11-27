# NovaOS Aggiornamenti

App di sistema (`org.novaos.Updates`, binario `novaos-updates`): controlla e
applica gli aggiornamenti di sistema, aggiorna i Flatpak e cambia desktop.

## Cosa fa

- **Sistema corrente**: nome e variante da `/etc/os-release` (`PRETTY_NAME`,
  `VARIANT_ID`), deployment avviato da `rpm-ostree status --json`
  (versione, data, checksum corto). Se esiste un deployment staged, mostra
  "Aggiornamento già preparato, in attesa di riavvio" con il pulsante
  "Riavvia per applicare".
- **Controllo aggiornamenti** (pulsante "Controlla aggiornamenti", un
  verdetto per sorgente + verdetto combinato):
  - `rpm-ostree upgrade --check` — exit 77 = nessun aggiornamento,
    0 = aggiornamenti disponibili (documentato nel man).
  - `bootc upgrade --check` — "nessun update" se l'output contiene la
    stringa `No changes in:` (come fa uupd); l'exit code di bootc non è
    documentato e non viene usato.
  - `flatpak remote-ls --system --updates` e `--user --updates` —
    elencano gli aggiornamenti senza applicarli.
- **Applica aggiornamento di sistema**: dopo conferma, in streaming nel
  log `pkexec bootc upgrade`; se bootc manca o fallisce, fallback
  `pkexec rpm-ostree upgrade` (loggato). A successo: notifica + pulsante
  "Riavvia ora".
- **Aggiorna Flatpak**: in streaming `pkexec flatpak update --system -y`
  e poi `flatpak update --user -y` (senza pkexec), esito per ambito nel log.
- **Cambio desktop**: una riga per ciascuno dei 5 DE (GNOME, COSMIC,
  Hyprland, MangoWC, Niri) con ref immagine GHCR e badge "In uso".
  Conferma con avviso onesto sui pacchetti layered, poi in streaming
  `pkexec bootc switch <ref>`; se fallisce, pulsante
  "Prova con rpm-ostree rebase" (`pkexec rpm-ostree rebase
  ostree-unverified-registry:<ref>`). A successo: pulsante "Riavvia ora".
- **Log**: TextView monospace con autoscroll; "Riavvia ora" (conferma →
  `pkexec systemctl reboot`) visibile quando serve.

## Comandi usati

| Operazione | Comando |
|---|---|
| Stato deployment | `rpm-ostree status --json` |
| Check sistema | `rpm-ostree upgrade --check`, `bootc upgrade --check` |
| Check Flatpak | `flatpak remote-ls --system/--user --updates` |
| Upgrade sistema | `pkexec bootc upgrade` → fallback `pkexec rpm-ostree upgrade` |
| Upgrade Flatpak | `pkexec flatpak update --system -y`, `flatpak update --user -y` |
| Cambio desktop | `pkexec bootc switch <ref>` → fallback `pkexec rpm-ostree rebase ostree-unverified-registry:<ref>` |
| Riavvio | `pkexec systemctl reboot` |
| Notifica | `notify-send` (via `novaos-common`) |

`rpm-ostreed-automatic.timer` è disabilitato nell'immagine
(`90-cleanup.sh`): niente updater automatico in conflitto; questa app fa
gli upgrade manuali.

## Architettura

- `src/main.rs`: UI libadwaita (stesso pattern di `nova-ai-ui`:
  `adw::Application` single-instance, worker thread + `async_channel`
  verso il main loop). Tutte le operazioni lunghe girano in worker thread.
- `src/checks.rs`: logica pura dei verdetti (exit code rpm-ostree,
  stringa `No changes in:` di bootc, parsing di `flatpak remote-ls
  --updates`) con unit test. Niente UI e niente rete nei test.

## Limiti e TODO (da confermare su Fedora reale)

- **L'UI si può solo type-checkare su questa VM** (niente GTK reale):
  `cargo check` con i `.pc` stub in `$HOME/tmp/fake-pkgconfig`.
  L'app non è mai stata eseguita, né su Fedora reale né altrove.
- Il formato esatto di `flatpak remote-ls --updates` e l'output di
  `bootc upgrade --check` quando ci sono aggiornamenti vanno confermati
  sul sistema reale; se il parse non torna, il verdetto diventa
  "Non determinabile" invece di indovinare (vedi TODO in `checks.rs`).
- I flussi pkexec (agente polkit del DE, password) e lo switch su
  hardware vero non sono mai stati provati da questa app.
- Su sistemi senza sorgente immagine, `bootc upgrade --check` fallisce
  con "No image source specified": viene riportato come "Non
  determinabile" senza bloccare il check rpm-ostree.
