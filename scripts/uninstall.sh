#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
#
# Uninstalls cosmic-nightlight, however it was installed — install.sh, the Debian
# package, or the flatpak.
#
# Beyond deleting files, this tears down everything that can start the applet
# again (the COSMIC panel's applet registration, the systemd user service, and
# the XDG autostart entry) and resets the screen tint while a helper is still
# present — otherwise a leftover launcher keeps re-applying the warm tint after
# every login even though the app is gone.
#
# Usage:
#   ./scripts/uninstall.sh

set -euo pipefail

echo ">> Uninstalling cosmic-nightlight components (requires sudo)..."

config_home="${XDG_CONFIG_HOME:-$HOME/.config}"

# Every app id this has shipped under, current first. The teardown steps below
# all loop over both, so upgrading past the rename and then uninstalling doesn't
# strand the pre-rename "nightshift" install.
app_ids=(io.github.cosmic_nightlight io.github.cosmic_nightshift)

# 1. Un-register the panel applet FIRST. cosmic-panel restarts anything listed in
#    its plugin config as soon as the process dies, so killing or uninstalling
#    while the entry is still there only gets the applet respawned — and once the
#    files are gone the panel is left retrying a launch that can never succeed,
#    leaving a dead gap in the panel. Nothing else here removes this entry: it is
#    added by hand through COSMIC Settings, so no install path owns it.
strip_applet() {
    local file="$1" app_id="$2"
    # Escape the dots so they match literally rather than as any-character.
    local pattern="^[[:space:]]*\"${app_id//./\\.}\",?[[:space:]]*$"

    grep -qF "\"$app_id\"" "$file" || return 0
    # cosmic-panel writes one entry per line. If this one shares a line with
    # another applet, deleting the line would take that applet out too, so leave
    # it alone and say what needs doing.
    if grep -qE "$pattern" "$file"; then
        sed -i -E "/$pattern/d" "$file"
        echo "Removed applet entry from: $file"
    else
        echo "WARNING: $app_id shares a line in $file; remove it by hand." >&2
    fi
}

# The applet can be dropped into either container (Panel or Dock) and either of
# each one's lists (the wings or the center), so check all four.
for app_id in "${app_ids[@]}"; do
    for list in "$config_home"/cosmic/com.system76.CosmicPanel.*/v1/plugins_{wings,center}; do
        [[ -f "$list" ]] || continue
        strip_applet "$list" "$app_id"
    done
done

# 2. Stop whatever is already running, so it can't re-apply the tint while we
#    tear down or on the next login. Cover the per-user and the package's
#    system-wide (`--global`) enablement.
for unit in cosmic-nightlight.service cosmic-nightshift.service; do
    systemctl --user disable --now "$unit" 2>/dev/null || true
    sudo systemctl --global disable "$unit" 2>/dev/null || true
    # `disable` is a no-op once the unit file itself is gone (an already-removed
    # package, or the pre-rename name), which leaves the enablement symlink behind
    # pointing at nothing. Clear it by hand so a reinstall doesn't inherit it.
    sudo rm -f "/etc/systemd/user/graphical-session.target.wants/$unit"
done

# A flatpak instance isn't under systemd — it's a sandbox, so ask flatpak.
if command -v flatpak >/dev/null 2>&1; then
    for app_id in "${app_ids[@]}"; do
        flatpak kill "$app_id" 2>/dev/null || true
    done
fi

# 3. Remove the XDG autostart entry that the old in-app "Start on login" toggle
#    wrote (current and pre-rename names), from the invoking user's config. The
#    toggle is gone — every run mode now keeps to the schedule on its own — but
#    versions that had it left an entry behind that still starts a daemon.
for app_id in "${app_ids[@]}"; do
    entry="$config_home/autostart/$app_id.desktop"
    if [[ -f "$entry" ]]; then
        rm -f "$entry"
        echo "Removed: $entry"
    fi
done

