#!/usr/bin/env bash

set -euo pipefail

###############################################################################
# Runtime overlays
###############################################################################
# This phase owns every file NovaOS lays on top of its base image. It installs
# nothing; RPM/COPR installation belongs to 20-packages-and-services.sh.
#
#   1. system_files/ mirrors the image root: session files, GDM defaults,
#      and anything else shipped by path.
#   2. system_files/etc/skel/.config seeds each new user's ~/.config
#      (niri / Hyprland starter configs that launch Dux Shell).
#   3. GDM is NOT enabled here: on base-main images GDM is only installed
#      later, by 20-packages-and-services.sh, so `systemctl enable
#      gdm.service` here fails with "Unit gdm.service does not exist"
#      (that killed the cinnamon/hyprland/mangowc/niri jobs of CI run
#      45642597872). The enable step lives at the end of
#      20-packages-and-services.sh, after the GDM install.
#      GDM lists all sessions from /usr/share/wayland-sessions/*.desktop.
###############################################################################

shopt -s nullglob

echo "::group:: Overlay NovaOS system files"

# rsync is present in the Fedora base images; fall back to cp if not.
if command -v rsync >/dev/null 2>&1; then
	rsync -rvK /ctx/system_files/ /
else
	cp -rv /ctx/system_files/. /
fi

echo "::endgroup::"

shopt -u nullglob

echo "Overlay phase complete!"
