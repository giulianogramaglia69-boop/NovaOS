# MangoWC on NovaOS — source verification (25 Nov 2026)

MangoWC is **not** in the official Fedora 45 repos (checked against the F45
release + updates repodata). The image therefore gets it from a third-party
source, selected by the `MANGOWC_SOURCE` build arg (`terra` by default).

## Verified: Terra repo (Fyra Labs) — used by default

- Repo: `https://repos.fyralabs.com/terra45` (added at build time via
  `terra-release`, see `build/20-packages-and-services.sh`).
- **Package name is `mangowm`, not `mangowc`.** Verified against the
  terra45 repodata primary (2026-11-25): `mangowm 0.19.0-1.fc45`
  (plus `mangobar 0~20260527git.c0138d9-1.fc45`). No package named
  `mangowc` exists in the repo.
- The package ships the binaries `/usr/bin/mango`, `/usr/bin/mangonag`,
  `/usr/bin/mmsg` and default configs `/etc/mango/config.conf` +
  `/etc/mango/config.toml`. It ships **no** Wayland session file, so NovaOS
  provides `system_files/usr/share/wayland-sessions/mangowm.desktop`
  (Exec `/usr/bin/mango`).
- Upstream renamed itself: `DreamMaoMao/mangowc` → org `mangowm`, repo
  `mango`. Dux Shell supports mangowm 0.18+; Terra ships 0.19.0.

## Fallbacks (in order)

1. **COPR `dennemann/MangoWC`** (`MANGOWC_SOURCE=copr`): community COPR
   packaging of MangoWC. Not verified for F45 at build time — the build
   will fail loudly if the COPR has no F45 build.
2. **Build from source** (`MANGOWC_SOURCE=source`): not wired into the
   build script yet. Upstream is `github.com/DreamMaoMao/mangowc`
   (now org `mangowm`); needs meson + wlroots 0.19.2 + scenefx 0.4.1.

## Autostart on NovaOS (added 26 Nov 2026)

The package's default `/etc/mango/config.conf` has **no autostart lines**
(verified against the Terra mangowm 0.19.2 RPM): nothing starts Dux
Shell, a wallpaper, or NovaAI. `build/20-packages-and-services.sh`
therefore appends three `exec_once` lines to `/etc/mango/config.conf`
after the package install (appended at build time, not overlaid from
`system_files/`, because the file is owned by the package and the
install would clobber the overlay):
`exec_once=dms run`, `exec_once=swaybg -m fill -i
/usr/share/backgrounds/novaos/novaos-dark.jpg`, `exec_once=nova-ai`.
`swaybg` comes from `desktops/mangowc/packages.txt`. If the config file
is absent (e.g. the COPR fallback ships a different path), the step only
warns and the build continues.

## Supply-chain note

Terra is a third-party repo: it can lag behind Fedora releases, disappear,
or break. If the Terra build ever fails in CI, the fix is to switch
`MANGOWC_SOURCE` to `copr` or to vendor a MangoWC build from source.