# 4. Reset the screen to a neutral ramp while a helper is still around, so the
#    user isn't left staring at a warm screen. This has to land after the
#    launchers are gone (steps 1-2) so nothing re-tints right behind us, and
#    before anything is deleted (steps 5-6) so there is still a helper to run.
#
#    Look inside the flatpak as well as on the host: the host helper is already
#    gone whenever the Debian package was removed first, which used to make this
#    step skip silently and strand the screen warm with nothing left to fix it.
#    The flatpak ships the same binary, and `current/active` resolves it without
#    having to know the commit hash.
helpers=(/usr/local/bin/cosmic-nightlight-helper /usr/bin/cosmic-nightlight-helper)
for app_id in "${app_ids[@]}"; do
    for flatpak_root in "$HOME/.local/share/flatpak" /var/lib/flatpak; do
        helpers+=("$flatpak_root/app/$app_id/current/active/files/libexec/cosmic-nightlight-helper")
    done
done

tint_reset=0
for helper in "${helpers[@]}"; do
    if [[ -x "$helper" ]]; then
        echo ">> Resetting screen tint via $helper --off"
        sudo "$helper" --off || true
        tint_reset=1
        break
    fi
done
if [[ "$tint_reset" -eq 0 ]]; then
    echo "WARNING: no helper found to reset the tint. If the screen is still warm," >&2
    echo "         log out and back in — the compositor restores a neutral ramp." >&2
fi

# 5. Remove the files install.sh placed under /usr/local and /usr/share.
remove() {
    if [[ -e "$1" ]]; then
        sudo rm -f "$1"
        echo "Removed: $1"
    fi
}
remove /usr/local/bin/cosmic-nightlight-helper
remove /usr/local/bin/cosmic-nightlight
remove /etc/polkit-1/rules.d/49-cosmic-nightlight.rules
remove /usr/share/applications/io.github.cosmic_nightlight.desktop
remove /usr/share/applications/io.github.cosmic_nightlight.settings.desktop
remove /usr/share/metainfo/io.github.cosmic_nightlight.metainfo.xml
remove /usr/share/icons/hicolor/scalable/apps/io.github.cosmic_nightlight.svg
remove /usr/share/icons/hicolor/128x128/apps/io.github.cosmic_nightlight.png
# A user service some setups copy in by hand (see systemd/cosmic-nightlight.service).
remove "$config_home/systemd/user/cosmic-nightlight.service"

# 6. Remove the flatpak, which is an install wholly separate from everything
#    above — it puts nothing under /usr/local and dpkg knows nothing about it, so
#    neither the file removal above nor `apt remove` touches it. A flatpak
#    install used to survive this script completely untouched.
if command -v flatpak >/dev/null 2>&1; then
    for app_id in "${app_ids[@]}"; do
        if flatpak info --user "$app_id" >/dev/null 2>&1; then
            echo ">> Removing user flatpak: $app_id"
            flatpak uninstall --user --delete-data -y "$app_id" || true
        fi
        if flatpak info --system "$app_id" >/dev/null 2>&1; then
            echo ">> Removing system flatpak: $app_id"
            sudo flatpak uninstall --system --delete-data -y "$app_id" || true
        fi
    done
fi

# Update desktop database if it exists
if command -v update-desktop-database >/dev/null 2>&1; then
    sudo update-desktop-database /usr/share/applications 2>/dev/null || true
fi

# Drop the removed icons from the theme cache.
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    sudo gtk-update-icon-cache -f -t /usr/share/icons/hicolor 2>/dev/null || true
fi

echo
echo "Uninstallation complete."
echo "If you installed the Debian package, also remove it with:"
echo "  sudo apt purge cosmic-nightlight"
echo "Purge rather than remove: 'remove' leaves the package in dpkg's 'rc' state,"
echo "which keeps its config files and still lists it in 'dpkg -l'."
echo "Your settings remain in $config_home/cosmic/io.github.cosmic_nightlight (delete to fully reset)."
echo "If the polkit rule is still active, you may need to: sudo systemctl restart polkit"
