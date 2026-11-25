#!/usr/bin/env bash

set -euo pipefail

###############################################################################
# NovaAI — build the system AI agent from source and install it.
###############################################################################
# Builds nova-ai/ (in the build context) with cargo and installs
# /usr/bin/nova-ai. The autostart entry ships via
# system_files/etc/xdg/autostart/nova-ai.desktop (copied by 10-overlay.sh).
#
# Runtime libs (gtk4, libadwaita, gtk4-layer-shell, libsecret, alsa-lib)
# stay installed; the toolchain and -devel packages are removed again so
# the image stays clean. libvosk (wake word) comes from the official
# upstream prebuilt zip — Fedora does not package it. If that download
# fails, NovaAI is still built without the wake-word feature
# (--no-default-features); the settings screen says so.
###############################################################################

NOVA_AI_SRC="/ctx/nova-ai"
BUILD_DIR="/var/tmp/nova-ai-build"
VOSK_VERSION="0.3.45"
VOSK_URL="https://github.com/alphacep/vosk-api/releases/download/v${VOSK_VERSION}/vosk-linux-x86_64-${VOSK_VERSION}.zip"

echo "::group:: NovaAI — build dependencies"

dnf5 install -y rust cargo gtk4-devel libadwaita-devel gtk4-layer-shell-devel libsecret-devel alsa-lib-devel

echo "::endgroup::"
echo "::group:: NovaAI — runtime libraries"

dnf5 install -y gtk4 libadwaita gtk4-layer-shell libsecret alsa-lib

echo "::endgroup::"

# libvosk for the wake-word feature ("Ehi Nova").
mkdir -p "${BUILD_DIR}"
if curl -fsSL -o "${BUILD_DIR}/vosk.zip" "${VOSK_URL}" \
    && python3 -c "import zipfile; zipfile.ZipFile('${BUILD_DIR}/vosk.zip').extractall('${BUILD_DIR}')" \
    && [[ -f "${BUILD_DIR}/vosk-linux-x86_64-${VOSK_VERSION}/libvosk.so" ]]; then
    cp "${BUILD_DIR}/vosk-linux-x86_64-${VOSK_VERSION}/libvosk.so" /usr/lib64/
    ldconfig
    echo "libvosk ${VOSK_VERSION} installed — wake word enabled"
    WAKEWORD_OFF=0
else
    echo "WARNING (nova-ai): could not fetch libvosk — building NovaAI WITHOUT the wake-word feature" >&2
    WAKEWORD_OFF=1
fi

echo "::group:: NovaAI — cargo build"

# The build context is mounted read-only; copy the source to a writable dir.
cp -a "${NOVA_AI_SRC}" "${BUILD_DIR}/src"
export CARGO_HOME="${BUILD_DIR}/cargo-home"
export CARGO_TARGET_DIR="${BUILD_DIR}/target"
if [[ "${WAKEWORD_OFF}" -eq 1 ]]; then
    RUSTFLAGS="-L /usr/lib64" cargo build --release --locked \
        --manifest-path "${BUILD_DIR}/src/Cargo.toml" --no-default-features
else
    RUSTFLAGS="-L /usr/lib64" cargo build --release --locked \
        --manifest-path "${BUILD_DIR}/src/Cargo.toml"
fi
install -m 0755 "${BUILD_DIR}/target/release/nova-ai" /usr/bin/nova-ai

echo "::endgroup::"

# Clean up: toolchain + headers go, runtime libs (and libvosk) stay.
dnf5 remove -y rust cargo gtk4-devel libadwaita-devel gtk4-layer-shell-devel libsecret-devel alsa-lib-devel
rm -rf "${BUILD_DIR}"

test -x /usr/bin/nova-ai && echo "NovaAI installed in /usr/bin/nova-ai"
