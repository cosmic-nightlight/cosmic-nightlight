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

The helper therefore has to already be on the host, and a flatpak cannot put it
there. So it ships at `/app/libexec/cosmic-nightlight-helper` purely as the
payload for a **one-time, user-triggered setup**
([`scripts/flatpak-host-setup.sh`](../scripts/flatpak-host-setup.sh)), which
copies it to `/usr/local/bin/` and installs the polkit rule beside it.

Both of those paths are already whitelisted by
[`polkit/49-cosmic-nightlight.rules`](../polkit/49-cosmic-nightlight.rules), so
nothing about the rule differs between the `.deb` and the flatpak.

**Before setup the app still works** — pkexec just prompts for a password on
every schedule transition. The setup does not unlock the feature; it stops the
nagging. That distinction matters: this is not an app that is broken until
granted root.

The user-facing sequence:

1. Install from the Store. It works, but prompts on every transition.
2. A notice offers to fix that.
3. One password prompt.
4. Never prompted again.

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
--talk-name=org.freedesktop.Flatpak
--filesystem=xdg-config/cosmic:rw
```

`--device=dri` is for libcosmic's GPU rendering, not for DRM — the sandbox never
touches DRM. There is no `--device=all` and no `--filesystem=host`. This is a
lighter permission set than minimon-applet, which is already published there.

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

## Status

Done, on branch `flatpak-sandbox-support`:

- sandbox detection and host routing in `crates/cosmic-nightlight/src/backend.rs`
- the manifest, in [`flatpak/`](../flatpak/)
- the host setup script

Not done:

- `--version` on the helper and the contract check
- the setup UI — the script exists, but nothing offers to run it
- extending `scripts/release-notes.sh` to guard the metainfo `<releases>` version,
  which it does not cover today
- a release tag for the manifest to point at, in place of a branch
- the submission itself
