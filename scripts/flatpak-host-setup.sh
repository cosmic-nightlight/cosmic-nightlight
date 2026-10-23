#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
#
# Puts the privileged helper on the host, for the flatpak build.
#
# A flatpak cannot set the screen tint itself: writing gamma LUTs needs DRM
# master, which needs CAP_SYS_ADMIN, which no sandbox permission grants. The
# applet therefore calls out with `flatpak-spawn --host pkexec`, and the helper
# has to already be on the host for that to reach anything.
#
# This script is what puts it there. It ships inside the flatpak and runs through
# pkexec, so it costs one password prompt, after which the polkit rule makes every
# schedule transition password-less.
#
# Installs:
#   the helper    -> /usr/local/bin/cosmic-nightlight-helper
#   the rule      -> /etc/polkit-1/rules.d/49-cosmic-nightlight.rules
#   the cleanup   -> /var/lib/cosmic-nightlight/ and two systemd units
#
# The helper and rule paths are already whitelisted by the rule itself, so
# nothing there differs from what the .deb and install.sh set up. The cleanup is
# flatpak-only: it removes all of the above once the app is uninstalled, since
# flatpak gives an app no way to do that itself. See flatpak-host-cleanup.sh.
#
# Any arguments are forwarded verbatim to the helper once it is installed, which
# is what lets the setup ride along on a change the user was making anyway. Before
# the setup has run, every tint change already costs a password prompt; spending
# that same prompt here buys the change *and* permanent silence, instead of just
# the change. The GUI therefore routes its first privileged call through this
# script rather than straight at the helper. See `privileged_program` in
# crates/cosmic-nightlight/src/backend.rs.
#
# Usage (from inside the sandbox, where $app is /.flatpak-info's app-path):
#   flatpak-spawn --host pkexec "$app/libexec/cosmic-nightlight-setup"
#   flatpak-spawn --host pkexec "$app/libexec/cosmic-nightlight-setup" \
#       --temp 3500 --brightness 0.8 --session-vt 1

set -euo pipefail

if [[ "$(id -u)" -ne 0 ]]; then
    echo "cosmic-nightlight-setup: must run as root; invoke it through pkexec." >&2
    exit 1
fi

# The flatpak's own files, as seen from the host. Resolved from this script's
# location rather than hardcoded, because it differs between a user install
# (~/.local/share/flatpak) and a system one (/var/lib/flatpak).
libexec_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
app_dir="$(dirname "$libexec_dir")"

helper="$libexec_dir/cosmic-nightlight-helper"
rule="$app_dir/share/polkit-1/rules.d/49-cosmic-nightlight.rules"
cleanup="$libexec_dir/cosmic-nightlight-cleanup"

for f in "$helper" "$rule"; do
    [[ -f "$f" ]] || { echo "cosmic-nightlight-setup: missing $f" >&2; exit 1; }
done

# Own both as root. The rule grants password-less root to whatever sits at the
# helper path, so that path must not be writable by the user it is granting to —
# copying out of the flatpak (which is user-writable on a user install) is the
# whole point of this step.
#
# Best-effort, and deliberately not fatal: if /usr/local or /etc cannot be written
# (a read-only or otherwise locked-down host), a forwarded tint change should
# still happen. The user is no worse off than before — they keep being prompted —
# whereas failing here would take the screen change down with the install.
installed=0
if install -o root -g root -m 0755 "$helper" /usr/local/bin/cosmic-nightlight-helper &&
    install -D -o root -g root -m 0644 "$rule" \
        /etc/polkit-1/rules.d/49-cosmic-nightlight.rules; then
    installed=1
    echo "Installed:"
    echo "  /usr/local/bin/cosmic-nightlight-helper"
    echo "  /etc/polkit-1/rules.d/49-cosmic-nightlight.rules"
else
    echo "cosmic-nightlight-setup: could not install to the host; leaving it be." >&2
fi

# Arrange for the two files above to go away with the app. A path unit runs the
# cleanup whenever a flatpak `app/` directory changes, which is what an
# uninstall from the Store or the command line does, and a boot-time run catches
# anything the watch missed. The cleanup removes itself and both units once it
# has done its job.
#
# The script lives under /var/lib rather than /usr/local/libexec, which Pop!_OS
# does not have, and runs through bash so it needs no exec permission there.
# The hashes let it tell our files apart from the same paths written later by
# install.sh, and `roots` lets it tell an uninstalled app apart from a home
# directory that simply is not mounted yet.
#
# Best-effort like the install itself: without it the app works exactly as
# before, it just leaves the helper behind when uninstalled.
install_cleanup() {
    local state_dir=/var/lib/cosmic-nightlight
    local unit_dir=/etc/systemd/system
    local watched

    [[ -f "$cleanup" && -d /run/systemd/system ]] || return 1
    install -d -o root -g root -m 0755 "$state_dir" || return 1
    install -o root -g root -m 0644 "$cleanup" "$state_dir/cleanup.sh" || return 1
    sha256sum /usr/local/bin/cosmic-nightlight-helper \
        /etc/polkit-1/rules.d/49-cosmic-nightlight.rules >"$state_dir/sums" || return 1
    bash "$state_dir/cleanup.sh" roots >"$state_dir/roots" || return 1
    watched="$(bash "$state_dir/cleanup.sh" paths)" || return 1

    {
        echo "[Unit]"
        echo "Description=Remove the Night Light helper once its flatpak is uninstalled"
        echo
        echo "[Path]"
        while read -r dir; do
            echo "PathChanged=$dir"
        done <<<"$watched"
        echo "Unit=cosmic-nightlight-cleanup@watch.service"
        echo
        echo "[Install]"
        echo "WantedBy=paths.target"
    } >"$unit_dir/cosmic-nightlight-cleanup.path" || return 1

    cat >"$unit_dir/cosmic-nightlight-cleanup@.service" <<'EOF' || return 1
[Unit]
Description=Remove the Night Light helper once its flatpak is uninstalled (%i)
After=local-fs.target remote-fs.target

[Service]
Type=oneshot
ExecStart=/bin/bash /var/lib/cosmic-nightlight/cleanup.sh %i

[Install]
WantedBy=multi-user.target
EOF

    systemctl daemon-reload &&
        systemctl enable cosmic-nightlight-cleanup@boot.service &&
        systemctl enable --now cosmic-nightlight-cleanup.path
}

if [[ "$installed" -eq 1 ]]; then
    if install_cleanup; then
        echo "  /var/lib/cosmic-nightlight/ (removes all of this when the app is uninstalled)"
    else
        echo "cosmic-nightlight-setup: could not set up the uninstall cleanup." >&2
    fi
fi

# Hand the rest of the job to the helper. `exec "$helper" "$@"` and nothing else:
# the arguments are passed straight through as arguments, never re-parsed by a
# shell, and the helper validates them itself. This widens no privilege — anyone
# who can pkexec this script can already pkexec the helper directly.
if [[ "$#" -gt 0 ]]; then
    if [[ "$installed" -eq 1 ]]; then
        exec /usr/local/bin/cosmic-nightlight-helper "$@"
    fi
    # Install failed, so fall back to the copy inside the flatpak. We are already
    # root here, so this still applies; it just has to be done again next time.
    exec "$helper" "$@"
fi

if [[ "$installed" -eq 0 ]]; then
    exit 1
fi
