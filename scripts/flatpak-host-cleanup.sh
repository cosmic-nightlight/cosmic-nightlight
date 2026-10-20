#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
#
# Removes what flatpak-host-setup.sh put on the host, once the flatpak is gone.
#
# Flatpak runs no app code on uninstall, so the COSMIC Store (or
# `flatpak uninstall`) deletes the sandbox and leaves the host helper and its
# polkit rule behind. The setup therefore also installs this script, with a
# systemd path unit that runs it whenever a flatpak `app/` directory changes and
# a boot-time check as a backstop. Each run asks one question: is the app still
# installed anywhere? If not, it resets the tint, removes the helper and the
# rule, and then removes itself and its units. See "Cleaning up after an
# uninstall" in docs/flatpak-design.md.
#
# The directories it watches and scans are writable by ordinary users. They are
# only ever tested for existence here: nothing is read from them, run from them
# or passed on from them. The worst a user can do through them is fake an
# install, which keeps the helper around, which is the state the setup left
# anyway.
#
# Installed by the setup as /var/lib/cosmic-nightlight/cleanup.sh, run as root.
#
# Usage:
#   cleanup.sh watch   # from the path unit: settle, check, clean up
#   cleanup.sh boot    # from the boot check: same, but leave the screen alone
#   cleanup.sh roots   # print the flatpak installations that have the app
#   cleanup.sh paths   # print the directories the path unit should watch

set -euo pipefail

app_id=io.github.cosmic_nightlight

helper=/usr/local/bin/cosmic-nightlight-helper
rule=/etc/polkit-1/rules.d/49-cosmic-nightlight.rules
state_dir=/var/lib/cosmic-nightlight
sums="$state_dir/sums"
roots_file="$state_dir/roots"
unit_dir=/etc/systemd/system
path_unit=cosmic-nightlight-cleanup.path
service_template=cosmic-nightlight-cleanup@.service
boot_unit=cosmic-nightlight-cleanup@boot.service

# Every flatpak installation the app could be in: the system one, and each
# user's. Custom installations under /etc/flatpak/installations.d are not
# covered; an app there is simply not seen, and its helper is kept only until
# the next check that finds no other install.
installations() {
    echo /var/lib/flatpak
    getent passwd | while IFS=: read -r _ _ _ _ _ home _; do
        [[ "$home" == /?* && -d "$home" ]] || continue
        echo "$home/.local/share/flatpak"
    done
}

# Whether the app is deployed in one installation. `current/active` is a
# symlink flatpak swaps atomically on update, so it is there for as long as the
# app is installed and goes away when it is uninstalled.
has_app() {
    [[ -e "$1/app/$app_id/current/active/files" ]]
}

# An installation we can actually see into. A flatpak installation always has
# `repo/`; a home that is not mounted yet (an encrypted home, or one on a drive
# that is not up) has nothing, and must not read as "uninstalled".
visible() {
    [[ -d "$1/repo" ]]
}

installed_anywhere() {
    local root
    while read -r root; do
        has_app "$root" && return 0
    done < <(installations)
    return 1
}

# Every installation that had the app when the setup ran must be visible now.
# If one is not, we cannot tell whether it still has the app, so do nothing.
recorded_roots_visible() {
    local root
    [[ -f "$roots_file" ]] || return 0
    while read -r root; do
        [[ -n "$root" ]] || continue
        visible "$root" || return 1
    done <"$roots_file"
}

# Whether a file is still the one the setup installed. install.sh writes the
# same two paths, so anything that has changed since belongs to someone else.
ours() {
    [[ -f "$1" && -f "$sums" ]] || return 1
    grep -qxF "$(sha256sum "$1")" "$sums"
}

remove_self() {
    systemctl disable --no-block "$path_unit" "$boot_unit" 2>/dev/null || true
    systemctl stop --no-block "$path_unit" 2>/dev/null || true
    rm -f "$unit_dir/$path_unit" "$unit_dir/$service_template"
    systemctl daemon-reload 2>/dev/null || true
    # Deleting the running script is safe: bash keeps reading from the open file.
    rm -rf "$state_dir"
}

clean_up() {
    local mode="$1"

    if [[ "$mode" == watch ]]; then
        # Let a flatpak operation finish before judging it. An uninstall that is
        # followed straight away by a reinstall (a branch or remote switch)
        # should not cost the user the helper.
        sleep 5
    fi

    installed_anywhere && return 0
    recorded_roots_visible || return 0

    # Someone else now uses the helper and the rule, so leave both: a GUI from
    # install.sh, or a helper install.sh has replaced since. The rule matches
    # either way, since install.sh writes the same one.
    if [[ -e /usr/local/bin/cosmic-nightlight ]] ||
        { [[ -e "$helper" ]] && ! ours "$helper"; }; then
        remove_self
        return 0
    fi

    # Reset the tint while the helper is still here, so nobody is left with a
    # warm screen and no app to undo it. Not at boot, though: there is no tint
    # to clear yet, and the VT switch would get in the way of the login screen.
    if [[ "$mode" == watch ]] && ours "$helper"; then
        "$helper" --off || true
    fi

    # The helper goes first. A leftover applet process that calls it in between
    # then fails outright, rather than prompting for a password once the rule
    # has gone.
    if ours "$helper"; then
        rm -f "$helper"
        echo "Removed: $helper"
    fi
    if ours "$rule"; then
        rm -f "$rule"
        echo "Removed: $rule"
    fi

    remove_self
}

case "${1:-}" in
    watch | boot)
        clean_up "$1"
        ;;
    roots)
        while read -r root; do
            if has_app "$root"; then
                echo "$root"
            fi
        done < <(installations)
        ;;
    paths)
        # Only real login accounts. systemd watches the nearest existing parent
        # of a path that is not there yet, so a user who installs flatpaks later
        # is still covered. Paths systemd would misread are skipped; the boot
        # check still covers those users.
        echo /var/lib/flatpak/app
        getent passwd | while IFS=: read -r _ _ uid _ _ home _; do
            ((uid >= 1000 && uid != 65534)) || continue
            [[ "$home" == /?* && -d "$home" ]] || continue
            [[ "$home" =~ [[:space:]%\\] ]] && continue
            echo "$home/.local/share/flatpak/app"
        done
        ;;
    *)
        echo "usage: $0 watch|boot|roots|paths" >&2
        exit 2
        ;;
esac
