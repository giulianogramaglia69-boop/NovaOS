#!/usr/bin/env bash

set -euo pipefail

###############################################################################
# Desktop packages and services
###############################################################################
# This phase owns RPM, group and COPR installation for the selected desktop.
# The desktop is selected by the DE build arg (gnome|cosmic|hyprland|
# mangowc|niri). Package lists live in desktops/<de>/packages.txt; COSMIC
# is installed as a dnf group instead.
#
# Dux Shell (package `dms`) comes from the duxlabs/dms COPR on the tiling
# images (niri, hyprland, mangowc). copr_install_isolated enables the COPR
# only for the install, then disables it again, so the repo never stays live
# on the finished image.
###############################################################################

: "${DE:?DE must be set (gnome|cosmic|hyprland|mangowc|niri)}"

# Source helper functions
# shellcheck source=/dev/null
source /ctx/build/copr-helpers.sh

shopt -s nullglob

echo "::group:: Install shared tools"

# just: the ujust-style entry point and general swiss-army knife.
# fastfetch: shows the NovaOS identity on the terminal.
# brightnessctl: screen brightness for NovaAI (set_brightness).
# pipewire-utils: pw-record for NovaAI voice input (wpctl is wireplumber,
# already on every image via the desktop stack).
# libsecret: secret-tool, the API-key store for NovaAI.
# distrobox: containers for other distros, driven by NovaAI + BoxBuddy.
# grim: screenshots for NovaAI (wlroots compositors).
# flatpak: app installs for NovaAI + BoxBuddy (also in the Atomic base,
# listed here so the image never depends on that assumption).
# power-profiles-daemon: powerprofilesctl for NovaAI power profiles.
# firefox: headless browser for the NovaAI `browser` tool (WebDriver).
# bubblewrap: sandbox for NovaAI read-only shell commands (bwrap).
# upower: battery status for the NovaAI watchers (`nova-ai --check`).
# libnotify: notify-send, the desktop notifications of the watchers.
# lxpolkit: PolicyKit authentication agent, so pkexec password prompts
# work on every DE (GNOME has its own inside gnome-shell; COSMIC has
# none of its own — pop-os/cosmic-epoch #2116; the tiling images
# already list lxpolkit in their packages.txt, here it covers
# gnome/cosmic too). It is autostarted everywhere: XDG autostart
# (GNOME/COSMIC) + spawn/exec_once on the tiling compositors.
dnf5 install -y just fastfetch brightnessctl pipewire-utils libsecret distrobox grim flatpak power-profiles-daemon firefox bubblewrap upower libnotify lxpolkit

echo "::endgroup::"

