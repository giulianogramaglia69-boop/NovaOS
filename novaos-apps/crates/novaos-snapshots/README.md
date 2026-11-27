# novaos-snapshots

App libadwaita (`org.novaos.Snapshots`, binario `novaos-snapshots`) che
elenca i deployment rpm-ostree come snapshot del sistema, con azioni
per avviarne uno, pinnarlo e tornare al precedente.

## Cosa fa

- **Elenco snapshot**: legge `rpm-ostree status --json` (via
  `novaos-common::ostree::status`) e mostra una riga per deployment con
  etichetta versione (o checksum corto), data leggibile, checksum corto,
  origine (se presente) e badge **Avviato** / **In attesa di riavvio**
  (staged) / **Pinnato**. Pulsante **Aggiorna** in header per ricaricare.
- **Avvia**: dialogo di conferma → `pkexec rpm-ostree deploy <checksum>`
  in streaming nel registro → a successo compare **Riavvia ora**.
- **Pin / Rimuovi pin**: risolve l'indice del deployment con
  `ostree admin status` (prima senza privilegi, poi con pkexec) e
  `pin_index_from_admin_status`; poi `pkexec ostree admin pin <indice>`
  o `pkexec ostree admin pin --unpin <indice>`, e ricarica la lista.
  Se l'indice non si trova, mostra un errore e non fa nulla: l'indice
  non viene mai indovinato.
- **Torna al precedente**: dialogo di conferma →
  `pkexec rpm-ostree rollback` → a successo compare **Riavvia ora**.
- **Riavvia ora**: visibile dopo deploy/rollback riusciti; conferma →
  `pkexec systemctl reboot`.
- **Registro**: TextView monospace con autoscroll per output ed errori
  dei comandi privilegiati; gli errori sono sempre visibili anche nel
  messaggio di stato sopra la lista.

Se `rpm-ostree` manca o `status` fallisce, mostra:
"Questo strumento funziona solo su NovaOS / Fedora Atomic" + dettaglio
errore.

## Comandi usati

| Azione | Comando |
| --- | --- |
| Elenco | `rpm-ostree status --json` |
| Avvia | `pkexec rpm-ostree deploy <checksum>` |
| Indice pin | `ostree admin status` (poi pkexec se serve) |
| Pin | `pkexec ostree admin pin <indice>` |
| Rimuovi pin | `pkexec ostree admin pin --unpin <indice>` |
| Rollback | `pkexec rpm-ostree rollback` |
| Riavvio | `pkexec systemctl reboot` |

Non esiste `rpm-ostree pin`: il pin è sempre `ostree admin pin`.

## Struttura

- `src/main.rs`: UI (adw::Application single-instance, worker thread +
  `async_channel` verso il main loop — stesso pattern di `nova-ai-ui`),
  dialoghi di conferma, comandi privilegiati in streaming, registro.
- `src/rows.rs`: logica di presentazione (etichette riga, badge,
  disponibilità azioni) con unit test, senza codice UI.

## Limiti / TODO (da verificare su Fedora reale)

- **L'UI si può solo type-checkare su questa VM** (niente GTK reale):
  `cargo check` con `PKG_CONFIG_PATH=$HOME/tmp/fake-pkgconfig`.
  La finestra non è mai stata eseguita né vista.
- **Formato di `ostree admin status`**: il parse in `novaos-common`
  (`* <osname> <checksum> (<versione>)`, eventuale `(pinned)`) va
  confermato sul sistema reale. Se non corrisponde, il pin si
  disabilita da solo con spiegazione.
- **Lettura di `ostree admin status` da non-root**: non è certo che
  funzioni senza privilegi; il codice riprova con pkexec se fallisce.
- **Flussi pkexec**: mai provati qui. Se manca l'agente polkit
  (lxpolkit sui tiling, gnome-shell su GNOME), pkexec fallisce e l'app
  mostra l'errore suggerendo il terminale.
- **Deploy / rollback / riavvio**: mai eseguiti su hardware vero.
- Il pulsante **Pin** resta disponibile anche sul deployment avviato
  (pinnare lo snapshot corrente lo protegge dalla pulizia automatica);
  **Avvia** invece è nascosto per il booted.
