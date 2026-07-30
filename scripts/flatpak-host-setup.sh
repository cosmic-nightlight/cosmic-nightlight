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
# This script is what puts it there. It ships inside the flatpak and is run once,
# by the user, through pkexec — so it costs one password prompt, after which the
# polkit rule makes every schedule transition password-less. Skipping it leaves
# the app working but prompting on every transition.
#
# Installs:
#   the helper    -> /usr/local/bin/cosmic-nightlight-helper
#   the rule      -> /etc/polkit-1/rules.d/49-cosmic-nightlight.rules
#
# Both paths are already whitelisted by the rule itself, so nothing here differs
# from what the .deb and install.sh set up.
#
# Usage (from inside the sandbox, where $app is /.flatpak-info's app-path):
#   flatpak-spawn --host pkexec "$app/libexec/cosmic-nightlight-setup"

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

for f in "$helper" "$rule"; do
    [[ -f "$f" ]] || { echo "cosmic-nightlight-setup: missing $f" >&2; exit 1; }
done

# Own both as root. The rule grants password-less root to whatever sits at the
# helper path, so that path must not be writable by the user it is granting to —
# copying out of the flatpak (which is user-writable on a user install) is the
# whole point of this step.
install -o root -g root -m 0755 "$helper" /usr/local/bin/cosmic-nightlight-helper
install -D -o root -g root -m 0644 "$rule" \
    /etc/polkit-1/rules.d/49-cosmic-nightlight.rules

echo "Installed:"
echo "  /usr/local/bin/cosmic-nightlight-helper"
echo "  /etc/polkit-1/rules.d/49-cosmic-nightlight.rules"
