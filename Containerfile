###############################################################################
# NovaOS — an rpm-ostree distro like Bluefin, with interchangeable desktops.
#
# One Containerfile builds all five images. The matrix in
# .github/workflows/build.yml (or `just build`) picks the desktop with the DE
# build arg and the matching base image:
#
#   DE            BASE_IMAGE_NAME    Image
#   gnome         silverblue-main    novaos-gnome
#   cosmic        base-main          novaos-cosmic
#   hyprland      base-main          novaos-hyprland
#   mangowc       base-main          novaos-mangowc
#   niri          base-main          novaos-niri
#
# Adapted from the projectbluefin/finpilot template (official Bluefin
# template). Differences from finpilot: no projectbluefin/common or ublue-os
# brew OCI stages (NovaOS ships only its own files, so its identity stays
# clean), and the base image is ghcr.io/ublue-os/*-main instead of
# quay.io/fedora-ostree-desktops/*.
#
# Signing is keyless OIDC via Cosign/Sigstore (see build.yml), like finpilot:
# no keypair to manage. Note the honest consequence, same as finpilot: the
# image-ref written by 00-image-info.sh uses the unverified transport,
# because a keyless GitHub Actions identity cannot be matched by a
# containers/policy.json entry on the device.
###############################################################################

ARG FEDORA_MAJOR_VERSION="45"
ARG BASE_IMAGE_NAME="silverblue-main"
ARG DE="gnome"

# Context stage - local build scripts and system files only.
FROM scratch AS ctx
COPY build /build
COPY system_files /system_files
COPY desktops /desktops
COPY nova-ai /nova-ai

# Base image. GNOME rides on silverblue-main (GNOME 51 on F45); the other four
# desktops are installed on top of the desktop-less base-main.
FROM ghcr.io/ublue-os/${BASE_IMAGE_NAME}:${FEDORA_MAJOR_VERSION}

# Image identity - these define how bootc, fastfetch, and the ublue ecosystem
# recognize your image. The Justfile / CI supply them as build args.
ARG IMAGE_NAME="novaos-gnome"
ARG IMAGE_VENDOR="giulianogramaglia69-boop"
ARG UBLUE_IMAGE_TAG="45"
ARG BASE_IMAGE_NAME="silverblue-main"
ARG FEDORA_MAJOR_VERSION="45"
ARG DE="gnome"
# MangoWC source (terra | copr | source) and package name. Verified 25 Nov 2026:
# the Terra repo for F45 ships the compositor as `mangowm` 0.19.0 (binary `mango`).
ARG MANGOWC_SOURCE="terra"
ARG MANGOWC_PKG="mangowm"
ARG VERSION=""

### MODIFICATIONS
## Build scripts run in order: image identity, runtime overlays, desktop
## packages and services, branding (logo + wallpapers + login screen),
## NovaAI (the system AI agent, built from source), then cleanup. The DE
## build arg selects the desktop inside 20-packages-and-services.sh.
## Scripts are invoked via `bash`, not executed directly: files pushed to
## GitHub through the API lose their executable bit, and a direct call fails
## with "Permission denied" (exit 126) — that killed the first CI run.

RUN --mount=type=bind,from=ctx,source=/,target=/ctx \
    --mount=type=tmpfs,dst=/boot \
    --mount=type=tmpfs,dst=/tmp \
    bash /ctx/build/00-image-info.sh

# Set dnf options before build scripts (persists across subsequent RUN layers)
RUN cp /etc/dnf/dnf.conf /etc/dnf/dnf.conf.tmp \
    && mv /etc/dnf/dnf.conf.tmp /etc/dnf/dnf.conf \
    && dnf5 config-manager setopt keepcache=1 install_weak_deps=0

### RUNTIME OVERLAYS
## Copies system_files/ into the image (session files, skel configs).
## Installs no packages and enables no services: GDM is enabled in
## 20-packages-and-services.sh, after it is installed on base-main images.
RUN --mount=type=bind,from=ctx,source=/,target=/ctx \
    --mount=type=cache,dst=/var/cache/libdnf5 \
    --mount=type=cache,dst=/var/cache/rpm-ostree \
    --mount=type=tmpfs,dst=/boot \
    --mount=type=tmpfs,dst=/tmp \
    bash /ctx/build/10-overlay.sh

