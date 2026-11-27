# novaos-apps — the NovaOS system apps

Three small Rust apps (GTK4/libadwaita, GPLv3) that ship in every NovaOS
image, built from source by `build/45-novaos-apps.sh`:

| App (binary) | App ID | What it does |
|---|---|---|
| `novaos-setup` | `org.novaos.Setup` | First-boot welcome: optional codec / gaming / dev packages |
| `novaos-updates` | `org.novaos.Updates` | System updates + desktop switching |
| `novaos-snapshots` | `org.novaos.Snapshots` | rpm-ostree deployments as snapshots: boot, pin, rollback |

Shared logic lives in `crates/novaos-common/` (privileged commands via
pkexec, `rpm-ostree status --json` parsing, os-release, Italian dates,
done-flags) — no GUI code, unit-tested with plain `cargo test`.

Each app crate splits the same way: the testable logic is a library
(`src/lib.rs` exposes `steps` / `checks` / `rows`), the GTK UI is the
binary in `src/main.rs`. The split exists because on the build VM the UI
can only be type-checked (no real GTK libraries for linking): the logic
tests run with `cargo test --lib`, without GTK.

## Verify (on the VM)

```sh
export PATH="$HOME/.rust-standalone/bin:$PATH"
export PKG_CONFIG_PATH="$HOME/tmp/fake-pkgconfig"   # GTK .pc stubs, type-check only
cargo test --workspace --lib --offline              # 62 tests
rustfmt --edition 2024 --check crates/*/src/*.rs
cargo check --workspace --offline
cargo-clippy clippy --workspace --offline --all-targets -- -D warnings
```

(`cargo fmt` does not work with this standalone toolchain — use `rustfmt`
directly; `cargo clippy` needs the `cargo-clippy` binary directly.)

## Honest limits

- The UIs are only type-checked here, never executed or seen; the real
  first run is on Fedora (laptop / first-boot VM test).
- Details per app in `crates/novaos-*/README.md`.
