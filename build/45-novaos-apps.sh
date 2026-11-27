#!/usr/bin/env bash

set -euo pipefail

###############################################################################
# NovaOS system apps — build novaos-setup, novaos-updates and
# novaos-snapshots from source and install them.
###############################################################################
# Builds novaos-apps/ (in the build context) with cargo and installs
# /usr/bin/novaos-setup, /usr/bin/novaos-updates and
# /usr/bin/novaos-snapshots. The .desktop entries ship via
# system_files/usr/share/applications/ (copied by 10-overlay.sh);
# novaos-setup autostarts once at first login via
# system_files/etc/xdg/autostart/ + the compositor configs.
#
# Runtime libs (gtk4, libadwaita) stay installed; the toolchain and
# -devel packages are removed again so the image stays clean.
#
# The apps never enable third-party repos at build time (90-cleanup.sh
# fails the build if any is left enabled): RPM Fusion and friends are
# enabled by novaos-setup at runtime, on the installed system, only if
# the user asks for them there.
###############################################################################

NOVAOS_APPS_SRC="/ctx/novaos-apps"
BUILD_DIR="/var/tmp/novaos-apps-build"

echo "::group:: NovaOS apps — build dependencies"

dnf5 install -y rust cargo gtk4-devel libadwaita-devel

echo "::endgroup::"
echo "::group:: NovaOS apps — runtime libraries"

dnf5 install -y gtk4 libadwaita

echo "::endgroup::"

echo "::group:: NovaOS apps — cargo build"

# The build context is mounted read-only; copy the source to a writable dir.
mkdir -p "${BUILD_DIR}"
cp -a "${NOVAOS_APPS_SRC}" "${BUILD_DIR}/src"
export CARGO_HOME="${BUILD_DIR}/cargo-home"
export CARGO_TARGET_DIR="${BUILD_DIR}/target"
cargo build --release --locked --manifest-path "${BUILD_DIR}/src/Cargo.toml"
install -m 0755 "${BUILD_DIR}/target/release/novaos-setup" /usr/bin/novaos-setup
install -m 0755 "${BUILD_DIR}/target/release/novaos-updates" /usr/bin/novaos-updates
install -m 0755 "${BUILD_DIR}/target/release/novaos-snapshots" /usr/bin/novaos-snapshots

echo "::endgroup::"

# Clean up: toolchain + headers go, runtime libs stay.
dnf5 remove -y rust cargo gtk4-devel libadwaita-devel
rm -rf "${BUILD_DIR}"

for bin in novaos-setup novaos-updates novaos-snapshots; do
	test -x "/usr/bin/${bin}" && echo "${bin} installed in /usr/bin/${bin}"
done
