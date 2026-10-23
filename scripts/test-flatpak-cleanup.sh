#!/usr/bin/env bash
# Tests the flatpak uninstall cleanup (flatpak-host-cleanup.sh) unattended.
#
# Each case runs the real script inside a bubblewrap sandbox as your own user.
# The paths it touches (/var/lib/flatpak, /var/lib/cosmic-nightlight, /home,
# /etc/systemd/system, the helper and the polkit rule) are fake directories in
# a temporary folder, `getent passwd` reads a fake user list, and `systemctl`
# and `sleep` are stubs. Nothing on the host is read from or written to apart
# from that folder.
#
# Usage: scripts/test-flatpak-cleanup.sh [path/to/flatpak-host-cleanup.sh]
#
# Needs bwrap (the bubblewrap package).

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cleanup="$(realpath "${1:-$here/flatpak-host-cleanup.sh}")"
app_id=io.github.cosmic_nightlight

command -v bwrap >/dev/null || {
    echo "bwrap is not installed" >&2
    exit 2
}

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

failures=0
case_dir=
state=

# Sets up an empty fake host for one case.
fresh() {
    case_dir="$work/$1"
    mkdir -p "$case_dir"/{varlib/cosmic-nightlight,varlib/flatpak,home/alice,home/bob,systemd,polkit,bin,etc-flatpak/installations.d,mnt,stubs}
    state="$case_dir/varlib/cosmic-nightlight"
    cp "$cleanup" "$state/cleanup.sh"
    printf '#!/bin/sh\necho "helper $*"\n' >"$case_dir/bin/cosmic-nightlight-helper"
    chmod +x "$case_dir/bin/cosmic-nightlight-helper"
    echo "polkit rule" >"$case_dir/polkit/49-cosmic-nightlight.rules"
    {
        echo "$(sha256sum <"$case_dir/bin/cosmic-nightlight-helper" | cut -d' ' -f1)  /usr/local/bin/cosmic-nightlight-helper"
        echo "$(sha256sum <"$case_dir/polkit/49-cosmic-nightlight.rules" | cut -d' ' -f1)  /etc/polkit-1/rules.d/49-cosmic-nightlight.rules"
    } >"$state/sums"
    : >"$state/roots"
    {
        echo "root:x:0:0:root:/root:/bin/bash"
        echo "alice:x:1000:1000::/home/alice:/bin/bash"
        echo "bob:x:1001:1001::/home/bob:/bin/bash"
    } >"$case_dir/passwd"
    printf 'passwd: files\ngroup: files\n' >"$case_dir/nsswitch.conf"
    printf '#!/bin/sh\necho "systemctl $*"\n' >"$case_dir/stubs/systemctl"
    printf '#!/bin/sh\nexit 0\n' >"$case_dir/stubs/sleep"
    chmod +x "$case_dir/stubs/"*
    : >"$case_dir/log"
}