### DESKTOP PACKAGES AND SERVICES
## Installs the desktop's RPM/COPR packages from desktops/<de>/packages.txt
## (plus the COSMIC group) and enables the services they provide.
RUN --mount=type=bind,from=ctx,source=/,target=/ctx \
    --mount=type=cache,dst=/var/cache/libdnf5 \
    --mount=type=cache,dst=/var/cache/rpm-ostree \
    --mount=type=tmpfs,dst=/boot \
    --mount=type=tmpfs,dst=/tmp \
    bash /ctx/build/20-packages-and-services.sh

### BRANDING
## Applies the NovaOS logo and wallpapers (fastfetch logo, GNOME/COSMIC/
## niri/Hyprland default wallpapers, GDM login logo + background). Runs
## after the desktop packages are installed; nothing in it is fatal.
RUN --mount=type=bind,from=ctx,source=/,target=/ctx \
    --mount=type=cache,dst=/var/cache/libdnf5 \
    --mount=type=cache,dst=/var/cache/rpm-ostree \
    --mount=type=tmpfs,dst=/boot \
    --mount=type=tmpfs,dst=/tmp \
    bash /ctx/build/30-branding.sh

### NOVAAI
## Builds the NovaAI system agent (nova-ai/) from source with cargo and
## installs /usr/bin/nova-ai. Runs after branding so the NovaAI desktop
## icon (novaos-logo) is already in place. Toolchain and headers are
## removed again inside the script; runtime libs stay.
RUN --mount=type=bind,from=ctx,source=/,target=/ctx \
    --mount=type=cache,dst=/var/cache/libdnf5 \
    --mount=type=cache,dst=/var/cache/rpm-ostree \
    --mount=type=tmpfs,dst=/boot \
    --mount=type=tmpfs,dst=/tmp \
    bash /ctx/build/40-nova-ai.sh

### CLEANUP
## Finalises package sources, disables third-party repos, then prunes build
## artifacts before linting.
RUN --mount=type=bind,from=ctx,source=/,target=/ctx \
    --mount=type=tmpfs,dst=/tmp \
    --mount=type=tmpfs,dst=/boot \
    bash /ctx/build/90-cleanup.sh

### /opt
## Makes /opt writeable by default.
RUN rm -rf /opt && ln -s /var/opt /opt

### IMAGE METADATA
ARG IMAGE_DESC="NovaOS — rpm-ostree desktops, interchangeable by rebase"
ARG IMAGE_CREATED=""
ARG IMAGE_LOGO_URL="https://github.com/giulianogramaglia69-boop.png"
ARG IMAGE_KEYWORDS="bootc,ublue,universal-blue,fedora-atomic,novaos"
ARG IMAGE_REF="main"
ARG SHA_HEAD_SHORT=""

LABEL org.opencontainers.image.title="${IMAGE_NAME}" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.revision="${SHA_HEAD_SHORT}" \
      org.opencontainers.image.description="${IMAGE_DESC}" \
      org.opencontainers.image.source="https://github.com/${IMAGE_VENDOR}/NovaOS/blob/${IMAGE_REF}/Containerfile" \
      org.opencontainers.image.url="https://github.com/${IMAGE_VENDOR}/NovaOS" \
      org.opencontainers.image.vendor="${IMAGE_VENDOR}" \
      org.opencontainers.image.created="${IMAGE_CREATED}" \
      io.artifacthub.package.logo-url="${IMAGE_LOGO_URL}" \
      io.artifacthub.package.keywords="${IMAGE_KEYWORDS}" \
      io.artifacthub.package.license="GPL-3.0-only" \
      io.artifacthub.package.deprecated="false" \
      containers.bootc="1"

### INIT
## Required for bootc images
CMD ["/sbin/init"]

### LINTING
## Verify final image and contents are correct. --fatal-warnings catches issues.
RUN bootc container lint --fatal-warnings
