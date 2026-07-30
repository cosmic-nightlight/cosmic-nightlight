#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
#
# Uninstalls cosmic-nightlight components installed by install.sh.
#
# Beyond deleting files, this tears down everything that can start the daemon on
# the next login (systemd user service + XDG autostart entry) and resets the
# screen tint while the helper is still present — otherwise a leftover autostart
# keeps re-applying the warm tint after every login even though the app is gone.
#
# Usage:
#   ./scripts/uninstall.sh

set -euo pipefail

echo ">> Uninstalling cosmic-nightlight components (requires sudo)..."

# 1. Stop and disable the background daemon FIRST, so it can't re-apply the tint
#    while we tear down or on the next login. Cover the per-user and the package's
#    system-wide (`--global`) enablement, plus the pre-rename "nightshift" name.
for unit in cosmic-nightlight.service cosmic-nightshift.service; do
    systemctl --user disable --now "$unit" 2>/dev/null || true
    sudo systemctl --global disable "$unit" 2>/dev/null || true
    # `disable` is a no-op once the unit file itself is gone (an already-removed
    # package, or the pre-rename name), which leaves the enablement symlink behind
    # pointing at nothing. Clear it by hand so a reinstall doesn't inherit it.
    sudo rm -f "/etc/systemd/user/graphical-session.target.wants/$unit"
done

# 2. Remove the XDG autostart entry that the old in-app "Start on login" toggle
#    wrote (current and pre-rename names), from the invoking user's config. The
#    toggle is gone — every run mode now keeps to the schedule on its own — but
#    versions that had it left an entry behind that still starts a daemon.
config_home="${XDG_CONFIG_HOME:-$HOME/.config}"
for app_id in io.github.cosmic_nightlight io.github.cosmic_nightshift; do
    entry="$config_home/autostart/$app_id.desktop"
    if [[ -f "$entry" ]]; then
        rm -f "$entry"
        echo "Removed: $entry"
    fi
done

# 3. Reset the screen to a neutral ramp while the helper is still installed, so
#    the user isn't left staring at a warm screen after uninstall. Done after the
#    daemon is stopped (step 1) so nothing re-tints right behind us.
for helper in /usr/local/bin/cosmic-nightlight-helper /usr/bin/cosmic-nightlight-helper; do
    if [[ -x "$helper" ]]; then
        echo ">> Resetting screen tint via $helper --off"
        sudo "$helper" --off || true
        break
    fi
done

# 4. Remove the files install.sh placed under /usr/local and /usr/share.
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
echo "  sudo apt remove cosmic-nightlight"
echo "Your settings remain in $config_home/cosmic/io.github.cosmic_nightlight (delete to fully reset)."
echo "If the polkit rule is still active, you may need to: sudo systemctl restart polkit"
