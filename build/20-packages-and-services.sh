#!/usr/bin/env bash

set -euo pipefail

###############################################################################
# Desktop packages and services
###############################################################################
# This phase owns RPM, group and COPR installation for the selected desktop.
# The desktop is selected by the DE build arg (gnome|cinnamon|hyprland|
# mangowc|niri). Package lists live in desktops/<de>/packages.txt; Cinnamon
# is installed as a dnf group instead.
#
# Dux Shell (package `dms`) comes from the duxlabs/dms COPR on the tiling
# images (niri, hyprland, mangowc). copr_install_isolated enables the COPR
# only for the install, then disables it again, so the repo never stays live
# on the finished image.
###############################################################################

: "${DE:?DE must be set (gnome|cinnamon|hyprland|mangowc|niri)}"

# Source helper functions
# shellcheck source=/dev/null
source /ctx/build/copr-helpers.sh

shopt -s nullglob

echo "::group:: Install shared tools"

# just: the ujust-style entry point and general swiss-army knife.
# fastfetch: shows the NovaOS identity on the terminal.
dnf5 install -y just fastfetch

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
	cinnamon)
		echo "::group:: Install Cinnamon desktop group"
		# Cinnamon on Fedora 45 is a 6.7.8 development snapshot (see README).
		# Group install: ~280 packages, includes cinnamon-session, nemo,
		# cinnamon-control-center and the X session file. GDM stays the DM.
		dnf5 group install -y cinnamon-desktop
		install_package_list cinnamon
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

# Restore default glob behavior
shopt -u nullglob