install_package_list() {
	local de="$1"
	local list="/ctx/desktops/${de}/packages.txt"
	[[ -f "${list}" ]] || { echo "ERROR: package list not found: ${list}" >&2; exit 1; }
	# Strip comments and blank lines; one package per line.
	mapfile -t pkgs < <(grep -vE '^\s*(#|$)' "${list}")
	if [[ ${#pkgs[@]} -gt 0 ]]; then
		echo "Installing from ${list}: ${pkgs[*]}"
		dnf5 install -y "${pkgs[@]}"
	fi
}

case "${DE}" in
	gnome)
		echo "::group:: GNOME (already in silverblue-main)"
		# Nothing to install: the base image ships GNOME 51. Extra GNOME
		# apps can be added to desktops/gnome/packages.txt later.
		install_package_list gnome
		echo "::endgroup::"
		;;
	cosmic)
		echo "::group:: Install COSMIC desktop"
		# COSMIC 1.10.0 is in the official Fedora 45 repos (verified
		# 25 Nov 2026 against the F45 repodata: 29 packages, no COPR
		# needed). Group name is `cosmic-desktop` — the
		# `cosmic-desktop-environment` name quoted by many guides
		# does not exist on F45. Group install pulls cosmic-session
		# and the core desktop (comp, panel, launcher, settings,
		# greeter, portal); session file is
		# /usr/share/wayland-sessions/cosmic.desktop. Wayland-only;
		# XWayland comes in via cosmic-comp.
		dnf5 group install -y cosmic-desktop
		install_package_list cosmic
		echo "::endgroup::"
		;;
	hyprland)
		echo "::group:: Install Hyprland + Dux Shell"
		install_package_list hyprland
		# Hyprland is NOT in Fedora 45 (verified 25 Nov 2026 against the
		# F45 release + updates repodata). The sdegler/hyprland COPR ships
		# Hyprland 0.57.0 for F45 with its companions. (The well-known
		# solopasha/hyprland COPR has no F45 builds anymore.)
		copr_install_isolated "sdegler/hyprland" hyprland hyprlock hypridle hyprpaper xdg-desktop-portal-hyprland
		# Dux Shell + companions from the duxlabs/dms COPR.
		# (quickshell/matugen resolve from the COPR or Fedora; the
		# --enablerepo flag only adds the COPR, base repos stay enabled.)
		copr_install_isolated "duxlabs/dms" dms quickshell matugen
		echo "::endgroup::"
		;;
	niri)
		echo "::group:: Install Niri + Dux Shell"
		install_package_list niri
		# Dux Shell from the duxlabs/dms COPR.
		copr_install_isolated "duxlabs/dms" dms quickshell matugen
		echo "::endgroup::"
		;;
	mangowc)
		echo "::group:: Install MangoWC + Dux Shell"
		# MangoWC is not in the Fedora repos. The source is selected by the
		# MANGOWC_SOURCE build arg (terra | copr | source); see
		# desktops/mangowc/README.md for how the source was verified.
		case "${MANGOWC_SOURCE:-terra}" in
			terra)
				# Terra (Fyra Labs) third-party repo. The package name was
				# verified against the terra45 repodata: ${MANGOWC_PKG}.
				: "${MANGOWC_PKG:?MANGOWC_PKG must be set for the terra source}"
				dnf5 install -y --nogpgcheck \
					--repofrompath "terra,https://repos.fyralabs.com/terra${FEDORA_MAJOR_VERSION:-45}" \
					terra-release
				dnf5 install -y "${MANGOWC_PKG}"
				;;
			copr)
				copr_install_isolated "dennemann/MangoWC" mangowc
				;;
			source)
				echo "ERROR: source build of MangoWC is not wired into this script yet" >&2
				echo "See desktops/mangowc/README.md" >&2
				exit 1
				;;
			*)
				echo "ERROR: unknown MANGOWC_SOURCE: ${MANGOWC_SOURCE}" >&2
				exit 1
				;;
		esac
		install_package_list mangowc
		# Dux Shell from the duxlabs/dms COPR.
		copr_install_isolated "duxlabs/dms" dms quickshell matugen
		# MangoWC's default /etc/mango/config.conf has no autostart lines
		# (verified 26 Nov 2026 against the Terra mangowm 0.19.2 RPM):
		# nothing starts Dux Shell, the wallpaper, NovaAI, the polkit
		# agent or the first-run setup on this DE. Append exec_once
		# lines after the package install (the file is owned by the
		# package, so overlaying it from system_files would be clobbered
		# by the install above). Guarded: the COPR fallback may ship a
		# different config path, and the step must not fail the whole
		# build if the file is absent.
		if [ -f /etc/mango/config.conf ]; then
			if ! grep -q '^exec_once=nova-ai$' /etc/mango/config.conf; then
				{
					echo ""
					echo "# NovaOS: Dux Shell, wallpaper, NovaAI, polkit agent and first-run setup autostart."
					echo "exec_once=dms run"
					echo "exec_once=swaybg -m fill -i /usr/share/backgrounds/novaos/novaos-dark.jpg"
					echo "exec_once=nova-ai"
					echo "exec_once=lxpolkit"
					echo "exec_once=novaos-setup --first-run"
				} >> /etc/mango/config.conf
			fi
		else
			echo "WARNING: /etc/mango/config.conf not found; Dux Shell/NovaAI autostart not wired on MangoWC" >&2
		fi
		echo "::endgroup::"
		;;
	*)
		echo "ERROR: unknown DE: ${DE}" >&2
		exit 1
		;;
esac

# GDM is required on base-main images (silverblue-main already ships it).
if ! rpm -q gdm >/dev/null 2>&1; then
	echo "::group:: Install GDM (base-main images)"
	dnf5 install -y gdm
	echo "::endgroup::"
fi

# GDM is the single display manager for all five images. It is enabled
# here, not in 10-overlay.sh: that phase runs before GDM exists on
# base-main, so enabling it there failed with "Unit gdm.service does not
# exist" (CI run 45642597872). At this point GDM is installed on every
# image — but the enable can still fail: a desktop can ship its own
# display manager whose %post preset claims the display-manager.service
# alias first (Cinnamon pulled LightDM, COSMIC pulls cosmic-greeter), and
# `systemctl enable gdm.service` then refuses to overwrite the alias
# (that killed the cinnamon job of CI run 45667619037). So disable any
# other display manager and drop the alias symlink first; GDM then
# owns the alias on every image.
echo "::group:: Enable display manager"
for other_dm in lightdm cosmic-greeter; do
	systemctl disable "${other_dm}.service" 2>/dev/null || true
done
rm -f /etc/systemd/system/display-manager.service
systemctl enable gdm.service
echo "::endgroup::"

# Restore default glob behavior
shopt -u nullglob