# Maps a path as the script sees it to the fake one.
fake() {
    case "$1" in
        /var/lib/flatpak*) echo "$case_dir/varlib/flatpak${1#/var/lib/flatpak}" ;;
        /home/*) echo "$case_dir/home/${1#/home/}" ;;
        /mnt/*) echo "$case_dir/mnt/${1#/mnt/}" ;;
        *) echo "unmapped path $1" >&2 && exit 2 ;;
    esac
}

user_root() { echo "/home/$1/.local/share/flatpak"; }

install_in() {
    local root
    root="$(fake "$1")"
    mkdir -p "$root/repo" "$root/app/$app_id/current/active/files"
}

uninstall_from() { rm -rf "$(fake "$1")/app/$app_id"; }

# Makes a home look like an encrypted one before login: the directory is there
# but empty.
unmount_home() {
    rm -rf "${case_dir:?}/home/$1"
    mkdir -p "$case_dir/home/$1"
}

record_roots() { printf '%s\n' "$@" >"$state/roots"; }

configure_custom() {
    printf '[Installation "drive"]\nPath=%s\n' "$1" >"$case_dir/etc-flatpak/installations.d/drive.conf"
}

run() {
    # /etc starts empty so the fake files below need no mount points on the
    # host; only the dynamic linker cache is carried over.
    bwrap --ro-bind / / --dev /dev --proc /proc --tmpfs /tmp \
        --tmpfs /etc --ro-bind-try /etc/ld.so.cache /etc/ld.so.cache \
        --bind "$case_dir/varlib" /var/lib \
        --bind "$case_dir/home" /home \
        --bind "$case_dir/mnt" /mnt \
        --bind "$case_dir/systemd" /etc/systemd/system \
        --bind "$case_dir/polkit" /etc/polkit-1/rules.d \
        --bind "$case_dir/bin" /usr/local/bin \
        --bind "$case_dir/etc-flatpak" /etc/flatpak \
        --ro-bind "$case_dir/passwd" /etc/passwd \
        --ro-bind "$case_dir/nsswitch.conf" /etc/nsswitch.conf \
        --ro-bind "$case_dir/stubs" /tmp/stubs \
        --setenv PATH "/tmp/stubs:/usr/bin:/bin" \
        bash /var/lib/cosmic-nightlight/cleanup.sh "$1" >>"$case_dir/log" 2>&1 ||
        echo "(cleanup.sh $1 exited with $?)" >>"$case_dir/log"
}

helper_kept() { [[ -e "$case_dir/bin/cosmic-nightlight-helper" && -e "$case_dir/polkit/49-cosmic-nightlight.rules" ]]; }
helper_removed() { [[ ! -e "$case_dir/bin/cosmic-nightlight-helper" && ! -e "$case_dir/polkit/49-cosmic-nightlight.rules" ]]; }

check() {
    local name="$1"
    shift
    if "$@"; then
        echo "ok    $name"
    else
        echo "FAIL  $name"
        sed 's/^/        /' "$case_dir/log"
        failures=$((failures + 1))
    fi
}

# --- Cases -------------------------------------------------------------------

fresh plain-uninstall
install_in "$(user_root alice)"
record_roots "$(user_root alice)"
uninstall_from "$(user_root alice)"
run boot
check "an uninstall removes the helper, the rule and the cleanup itself" \
    eval 'helper_removed && [[ ! -e $state ]] && grep -q "systemctl disable" "$case_dir/log"'

fresh still-installed
install_in "$(user_root alice)"
record_roots "$(user_root alice)"
run boot
check "the helper stays while the app is installed" helper_kept

fresh other-user-still-installed
install_in "$(user_root alice)"
install_in "$(user_root bob)"
record_roots "$(user_root alice)"
uninstall_from "$(user_root alice)"
run boot
check "the helper stays while another user has the app" helper_kept

fresh recorded-home-unmounted
install_in "$(user_root alice)"
install_in "$(user_root bob)"
record_roots "$(user_root alice)" "$(user_root bob)"
unmount_home bob
uninstall_from "$(user_root alice)"
run boot
check "a recorded installation in an unmounted home keeps the helper" helper_kept

fresh later-user-home-unmounted
install_in "$(user_root alice)"
record_roots "$(user_root alice)"
install_in "$(user_root bob)"
run boot
unmount_home bob
uninstall_from "$(user_root alice)"
run boot
check "an installation added after setup is remembered when its home is unmounted" helper_kept

fresh later-user-uninstalled
install_in "$(user_root alice)"
record_roots "$(user_root alice)"
install_in "$(user_root bob)"
run boot
uninstall_from "$(user_root bob)"
run boot
unmount_home bob
uninstall_from "$(user_root alice)"
run boot
check "an installation that is gone is forgotten, so it does not block cleanup" helper_removed

fresh install-sh-gui
install_in "$(user_root alice)"
record_roots "$(user_root alice)"
echo gui >"$case_dir/bin/cosmic-nightlight"
uninstall_from "$(user_root alice)"
run boot
check "an install.sh GUI keeps the helper and rule, and the cleanup removes itself" \
    eval 'helper_kept && [[ ! -e $state ]]'

fresh watch-resets-tint
install_in /var/lib/flatpak
record_roots /var/lib/flatpak
uninstall_from /var/lib/flatpak
run watch
check "a watch run resets the tint before removing the helper" \
    eval 'helper_removed && grep -q "helper --off" "$case_dir/log"'

if grep -q custom_installations "$cleanup"; then
    fresh custom-drive-unmounted
    install_in "$(user_root alice)"
    record_roots "$(user_root alice)"
    configure_custom /mnt/drive
    install_in /mnt/drive
    run boot
    rm -rf "$case_dir/mnt/drive"
    uninstall_from "$(user_root alice)"
    run boot
    check "a custom installation added after setup is remembered when its drive is gone" helper_kept

    fresh custom-still-installed
    install_in "$(user_root alice)"
    record_roots "$(user_root alice)"
    configure_custom /mnt/drive
    install_in /mnt/drive
    uninstall_from "$(user_root alice)"
    run boot
    check "the helper stays while a custom installation has the app" helper_kept
fi

echo
if ((failures)); then
    echo "$failures case(s) failed"
    exit 1
fi
echo "all cases passed"
