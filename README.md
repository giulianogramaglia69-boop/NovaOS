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
- **Layering is discouraged.** Install what you need into the image or use
  Flatpaks/Distrobox; `rpm-ostree install` on top of NovaOS works but
  slows updates and can block `bootc upgrade` (use `rpm-ostree upgrade`
  after layering).

## Repository layout

```
Containerfile                  # one file, ARG DE selects the desktop
build/                         # 00-image-info, 10-overlay, 20-packages-and-services,
                               # 90-cleanup, copr-helpers (adapted from finpilot)
desktops/<de>/packages.txt     # per-desktop package lists (+ mangowc/README.md)
system_files/                  # files mirrored into the image (session files,
                               # /etc/skel/.config starter configs)
iso/                           # bootc-image-builder configs (disk.toml, iso.toml)
.github/workflows/             # build.yml (matrix, 5 images) + build-disk.yml (ISOs)
Justfile                       # just build / build-qcow2 / build-iso / run-vm-*
```

## License

GPLv3 — see `LICENSE`. Copyright (C) 2026 Giuliano Gramaglia.
