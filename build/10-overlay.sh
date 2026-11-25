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
#   3. GDM is enabled on every image: it lists all sessions from
#      /usr/share/wayland-sessions/*.desktop (and xsessions for Cinnamon).
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

echo "::group:: Enable display manager and base services"

# GDM is the single display manager for all five images. It is already
# present on silverblue-main; on base-main it is installed by
# 20-packages-and-services.sh. Enabling it here works either way because
# systemctl enable only writes symlinks.
systemctl enable gdm.service

echo "::endgroup::"

shopt -u nullglob

echo "Overlay phase complete!"
