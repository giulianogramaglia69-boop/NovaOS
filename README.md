# NovaOS

NovaOS is an rpm-ostree / bootc distro in the style of Bluefin: five desktop
images built from one Containerfile on Fedora 45, switchable by rebasing —
no reinstall, with automatic rollback to the previous image.

| Image | Desktop | Base | Tag |
|---|---|---|---|
| `novaos-gnome` | GNOME 51 | `silverblue-main:45` | `ghcr.io/giulianogramaglia69-boop/novaos-gnome:45` |
| `novaos-cosmic` | COSMIC 1.10 | `base-main:45` | `ghcr.io/giulianogramaglia69-boop/novaos-cosmic:45` |
| `novaos-hyprland` | Hyprland 0.57 + Dux Shell | `base-main:45` | `ghcr.io/giulianogramaglia69-boop/novaos-hyprland:45` |
| `novaos-mangowc` | MangoWM 0.19 + Dux Shell | `base-main:45` | `ghcr.io/giulianogramaglia69-boop/novaos-mangowc:45` |
| `novaos-niri` | Niri + Dux Shell | `base-main:45` | `ghcr.io/giulianogramaglia69-boop/novaos-niri:45` |

Dux Shell is the bar / launcher / control center / notifications / lock screen
on the three tiling images (niri, Hyprland, MangoWM). GDM is the display
manager on every image. License: GPLv3.

## Switching desktop

Changing desktop = rebasing to another image, then rebooting. Your home
directory and user data are untouched.

```bash
# From one NovaOS image to another (signed):
sudo bootc switch --enforce-container-sigpolicy ghcr.io/giulianogramaglia69-boop/novaos-niri:45
sudo systemctl reboot
```

First switch from a stock Fedora Atomic install (Silverblue/Kinoite or
`base-main`): the stock install has no signature policy for GHCR, so the
first rebase is unverified, then the signed one after reboot:

```bash
sudo rpm-ostree rebase ostree-unverified-registry:ghcr.io/giulianogramaglia69-boop/novaos-niri:45
systemctl reboot
sudo rpm-ostree rebase ostree-image-signed:docker://ghcr.io/giulianogramaglia69-boop/novaos-niri:45
```

Rollback to the previous image if anything breaks:

```bash
sudo rpm-ostree rollback
# or pick the previous boot entry in the bootloader menu
```

## Building locally

Prerequisites: `podman`, `just`, `jq`, `skopeo` (optional, for tag
collision checks). The desktop is encoded in the image name.

```bash
just build novaos-niri 45        # builds localhost/novaos-niri:45
just build novaos-gnome 45
just build-qcow2 novaos-niri 45  # bootable qcow2 in output/qcow2/
just build-iso novaos-niri 45    # installable ISO in output/bootiso/
just run-vm-qcow2                # boot the qcow2 in QEMU/KVM
```

CI (`.github/workflows/build.yml`) builds all five images on every push to
`main` and pushes `:45` + dated `:45.YYYYMMDD` tags to GHCR, signed keyless
with Cosign/Sigstore OIDC. `.github/workflows/build-disk.yml` builds ISOs
on manual dispatch.

## How it is built

One repo, one `Containerfile`, one build arg `DE` (`gnome` / `cosmic` /
`hyprland` / `mangowc` / `niri`). The GitHub Actions matrix builds each
desktop from the same file. Build scripts live in `build/` (numbered, like
the finpilot template this is adapted from), per-desktop package lists in
`desktops/<de>/packages.txt`, files shipped into the image in
`system_files/` (session files, starter configs in `/etc/skel/.config`).

