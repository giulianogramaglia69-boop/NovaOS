#!/usr/bin/env bash

set -euo pipefail

###############################################################################
# Preinstalled Flatpaks (system scope)
###############################################################################
# Installs BoxBuddy (graphical UI for Distrobox) from Flathub so NovaAI's
# distrobox tool has a GUI companion out of the box. Distrobox itself is
# installed as an RPM in 20-packages-and-services.sh — BoxBuddy does NOT
# bundle it, it only drives the system binary.
#
# BoxBuddy app ID verified 2026-11-26 on the Flathub app page
# (https://flathub.org/apps/io.github.dvlv.boxbuddyrs): "BoxBuddy is a
# graphical user interface for Distrobox".
# The Flathub remote is already shipped in
# system_files/usr/share/flatpak/remotes.d/flathub.flatpakrepo; the
# remote-add below is a belt-and-braces fallback (--if-not-exists).
#
# Fatal on failure: a half-installed Flatpak is worse than a failed
# build — the error names the app so the CI log says what broke.
###############################################################################

echo "::group:: Preinstall Flatpaks (BoxBuddy)"

flatpak remote-add --if-not-exists --system flathub https://dl.flathub.org/repo/flathub.flatpakrepo

if ! flatpak install --system -y flathub io.github.dvlv.boxbuddyrs; then
	echo "ERROR: BoxBuddy install from Flathub failed (app id io.github.dvlv.boxbuddyrs)" >&2
	exit 1
fi

echo "BoxBuddy installed:"
flatpak list --system --app --columns=application,version | grep -F io.github.dvlv.boxbuddyrs || {
	echo "ERROR: BoxBuddy not found after install" >&2
	exit 1
}

echo "::endgroup::"
