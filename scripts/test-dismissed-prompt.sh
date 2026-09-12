#!/usr/bin/env bash
# Verifies the dismissed-password-prompt fix end to end.
#
# The fix: pkexec refusing (dialog dismissed, or not authorized) is damped on its
# own scale — an hour, doubling to six — instead of the 5s..5min scale used for
# faults that clear on their own. Before it, one dismissal at sunset re-opened the
# dialog about a hundred times before sunrise.
#
# To make pkexec actually prompt, the host helper and the polkit rule have to be
# out of the way. This stashes both, runs the flatpak's daemon for 90 seconds
# while you dismiss ONE prompt, then puts everything back — including on Ctrl-C,
# on error, and on kill.
#
# Nothing runs unattended: you type your own password for the stash, and the one
# polkit dialog is the thing under test.

set -uo pipefail

STASH="$(mktemp -d /tmp/nightlight-test-stash.XXXXXX)"
HELPER=/usr/local/bin/cosmic-nightlight-helper
RULE=/etc/polkit-1/rules.d/49-cosmic-nightlight.rules
CFG="$HOME/.config/cosmic/io.github.cosmic_nightlight/v1/override"
LOG="$STASH/daemon.log"
RUN_SECONDS=90

had_helper=0
had_rule=0
had_override=0
stashed=0

restore() {
    # Idempotent: safe to reach twice (EXIT fires after INT).
    [[ "$stashed" -eq 1 ]] || return 0
    stashed=0
    echo
    echo ">> Restoring..."

    if [[ "$had_helper" -eq 1 ]]; then
        sudo install -o root -g root -m 0755 "$STASH/helper" "$HELPER" &&
            echo "   restored $HELPER"
    else
        # It wasn't there before. If the test installed one, take it back out.
        sudo rm -f "$HELPER"
    fi

    if [[ "$had_rule" -eq 1 ]]; then
        sudo install -o root -g root -m 0644 "$STASH/rule" "$RULE" &&
            echo "   restored $RULE"
    else
        sudo rm -f "$RULE"
    fi

    if [[ "$had_override" -eq 1 ]]; then
        cp "$STASH/override" "$CFG" && echo "   restored the override setting"
    else
        rm -f "$CFG"
        echo "   removed the override setting (there was none before)"
    fi

    # The tint should never have landed — the prompt was dismissed — but if it
    # did, put the screen back rather than leaving it warm.
    if [[ "$had_helper" -eq 1 ]]; then
        sudo "$HELPER" --off >/dev/null 2>&1 && echo "   reset the screen tint"
    fi

    echo ">> Verifying:"
    [[ "$had_helper" -eq 1 ]] && { [[ -x "$HELPER" ]] && echo "   OK  helper present" || echo "   !!  HELPER MISSING - copy is at $STASH/helper"; }
    [[ "$had_rule" -eq 1 ]] && { sudo test -f "$RULE" && echo "   OK  polkit rule present" || echo "   !!  RULE MISSING - copy is at $STASH/rule"; }
    echo
    echo "Stash kept at $STASH (delete it once you're happy)."
}
trap restore EXIT INT TERM

echo "=== Dismissed-prompt backoff test ==="
echo

if flatpak ps --columns=application 2>/dev/null | grep -q cosmic_nightlight; then
    cat <<'EOF'
!! A cosmic-nightlight flatpak instance is already running - almost certainly the
   panel applet. Every run mode reconciles, so it will hit the same stashed
   helper and put up its own password dialogs alongside the daemon's.

   That matters most if it is still the OLD build: 0.4.0 retries a dismissal on
   the 5s..5min scale, so it will keep re-prompting throughout the test.

   Note that `flatpak kill` on its own does NOT clear it. cosmic-panel restarts
   anything listed in its plugin config the moment the process dies - the same
   behavior scripts/uninstall.sh strips the panel entry to work around.

   Two ways forward:

   a) Take Night Light off the bar for the duration (cleanest):
        COSMIC Settings > Desktop > Panel (or Dock) > Configure applets
      then re-add it afterwards. Re-run this script once it is off.

   b) Leave it, but first make sure it is running the FIXED build, by logging
      out and back in (or restarting cosmic-panel). Then expect TWO dialogs
      rather than one - the applet and the daemon each back off independently,
      which is itself the per-process behavior worth seeing. The verdict below
      counts only the daemon's own attempts, so it stays valid either way.

   Re-run with ALLOW_RUNNING=1 to proceed anyway (option b).