Bases (verified to exist on GHCR): `ghcr.io/ublue-os/silverblue-main:45`
and `ghcr.io/ublue-os/base-main:45`. The template this adapts is
[projectbluefin/finpilot](https://github.com/projectbluefin/finpilot).
Differences from finpilot: no `projectbluefin/common` / `ublue-os/brew` OCI
stages (NovaOS ships only its own files), and no stable/stable-testing
promotion lanes yet (main publishes `:45` directly).

## Branding

The NovaOS logo (crystalline "N", blue + gold) and four wallpapers ship in
every image:

- **Wallpapers** — `/usr/share/backgrounds/novaos/`: `novaos-dark.jpg`
  (default everywhere), `novaos-light.jpg`, `novaos-galaxy.jpg`,
  `novaos-logo.jpg` (logo lockup poster).
- **Default wallpaper per desktop** — GNOME via a gschema override
  (`90-novaos.gschema.override`, desktop + lock screen); COSMIC via the
  system cosmic-config files (`com.system76.CosmicBackground[.Dark|.Light]`,
  dark wallpaper for dark mode, light for light mode); Niri via `swaybg`
  (spawned from the starter config); Hyprland via `hyprpaper`
  (`hyprpaper.conf` + autostart in `hyprland.lua`). MangoWC has no
  wallpaper mechanism wired yet — the wallpapers are installed, pick one
  from Dux Shell.
- **Terminal** — `fastfetch` shows a blue/gold ASCII "N"
  (`/etc/fastfetch/config.jsonc` + `/usr/share/novaos/fastfetch-logo.txt`);
  a per-user config in `~/.config/fastfetch/` overrides it.
- **OS logo** — `LOGO=novaos-logo` in os-release, icon at
  `/usr/share/icons/hicolor/256x256/apps/novaos-logo.png` (fallback
  `/usr/share/pixmaps/novaos-logo.png`).
- **Login screen** — GDM shows the logo (dconf `gdm.d`) and the dark
  wallpaper as background (`build/30-branding.sh` patches it into the
  GNOME Shell theme gresource; dconf cannot set the greeter background
  since GNOME 48). The patch is guarded: if it fails, the original theme
  is restored and the build continues.

## NovaAI

Every image ships **NovaAI**, the system AI agent (`/usr/bin/nova-ai`,
source in `nova-ai/`, Rust + GTK4/libadwaita, GPLv3). A small tab sits
at the bottom-right edge of the screen ("✦ NovaAI"); clicking it opens
a chat to manage the system in Italian. NovaAI v3 is an expert agent:
native function calling (with a textual fallback for providers that
reject the `tools` parameter), shell commands (read-only directly,
mutating ones only after confirmation, catastrophic ones always
refused), files, screenshots it can see, window management on
Niri/Hyprland, OS updates and services, Distrobox containers, system
package installs via rpm-ostree layering, and web search — all with
confirmation in the chat before anything mutating. Details and honest
limits in `nova-ai/README.md`.

**NovaAI v3** adds: persistent memory (facts + last-session summary,
toggle in settings), multi-step plans (up to 20 tool iterations while
a plan is active, progress shown in chat), proactive hourly checks
(`nova-ai --check`: system updates, low battery, full disk — desktop
notifications via a user systemd timer, opt-in), a read-only calendar
from a private ICS link or local `.ics` file and a local vCard
address book, a real headless-Firefox browser (WebDriver, geckodriver
downloaded on first use), an offline provider (**Ollama**, local,
no API key — Ollama itself is not in the image, it is installed by
the user), progressive summarisation of long chats, and a bubblewrap
sandbox for read-only shell commands (home read-only, no network).
**Not in v3: email** (Gmail OAuth is a separate job — the planned
design is in `nova-ai/README.md`); windows still only on
Niri/Hyprland; the browser does not run JavaScript.

- **First boot** — the tab is autostarted at login; the first launch
  asks for the provider and API key: OpenAI, **Groq** (default,
  free tier), OpenRouter, **Locale (Ollama)** (no key), or a custom
  OpenAI-compatible endpoint. The key goes to the system keyring
  (libsecret), never in a file.
- **Voice** — hold the 🎤 button to talk (transcribed via the
  provider's Whisper); attach files and photos in the chat. Spoken
  answers are opt-in (off by default): offline Piper TTS, Italian
  voice, downloaded on first use (~55 MB).
- **Wake word "Ehi Nova"** — opt-in (off by default): the setup asks
  whether to enable it, and the chat settings have the same switch.
  Detection is fully offline with Vosk (Italian model, ~47 MB,
  downloaded on first enable, never in the image); the mic indicator
  dot on the tab shows when it is listening.
- **Build** — `build/40-nova-ai.sh` compiles it from source in the
  image build (toolchain removed afterwards) and installs
  `libvosk.so` from the official upstream prebuilt zip (Fedora does
  not package it). If that download fails, NovaAI still builds
  without the wake-word feature.

## NovaOS system apps

Three small Rust apps (GTK4/libadwaita, GPLv3, source in `novaos-apps/`,
shared logic in `novaos-apps/crates/novaos-common/`) ship in every image,
built from source by `build/45-novaos-apps.sh`:

- **Configurazione NovaOS** (`novaos-setup`, `org.novaos.Setup`) — first-boot
  welcome, autostarted once at first login (XDG autostart on GNOME/COSMIC,
  spawn/exec_once on the tiling compositors; `--first-run` exits at once if
  already seen, and it is re-openable from the app menu). Three optional
  packages, installed only if the user ticks them, with progress and errors
  in the UI: **multimedia codecs** (enables RPM Fusion *free* only, then
  layers `ffmpeg` + GStreamer plugins — `ffmpeg-free` is replaced via
  `rpm-ostree override remove` if present), **gaming** (Steam Flatpak +
  `mangohud`/`gamemode`, also i686), **dev** (`rustup` as the user, VS Code
  Flatpak, `git`). Layered packages need a reboot; Flatpak/rustup do not.
- **Aggiornamenti NovaOS** (`novaos-updates`, `org.novaos.Updates`) — system
  updates + desktop switching in one app: checks with `rpm-ostree upgrade
  --check` (exit 77 = up to date) and `bootc upgrade --check` (string
  "No changes in:"), applies the staged upgrade (`bootc upgrade`, fallback
  `rpm-ostree upgrade`), updates Flatpaks (system + user), and switches
  desktop with `bootc switch ghcr.io/giulianogramaglia69-boop/novaos-<de>:45`
  (fallback `rpm-ostree rebase ostree-unverified-registry:…`), warning that
  layered packages may need reinstalling while Flatpaks and home stay.
- **Snapshot NovaOS** (`novaos-snapshots`, `org.novaos.Snapshots`) — lists
  every rpm-ostree deployment as a snapshot (version, date, checksum,
  booted/pinned/staged) from `rpm-ostree status --json`; boot a deployment
  (`rpm-ostree deploy <checksum>`), go back (`rpm-ostree rollback`), pin /
  unpin (`ostree admin pin <index>` — there is no `rpm-ostree pin`).

Privileged operations go through pkexec with confirmation in the UI; the
polkit agent is `lxpolkit`, autostarted on every DE (GNOME has its own
inside gnome-shell; COSMIC has none of its own). Details and honest
limits in `novaos-apps/crates/novaos-*/README.md`.

## Software

- **GNOME Software** is installed on the three tiling images (niri,
  Hyprland, MangoWC) — GNOME already ships it, COSMIC ships Cosmic Store.
  On Atomic it installs Flatpaks and stages OS updates for the next reboot.
- **Flathub** is preconfigured system-wide
  (`/usr/share/flatpak/remotes.d/flathub.flatpakrepo`), so the store has
  apps on first boot. The Fedora Flatpak remote is deliberately *not*
  added (its first-boot service is removed in `90-cleanup.sh`).

## Distrobox / BoxBuddy

Every image ships **Distrobox** (RPM, from the official Fedora repos)
and **BoxBuddy** preinstalled from Flathub
(`io.github.dvlv.boxbuddyrs`, system scope, installed by
`build/50-flatpaks.sh`). Distrobox runs other distros in containers
(Ubuntu, Arch, Debian…) for software that is not a Flatpak and does
not belong layered on the host; BoxBuddy is its graphical UI. NovaAI
can create, list, run commands in and remove containers from the chat
(`distrobox` tool, mutating operations with confirmation); BoxBuddy
does the same by hand. Note: BoxBuddy does not bundle Distrobox — it
drives the system binary, which is why both are installed.

## Image signing

Images are signed keyless (Cosign/Sigstore OIDC) in CI — no keypair to
manage. Honest consequence, same as finpilot: the `image-ref` baked into
the image uses the *unverified* transport
(`ostree-unverified-image:docker://…`), because a keyless GitHub Actions
identity cannot be matched by a `containers/policy.json` entry on the
device. `bootc switch --enforce-container-sigpolicy` still verifies the
signature at switch time; the baked ref only describes the update source.

## Known limits

- **COSMIC is Wayland-only.** No X11 session ships; X11 apps run through
  XWayland (pulled in by `cosmic-comp`). The `cosmic-greeter` package (and
  `greetd`) comes along with the group but stays inactive — GDM is the
  display manager on every image.
- **MangoWC is not in Fedora.** It comes from the third-party Terra repo
  (Fyra Labs) as package `mangowm` 0.19.0 (binary `/usr/bin/mango`).
  Details and fallbacks (COPR `dennemann/MangoWC`, source build) in
  `desktops/mangowc/README.md`. Third-party repos can lag or disappear.
- **Hyprland is not in Fedora 45** (checked against the F45 release +
  updates repodata). It comes from the community COPR `sdegler/hyprland`
  (0.57.0); the well-known `solopasha/hyprland` COPR has no F45 builds.
- **Hyprland 0.55+ uses Lua configs.** The starter config is
  `~/.config/hypr/hyprland.lua` (`hl.config`, `hl.bind`, `hl.on`); old
  hyprlang `.conf` files do not load on 0.57. hyprlock/hypridle/hyprpaper
  still use hyprlang.
- **Dux Shell in the COPR is 1.6.3** (`duxlabs/dms`, F45 build at time of
  writing) while upstream is at v2.1 — the COPR lags upstream. `dms`,
  `quickshell` and `matugen` resolve from the COPR's dependency repo
  (`duxlabs/dux-repo`) or Fedora, whichever is newer.
- **One image per desktop, by design.** Putting all five desktops in one
  image is possible but wrong: image bloat, conflicting portals
  (`xdg-desktop-portal-gnome` vs `-hyprland` vs `-wlr`), secret-provider
  fights, and two compositors (`cosmic-comp` next to GNOME's `mutter`).
- **No out-of-tree kernel modules.** On ostree, akmods cannot compile at
  boot; modules must be built into the image. Irrelevant on AMD APUs
  (in-tree `amdgpu`), but a real limit if NVIDIA support is ever wanted.
- **Fedora 45 EOL is 8 Dec 2027.** Moving to F46 is a one-arg change
  (`FEDORA_MAJOR_VERSION`), but every desktop must be re-tested.
- **Wallpapers are 1179×658 (phone-sized).** On a 1080p+ screen they
  upscale and look slightly soft. Higher-resolution sources would fix it.
- **COSMIC lock screen has no system default.** The greeter reads the
  per-user state, not the system config, so it may show COSMIC's built-in
  nebula until the user picks a wallpaper once.
- **Layering is discouraged.** Install what you need into the image or use
  Flatpaks/Distrobox; `rpm-ostree install` on top of NovaOS works but
  slows updates and can block `bootc upgrade` (use `rpm-ostree upgrade`
  after layering).
- **The three system apps never ran on a real system.** Their UI is only
  type-checked on the build VM (no real GTK there); the pkexec flows, the
  ffmpeg-free conflict path, `bootc switch`, deploy/rollback and the
  `ostree admin status` format are all to be confirmed at first boot.

## Repository layout

```
Containerfile                  # one file, ARG DE selects the desktop
build/                         # 00-image-info, 10-overlay, 20-packages-and-services,
                               # 30-branding, 40-nova-ai, 45-novaos-apps, 50-flatpaks, 90-cleanup, copr-helpers (adapted from finpilot)
nova-ai/                       # NovaAI system agent (Rust: core + GTK UI)
novaos-apps/                   # the three system apps (setup / updates / snapshots) + novaos-common
desktops/<de>/packages.txt     # per-desktop package lists (+ mangowc/README.md)
system_files/                  # files mirrored into the image (session files,
                               # /etc/skel/.config starter configs)
iso/                           # bootc-image-builder configs (disk.toml, iso.toml)
.github/workflows/             # build.yml (matrix, 5 images) + build-disk.yml (ISOs)
Justfile                       # just build / build-qcow2 / build-iso / run-vm-*
```

## License

GPLv3 — see `LICENSE`. Copyright (C) 2026 Giuliano Gramaglia.
