<!-- SPDX-License-Identifier: MPL-2.0 -->
# Shipping Night Light as a flatpak

How the applet reaches the COSMIC Store despite needing root, and why it is built
the way it is. Written for anyone reviewing the
[cosmic-flatpak](https://github.com/pop-os/cosmic-flatpak) submission, or picking
this work up later.

---

## The goal

Being listed and searchable in the COSMIC Store, under Applets. The Store draws
from three catalogs, and only one takes submissions:

| Catalog | How you get in |
|---|---|
| System (DEP-11 from apt repos) | Requires Ubuntu universe or System76 packaging it. No submission process. |
| Flathub | App ID has three components; Flathub requires four for `io.github.*`. |
| **COSMIC Flatpak** | **A normal pull request.** The target. |

The Applets page itself is not flatpak-only — `categories()` in the Store's
`main.rs` searches every backend and filters only on the component kind and a
`provides` ID, which `data/io.github.cosmic_nightlight.metainfo.xml` already
carries. It is the *catalogs* that are closed, not the page.

## The constraint

Setting a gamma ramp under COSMIC means writing LUTs straight to DRM/KMS,
because cosmic-comp does not implement `wlr-gamma-control-unstable-v1`
(pop-os/cosmic-comp#2059). That needs DRM master:
`drm_mode_gamma_set_ioctl` is registered `DRM_MASTER`, and
`drm_master_check_perm` requires `CAP_SYS_ADMIN`. Because the compositor holds
master, the write also has to happen inside a VT bounce, which needs root
independently.

**No flatpak permission grants `CAP_SYS_ADMIN`.** This is not a matter of asking
for enough; there is nothing to ask for. `--device=all` is sufficient for the raw
i2c that external-monitor-brightness does — i2c is reachable with a udev rule —
but it does not extend here.

So a sandboxed build cannot tint the screen, however it is configured. The
privileged work has to happen on the host.

## The design

The applet runs sandboxed and calls out:

```
applet (sandbox) → flatpak-spawn --host → pkexec → helper (host, root)
```

The helper therefore has to run on the host. It ships at
`/app/libexec/cosmic-nightlight-helper`, which serves two purposes.

First, it is the payload for a **one-time, user-triggered setup**
([`scripts/flatpak-host-setup.sh`](../scripts/flatpak-host-setup.sh)), which
copies it to `/usr/local/bin/` and installs the polkit rule beside it. Both of
those paths are already whitelisted by
[`polkit/49-cosmic-nightlight.rules`](../polkit/49-cosmic-nightlight.rules), so
nothing about the rule differs between the `.deb` and the flatpak.

Second — and this is what keeps the app from being broken on arrival — it is
runnable *where it sits*, before any setup has happened. A flatpak's own files
are visible on the host, and `/.flatpak-info` names that location as `app-path`,
so `probe_host_helper` in
[`backend.rs`](../crates/cosmic-nightlight/src/backend.rs) tries the two
whitelisted host paths first and falls back to the bundled copy's host path.
pkexec will run it; no rule names that path, so it prompts every time.

**Before setup the app therefore still works** — it just prompts for a password
on every schedule transition. The setup does not unlock the feature; it stops the
nagging. That distinction matters: this is not an app that is broken until
granted root.

### What happens when the user says no

"A prompt per transition" is the price only if the user answers them. Dismissing
one is a failed apply, and the retry backoff is what then decides how often the
dialog comes back — which on the ordinary five-second-to-five-minute scale meant
about a hundred prompts between sunset and sunrise. That is the sort of thing
that gets an app pulled from a store.

So pkexec's refusals are damped on a scale of their own — an hour, doubling to
six (`FIRST_AUTH_RETRY_DELAY` in `backend.rs`). The distinction being drawn is
between a fault that will clear while nobody is watching (a display asleep, no
CRTCs up yet, the session not foreground — all of which reach the helper and come
back as its own exit 1) and a person declining, which will not clear until that
person decides otherwise. Only the second kind costs the user a dialog, so only
the second kind needs to be rare.

That damping is **shared between our processes**, in the session runtime
directory beside the record of what is on screen (`backoff_path` in
`backend.rs`). It has to be: the applet, the settings window and the daemon all
reconcile on the same tick and queue up behind each other on the apply lock, so a
refusal only one of them collected would hold off only that one. With a per-
process backoff, one click with the applet and the settings window both open put
up two dialogs — the first process asked and was refused, and the second, already
blocked on the lock with nothing in the record to distinguish "refused" from
"never tried", walked straight into pkexec as soon as the lock came free. A
success suppressed that duplicate and a refusal did not, so it showed up only
before the user had authenticated once.

Sharing the record also means the delay doubles once per attempt rather than once
per attempt *per process*, so three running processes climb one ladder instead of
three.

It stays finite rather than never retrying, because a prompt can be dismissed by
accident or a password mistyped. Both ways back are faster than waiting it out
and neither consults the backoff: the toggle applies directly, and the settings
row runs the setup directly.

### One question at a time

A refusal answers everything that was already asked, not just the request that
happened to be carrying it. This matters because the app puts several routes to a
privileged call in front of the user at once: with a prompt up for the toggle,
they can still move the temperature slider, move the brightness slider, and pick
a schedule — and each of those wants privilege of its own.

Two rules keep that to a single dialog.

**Everything privileged takes the apply lock**, `run_host_setup` included. It is
the only one that does not apply a tint, and before it took the lock its prompt
could land *beside* an apply's rather than after it — picking a schedule while
the toggle's prompt was still up put two dialogs on screen at once.

**A refusal answers every request made before it.** The applies queued behind a
prompt were all decided while the user had no answer to go on, so cancelling the
prompt cancels them too; they never reach pkexec (`refused_since` in
`backend.rs`). The cut is by time, not by state — what was declined is the
authentication, and a request carrying a different temperature would raise the
identical dialog.

The timing is the whole point of the rule, and it is why the backoff is not
simply consulted here. A forced apply *does* ignore the backoff, deliberately:
clicking the toggle again after dismissing a prompt has to work, or a slip costs
an hour. So what separates the two is when the user acted. Before the refusal,
they were acting in ignorance of it and it answers for them; after it, they have
seen it and are trying again.

pkexec spends exit code 127 on both "not authorized" and "could not run the
program", which want opposite treatment — the first will not improve, the second
is a helper swapped out mid-flight and should be retried promptly. They are told
apart by looking to see whether the program is still where we left it.

### Why the resolved path is cached only when whitelisted

Probing costs a round trip out of the sandbox per candidate, so the answer wants
caching — but caching it unconditionally is a trap. The setup is offered from
*inside the running app*, and it is the one thing that changes the answer. An
instance that cached the bundled path before the setup ran would keep prompting
after it, which reads as a setup that silently failed.

So `host_helper_path` keeps only a whitelisted result, which nothing the user
does later can improve on. The bundled fallback is re-probed on each apply, so
the setup takes effect on the very next transition with no restart.

A remembered path can still be *removed*, though — by a `.deb` upgrade swapping
the binary mid-flight, or an uninstall — so any failed apply drops it and the
next attempt resolves from scratch. That covers the one case caching could
otherwise wedge until a restart. Failures are mostly something else entirely (a
dismissed prompt, displays asleep), where the cost is a single extra re-probe
already damped by the retry backoff.

The cost lands in the right place throughout: extra probes are paid only while
degraded or after a failure, where a password prompt or a backoff already dwarfs
them. A set-up system on the happy path probes exactly once.

The user-facing sequence:

1. Install from the Store.
2. Turn the night light on. One password prompt — which this build was going to
   ask for anyway, since nothing is set up yet.
3. Never prompted again.

There is no separate setup step, because there does not need to be. Before the
setup, *every* privileged call costs a prompt; spending the first one on the
install as well as the change is strictly better than spending it on the change
alone and asking again at sunset. So the first privileged call routes through
`cosmic-nightlight-setup`, which installs and then forwards the same arguments to
what it installed. See `privileged_program` in `backend.rs`.

The polkit dialog names that program, so the install is disclosed where consent
is actually given rather than buried in a notice the user would dismiss. The
settings row below remains as the deliberate path — for anyone who dismissed the
prompt, and for a contract update.

The install is best-effort: a host whose `/usr/local` or `/etc` cannot be written
still gets its tint change, because the script forwards the arguments either way
rather than taking the screen down with the install. That leaves a success the
app would otherwise misread — it routes through the setup, sees exit 0, and
routes through it again next time, charging a password prompt for an install that
can never land. So a setup that returns having left the host no better off is
recorded as ineffective (`SETUP_INEFFECTIVE`) and not attempted again; later
applies fall back to the bundled helper, which still prompts, but stops promising
something that is not going to happen.

## Why not the obvious alternatives

**Point the polkit rule at the helper inside the flatpak.** This is a local
privilege escalation and must not ship. The Store installs the `cosmic` remote
per-user, so the app's files live under `~/.local/share/flatpak/` — writable by
the very user the rule would be granting password-less root to. Copying the
binary out to a root-owned, root-only-writable path is the entire point of the
setup step.

**Make the setup itself password-less, so updates are silent.** Same hole one
step removed. You would have one rule saying "this binary runs as root without a
password" and another saying "any process running as the user may overwrite that
binary without a password."

> You can have password-less execution, or password-less replacement — not both.

**What the setup does still trust, and cannot check.** The binary it copies out
lives inside the flatpak, and on a per-user install (which is what the Store
does) that tree is owned by the user: `~/.local/share/flatpak/app/…/files/libexec`
is `drwxr-xr-x` under their own uid. Anything already running as that user can
replace the payload before the setup copies it, and what lands root-owned at a
whitelisted path is then whatever they put there — password-less root from that
point on, without ever knowing a password.

This is not fixable from inside the sandbox, and it is worth being exact about
why: nothing in a user-writable tree can attest to itself. A digest in the setup
script is checkable by whoever can also edit the script; one compiled into the
GUI is checkable by whoever can also replace the GUI. The trust anchor has to sit
outside the tree, and for a per-user flatpak there is no such anchor to reach.

What it is, then, is the same trust the user extends by running `install.sh` from
a checkout, and it is bounded the same way: it takes an authenticated prompt, so
it cannot happen without the user, and it is disclosed at the prompt because
pkexec names the program. What it converts is *one* authenticated root action
into a persistent one. A system-wide flatpak install has no such gap, the tree
being root-owned.

Disclose this in the submission. A reviewer who spots it unaided will reasonably
wonder what else went unexamined.

**Wait for the compositor.** [cosmic-comp#1543](https://github.com/pop-os/cosmic-comp/pull/1543)
was closed unmerged by Drakulix in July 2025: gamma belongs to the post-1.0 color
rendering pipeline, and he would not commit to an algorithm before then.
[#2417](https://github.com/pop-os/cosmic-comp/pull/2417) answers the technical
half — it uses the atomic `GAMMA_LUT` property, which was his other objection —
but it is a draft from a non-member with no maintainer response since May 2026.
Worth a nudge; not worth scheduling behind.

## The helper version contract

The host copy is not updated when the flatpak is. If a release changed the helper,
the user would silently run an old one against a new GUI.

Re-prompting on every app update would be a nag. The right trigger is not "did the
app update" but "does the installed helper still speak the language the GUI
needs" — so the helper's command line is treated as a **frozen contract**:

```
--temp <kelvin>   --brightness <0.0-1.0>   --off   --session-vt <n>
```

Nothing in the last four releases changed it. The helper exposes `--version`, the
GUI checks the host copy at startup, and the setup is re-offered only when the
contract has genuinely moved — in practice, rarely or never.

The number is
[`nightlight_core::HELPER_CONTRACT`](../crates/nightlight-core/src/lib.rs), which
is **not** the app version: keying off the app version would re-prompt on every
release, which is the nag this exists to avoid. `--version` prints both:

```
cosmic-nightlight-helper 0.4.0 (contract 1)
```

The format is written by `version_line` and read back by `parse_contract`, which
sit next to each other so the two ends cannot drift. `--version` is answered
before the root check, so asking costs no password.

A helper old enough to predate `--version` rejects the flag and exits non-zero,
which reads as no contract at all — the same verdict as a contract that is merely
too low, and the same remedy, so both surface as `HostSetup::Outdated`.

Bump `HELPER_CONTRACT` only for a change that would make an *older* helper
mishandle what the GUI sends: an argument removed or renamed, a unit or range
redefined, a new argument the GUI relies on. Adding an argument the GUI does not
require is not a bump.

## The setup UI

Setup normally happens without any UI at all — it rides along on the first tint
change, as above. What remains is one row at the top of the settings window, for
the cases where that did not happen: the user dismissed the password prompt, or a
contract bump means the installed helper needs replacing.

No wizard, no modal, no first-run gate. That is a deliberate reading of what this
app is: it works before the setup, and only pays a password prompt per change. A
startup wizard would tell the user the opposite, and would put a chore in front of
a night light. So the row is worded as the benefit — *Skip the password prompt* —
rather than as a requirement, and says *Update the installed helper* when the
contract has moved, since the remedy is the same script.

Its existence is **derived, never stored**: it renders when
`backend::host_setup()` reports anything other than `Ready`. There is no
"already dismissed" flag to go stale, a `.deb` install never sees it because the
condition cannot be true there, and it reappears correctly if the host helper is
later removed. After a successful setup it disappears on its own, with no
restart — which is exactly what the caching rule above buys.

Derived means derived *on the tick*, not once at startup. The setup usually
happens without this row being touched at all — it rides along on the first tint
change, and the toggle immediately above the row is one of the things that
triggers one. A row answered only by its own button would sit there afterwards
still offering a setup that had already happened, and charge a password prompt
for pressing it. Re-deriving costs nothing on the tick: the backend holds the
answer and re-probes the host only after something it ran could have changed it.

The setup blocks on a polkit dialog, so it runs on its own thread and reports
back through a channel; the window stays live and the button reads *Working…*
while it is outstanding. A failed attempt keeps the row and appends the reason,
rather than failing silently.

The discipline this costs: do not change the helper's arguments casually. Since
the helper exists precisely to be the smallest possible thing running as root,
that is a discipline worth having anyway.

## What ships where

| Path in the flatpak | Purpose |
|---|---|
| `/app/bin/cosmic-nightlight` | the applet and settings window |
| `/app/libexec/cosmic-nightlight-helper` | payload for the setup; never run in-sandbox |
| `/app/libexec/cosmic-nightlight-setup` | the one-time host setup script |
| `/app/share/polkit-1/rules.d/49-…rules` | payload for the setup |

Installed on the host by the setup:

| Path | Owner |
|---|---|
| `/usr/local/bin/cosmic-nightlight-helper` | `root:root 0755` |
| `/etc/polkit-1/rules.d/49-cosmic-nightlight.rules` | `root:root 0644` |

## Permissions requested

```
--share=ipc  --socket=wayland  --device=dri
--talk-name=com.system76.CosmicSettingsDaemon
--talk-name=com.system76.CosmicSettingsDaemon.Config.*
--talk-name=org.freedesktop.Flatpak
--filesystem=xdg-config/cosmic:rw
```

`--device=dri` is for libcosmic's GPU rendering, not for DRM — the sandbox never
touches DRM. There is no `--device=all` and no `--filesystem=host`. This is a
lighter permission set than minimon-applet, which is already published there.

The `Config.*` name is what makes the applet follow a light/dark switch, and the
subtree wildcard is load-bearing. `watch_config` on the daemon's own name hands
back a *per-config* service — `com.system76.CosmicSettingsDaemon.Config.com.system76.CosmicTheme.Mode.V1`
and one sibling per theme config — and every subsequent change signal is sent
from there, not from the daemon. Talking to the daemon alone is enough to set the
watch up, so it fails silently: libcosmic reports the watcher as created, and
then no update ever arrives. See "the theme watch is a different bus name" below.

The wildcard covers every app's watcher name, not just the theme's, but it opens
nothing new: those services carry a change signal for a config under
`~/.config/cosmic`, which `--filesystem=xdg-config/cosmic:rw` already reads and
writes outright.

Note that the host-side install is **runtime behavior and invisible to the
manifest**, and cosmic-flatpak's CI only checks that the manifest builds. That is
a reason to disclose it in the submission, not a reason to rely on it going
unnoticed.

## Verified, not assumed

Established by building the flatpak and running it, since none of it is in a spec:

- `XDG_VTNR` **does** survive into the sandbox, so `--session-vt` keeps working.
  The host side of `flatpak-spawn` does **not** inherit it, which is why it must
  be passed explicitly.
- `flatpak-spawn --host` reaches pkexec with only `--talk-name=org.freedesktop.Flatpak`.
- Two flatpak instances share `XDG_RUNTIME_DIR`, so the apply lock and
  applied-state record coordinate between the applet and settings window. The
  host does not see it — a user running both the `.deb` and the flatpak would get
  two uncoordinated schedulers.
- `/sys/class/tty/tty0/active` is readable in-sandbox, so the daemon's
  foreground-VT check needs no extra permission.
- `/usr/local/libexec` does **not** exist on Pop!_OS. `/usr/local/bin` does, and
  is already whitelisted by the rule.
- Flatpak rewrites `Exec=` on export and preserves both the `--settings` argument
  and `X-CosmicApplet=true`, so the applet picker lists it.
- `/.flatpak-info` carries the sandbox's host-side location as `app-path` under
  `[Instance]`, already resolved to the running commit — so reaching the bundled
  helper needs no guess about per-user vs. system installs or which commit is
  current.

- pkexec imposes no ownership or writability requirement on the program it is
  asked to run, so the bundled helper runs from the user-writable flatpak tree.
  It only *prompts*, which is the intended pre-setup behavior. Confirmed on a
  real build: a fresh flatpak install on a host with no helper and no rule tints
  the screen, asking for a password on every transition.

- **The theme watch is a different bus name than the daemon.** Run in-sandbox
  under `--talk-name=com.system76.CosmicSettingsDaemon` alone, the applet's
  theme-mode watcher is created without error and then never fires: flipping
  light/dark leaves the icon the old color until the applet is restarted, which
  is what re-adding it to the panel does. `busctl --user list` shows why — the
  daemon parks each watched config on its own well-known name under
  `com.system76.CosmicSettingsDaemon.Config.`, and the `Changed` signal comes
  from there, so the sandbox's bus proxy drops it. Adding
  `--talk-name=com.system76.CosmicSettingsDaemon.Config.*` makes the signal
  arrive and the icon re-color in place. Nothing outside the sandbox sees this;
  the same binary on the host follows the switch fine.

  Only libcosmic's own watches go over D-Bus. Our settings ride
  `Config::watch` (inotify), which the bind-mounted `xdg-config/cosmic` already
  serves — which is why applet and settings window mirrored each other
  correctly the whole time this was broken.

## Status

Done, on branch `flatpak-sandbox-support`:

- sandbox detection and host routing in `crates/cosmic-nightlight/src/backend.rs`
- the fallback to the bundled helper, so a fresh install tints the screen before
  the setup has ever been run
- the manifest, in [`flatpak/`](../flatpak/)
- the host setup script
- `scripts/uninstall.sh` covering a flatpak install, which nothing else removes
- `--version` on the helper, and the contract check behind it
- setup on first use, riding along on the first tint change
- the setup UI — one derived row in the settings window, as the fallback path

- the version guard over the metainfo `<releases>` block, in
  `scripts/check-version.sh`, run by the release workflow before it publishes

Not done:

- a release tag for the manifest to point at, in place of a branch
- the submission itself
