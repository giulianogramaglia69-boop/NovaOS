#!/usr/bin/env bash

set -euo pipefail

###############################################################################
# NovaOS branding — applies the logo and wallpapers at build time.
###############################################################################
# Runs after 20-packages-and-services.sh (it needs gdm / gnome-shell
# installed) and before 90-cleanup.sh.
#
# What it does:
#   1. Compiles the GNOME wallpaper defaults (90-novaos.gschema.override).
#   2. Compiles the GDM login-screen logo (dconf db gdm.d).
#   3. Patches the GNOME Shell theme gresource so the login-screen
#      background is the NovaOS wallpaper (dconf cannot set the greeter
#      background since GNOME 48).
#
# Nothing here is fatal: a failed branding step warns and continues, so an
# upstream theme change can never break the whole image build. A failed
# gresource patch restores the original theme file first.
###############################################################################

warn() { echo "WARNING (branding): $*" >&2; }

echo "::group:: NovaOS branding"

###############################################################################
# 1. GNOME default wallpaper + lock screen (gschema override)
###############################################################################
if command -v glib-compile-schemas >/dev/null 2>&1; then
    glib-compile-schemas /usr/share/glib-2.0/schemas \
        || warn "glib-compile-schemas failed — GNOME keeps its default wallpaper"
else
    warn "glib-compile-schemas not found — GNOME keeps its default wallpaper"
fi

###############################################################################
# 2. GDM login-screen logo (dconf)
###############################################################################
if command -v dconf >/dev/null 2>&1 && [[ -f /etc/dconf/db/gdm.d/00-novaos ]]; then
    dconf update || warn "dconf update failed — GDM keeps its default logo"
else
    warn "dconf not found or no GDM branding file — GDM keeps its default logo"
fi

###############################################################################
# 3. GDM login-screen background (GNOME Shell theme gresource patch)
###############################################################################
patch_gdm_background() {
    local gst=/usr/share/gnome-shell/gnome-shell-theme.gresource
    local work rc=0

    [[ -f "${gst}" ]] || { warn "no gnome-shell theme gresource — GDM keeps its default background"; return 0; }
    command -v gresource >/dev/null 2>&1 || { warn "gresource not found — GDM keeps its default background"; return 0; }

    work="$(mktemp -d)" || return 0
    cp "${gst}" "${work}/orig.gresource" || { rm -rf "${work}"; return 0; }

    (
        set +e
        cd "${work}" || exit 1

        # Extract every resource, preserving the path below /org/gnome/shell.
        # Anything outside that prefix aborts the patch (unknown theme layout).
        while IFS= read -r res; do
            case "${res}" in
                /org/gnome/shell/*) ;;
                *) echo "unexpected resource prefix: ${res}" >&2; exit 1 ;;
            esac
            dest=".${res#/org/gnome/shell}"
            mkdir -p "$(dirname "${dest}")" || exit 1
            gresource extract "${gst}" "${res}" > "${dest}" || exit 1
        done < <(gresource list "${gst}")

        # GNOME 48+ ships no gnome-shell.css: the theme is split into
        # gnome-shell-dark.css / gnome-shell-light.css (+ high-contrast).
        # Patch every variant present; if none is there, skip (not fatal).
        css_files=""
        for candidate in gnome-shell.css gnome-shell-dark.css gnome-shell-light.css gnome-shell-high-contrast.css; do
            [[ -f "./theme/${candidate}" ]] && css_files="${css_files} ${candidate}"
        done
        if [[ -z "${css_files}" ]]; then
            echo "no patchable theme CSS found — GDM keeps its default background" >&2
            exit 0
        fi

        # The NovaOS wallpaper becomes the login background.
        cp /usr/share/backgrounds/novaos/novaos-dark.jpg ./theme/novaos-login.jpg || exit 1

        # A later rule with the same specificity wins, so appending is enough
        # even if the theme already has a #lockDialogGroup rule.
        for css in ${css_files}; do
            cat >> "./theme/${css}" <<'EOF'

/* NovaOS — login-screen background (added at image build time). */
#lockDialogGroup {
  background: url(novaos-login.jpg);
  background-size: cover;
  background-repeat: no-repeat;
}
EOF
        done

        # Rebuild the resource bundle from the extracted tree.
        {
            printf '%s\n' '<?xml version="1.0" encoding="UTF-8"?>'
            printf '%s\n' '<gresources>'
            printf '%s\n' '  <gresource prefix="/org/gnome/shell">'
            find . -type f ! -name '*.xml' ! -name 'orig.gresource' | sort | while IFS= read -r f; do
                printf '    <file>%s</file>\n' "${f#./}"
            done
            printf '%s\n' '  </gresource>'
            printf '%s\n' '</gresources>'
        } > gnome-shell-theme.gresource.xml

        glib-compile-resources gnome-shell-theme.gresource.xml || exit 1
        gresource list gnome-shell-theme.gresource | grep -q 'novaos-login.jpg' || exit 1
        cp gnome-shell-theme.gresource "${gst}" || exit 1
        exit 0
    # NB: the subshell MUST be in an || list: with `set -e` a bare failing
    # subshell would kill the whole script before rc=$? ever runs.
    ) || rc=$?

    if [[ "${rc}" -ne 0 ]]; then
        cp "${work}/orig.gresource" "${gst}" 2>/dev/null || true
        warn "GDM background patch failed (original theme restored)"
    else
        echo "GDM login background patched (novaos-dark.jpg)"
    fi
    rm -rf "${work}"
    return 0
}

# glib-compile-resources lives in glib2-devel: install it only for this step,
# then remove it again so the image stays clean.
HAVE_GLIB2_DEVEL=0
if command -v glib-compile-resources >/dev/null 2>&1; then
    patch_gdm_background
else
    if dnf5 install -y glib2-devel 2>/dev/null; then
        HAVE_GLIB2_DEVEL=1
        patch_gdm_background
        dnf5 remove -y glib2-devel 2>/dev/null \
            || warn "could not remove glib2-devel (harmless, stays installed)"
    else
        warn "could not install glib2-devel — GDM keeps its default background"
    fi
fi

echo "::endgroup::"
echo "Branding phase complete!"