EOF
    [[ "${ALLOW_RUNNING:-0}" == "1" ]] || exit 1
    echo
    echo ">> ALLOW_RUNNING=1 set; continuing with the applet running."
fi

echo ">> Need sudo to move the helper and rule aside."
sudo -v || exit 1

# --- stash -----------------------------------------------------------------
[[ -e "$HELPER" ]] && { sudo cp "$HELPER" "$STASH/helper" && had_helper=1; }
sudo test -f "$RULE" && { sudo cp "$RULE" "$STASH/rule" && sudo chown "$USER" "$STASH/rule" && had_rule=1; }
[[ -f "$CFG" ]] && { cp "$CFG" "$STASH/override" && had_override=1; }
stashed=1

echo "   stashed: helper=$had_helper rule=$had_rule override=$had_override"
sudo rm -f "$HELPER" "$RULE"

# polkitd reloads rules.d on inotify; give it a moment to notice the removal.
sleep 2

# Ask for a tint, so the daemon has something to apply.
mkdir -p "$(dirname "$CFG")"
printf '"on"' > "$CFG"

# --- run -------------------------------------------------------------------
cat <<EOF

>> Running the daemon for ${RUN_SECONDS}s.

   A password dialog will appear naming "cosmic-nightlight-setup".
   DISMISS IT (Escape / Cancel) — once. Then just wait.

   What to watch for: with the fix, that is the ONLY dialog you see.
   Without it you'd get another after 5s, then 10s, 20s, 40s...

EOF
read -rp "   Press Enter to start. " _

timeout "$RUN_SECONDS" flatpak run io.github.cosmic_nightlight --daemon >"$LOG" 2>&1
echo
echo ">> Done. Daemon output:"
echo "---------------------------------------------------------------"
cat "$LOG"
echo "---------------------------------------------------------------"

# --- verdict ---------------------------------------------------------------
attempts=$(grep -c "helper exited with" "$LOG" 2>/dev/null || echo 0)
auth=$(grep -c "apply failed (Auth)" "$LOG" 2>/dev/null || echo 0)
other=$(grep -c "apply failed (Other)" "$LOG" 2>/dev/null || echo 0)
hour=$(grep -c "retrying in 3600s" "$LOG" 2>/dev/null || echo 0)

echo
echo ">> Verdict"
echo "   pkexec attempts (= dialogs shown): $attempts"
echo "   classified Auth:                   $auth"
echo "   classified Other:                  $other"
echo "   backed off a full hour:            $hour"
echo

if [[ "$attempts" -eq 1 && "$auth" -eq 1 && "$hour" -eq 1 ]]; then
    echo "   PASS - one dialog, classified as a refusal, backed off an hour."
elif [[ "$attempts" -eq 0 ]]; then
    echo "   INCONCLUSIVE - nothing was attempted. Either the tint was already"
    echo "   applied, or the daemon never got past waiting for the session VT."
elif [[ "$other" -gt 0 ]]; then
    echo "   INVESTIGATE - a refusal was classified 'Other', so it is being"
    echo "   retried on the fast scale. Check the exit status in the log above:"
    echo "   126 is a dismissed dialog, 127 is not-authorized/could-not-run."
else
    echo "   INVESTIGATE - expected exactly one dialog; got $attempts."
    echo "   If you dismissed more than one prompt, that accounts for it."
fi
