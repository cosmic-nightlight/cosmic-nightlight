<!-- SPDX-License-Identifier: MPL-2.0 -->
# Architecture

How the code is laid out, for anyone working on it. For how the app changes
the screen colors at all, see [How it works](../README.md#how-it-works) in the
README. For the Flatpak build, see [flatpak-design.md](flatpak-design.md).

## Crates

Three crates, so the part that runs as root stays small and separate from the
GUI:

| Crate | Runs as | Does |
| --- | --- | --- |
| [`nightlight-core`](../crates/nightlight-core) | library | Color math ([`gamma.rs`](../crates/nightlight-core/src/gamma.rs)), DRM writes ([`drm.rs`](../crates/nightlight-core/src/drm.rs)), the VT switching fallback ([`vt.rs`](../crates/nightlight-core/src/vt.rs)) |
| [`nightlight-helper`](../crates/nightlight-helper) | root (via `pkexec`) | Small CLI: reads `--temp`/`--brightness` and calls core |
| [`cosmic-ext-nightlight`](../crates/cosmic-ext-nightlight) | your user | The settings window (default), `--applet`, and optional `--daemon`; calls the helper |

The applet, the settings window and the daemon can each run the schedule. They
share a lock and a record of what's on screen in `$XDG_RUNTIME_DIR`, so running
several at once still causes only one change.

## What happens on a tint change

```
daemon/GUI ──pkexec──▶ cosmic-ext-nightlight-helper (root)
                          │ 1. Borrow a copy of COSMIC's display handle
                          └ 2. Write the color curve to each display through it
```

If a display didn't take the change, the fallback runs:

```
│ 1. Switch to a spare VT      (COSMIC releases DRM master)
│ 2. Take DRM master and write the color curve to each display
│ 3. Release DRM master
└ 4. Switch back to your session  (tint stays)
```
