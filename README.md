<!-- SPDX-License-Identifier: MPL-2.0 -->
# Night Light for COSMIC

**Night Light** is an applet for the **COSMIC** desktop (Pop!_OS) that warms
your screen's colors to cut blue light at night. It sits on your **panel or
dock**: click it to turn it on or off and set the warmth. In **Settings** you can
have it follow the real sunset and sunrise, or set your own schedule, and adjust
the brightness. It's built with COSMIC's own widgets, so it looks like part of
the desktop.

It exists because COSMIC doesn't support the usual night light tools
(`redshift`, `gammastep`, `wlsunset`) yet. See [How it works](#how-it-works).

## Contents

- [Screenshots](#screenshots)
- [Install](#install)
  - [Try the flatpak build](#try-the-flatpak-build)
  - [Build from source (for development)](#build-from-source-for-development)
- [Using it](#using-it)
- [Translating](#translating)
- [Known limitations](#known-limitations)
- [How it works](#how-it-works)
- [Architecture](#architecture)
- [The real fix](#the-real-fix)

## Screenshots

<div align="center">
<table>
<tr>
<td align="center">
<img src="docs/screenshots/applet_light.png#gh-light-mode-only" height="300" alt="Night Light applet popup">
<img src="docs/screenshots/applet_dark.png#gh-dark-mode-only" height="300" alt="Night Light applet popup">
<br><sub>The panel popup: on/off toggle and warmth slider</sub>
</td>
<td align="center">
<img src="docs/screenshots/settings_light.png#gh-light-mode-only" height="300" alt="Night Light settings window">
<img src="docs/screenshots/settings_dark.png#gh-dark-mode-only" height="300" alt="Night Light settings window">
<br><sub>The settings window: schedule, warmth and brightness</sub>
</td>
</tr>
</table>
</div>

## Install

1. Download the latest **`cosmic-nightlight_*.deb`** from the
   [**Releases**](https://github.com/cosmic-nightlight/cosmic-nightlight/releases)
   page.
2. Open it with the **COSMIC Store** and click **Install**
   (or run `sudo apt install ./cosmic-nightlight_*.deb`).
3. Add the applet: **COSMIC Settings → Desktop → Panel** (or **Dock**)
   **→ Configure applets → Add applet → Night Light**.

That's it. Click the Night Light icon to turn it on or open its settings.

The app isn't in the COSMIC Store yet. A flatpak version is being submitted, and
until it's accepted, the `.deb` above is the way to install it.

### Try the flatpak build

This is optional. The `.deb` is the recommended install. Every release includes
a ready-built flatpak:

1. Download **`cosmic-nightlight-*.flatpak`** from the
   [**Releases**](https://github.com/cosmic-nightlight/cosmic-nightlight/releases)
   page.
2. `flatpak install --user ./cosmic-nightlight-*.flatpak`
3. Add the applet as above, or run `flatpak run io.github.cosmic_nightlight --settings`.

The first time you turn it on, it asks for your password once to install a small
helper on your system. A sandboxed app can't change screen colors on its own
([why](docs/flatpak-design.md)).

To remove it, run `flatpak uninstall io.github.cosmic_nightlight`, then
`./scripts/uninstall.sh` to remove that helper too.

<details>
<summary>Build the flatpak yourself</summary>

The build runs offline, so first generate `cargo-sources.json`, the list of
crates it needs:

```sh
sudo apt-get install flatpak flatpak-builder
flatpak install flathub com.system76.Cosmic.BaseApp org.freedesktop.Sdk//25.08 \
    org.freedesktop.Sdk.Extension.rust-stable//25.08

cd flatpak
python3 -m venv venv
./venv/bin/pip install aiohttp toml tomlkit
curl -sSLO https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/master/cargo/flatpak-cargo-generator.py
./venv/bin/python flatpak-cargo-generator.py ../Cargo.lock -o cargo-sources.json

flatpak-builder --force-clean --user --install build io.github.cosmic_nightlight.json
flatpak run io.github.cosmic_nightlight --settings
```

This builds the latest release. To build your local changes instead, run
`python3 generate-local-manifest.py` and build the manifest it creates. See
[flatpak/README.md](flatpak/README.md).

</details>

### Build from source (for development)

You'll need a Rust toolchain and `libdrm-dev`:

```sh
./scripts/install.sh --gui     # build and install everything
./scripts/uninstall.sh         # remove it all again
```

To build the `.deb`, see [PACKAGING.md](PACKAGING.md). For packagers:
[docs/flatpak-design.md](docs/flatpak-design.md) explains the flatpak setup, and
[docs/cosmic-store.md](docs/cosmic-store.md) covers the COSMIC Store listing.

## Using it

**The applet.** Click the icon for the on/off toggle, the warmth slider, and a
**Night Light Settings…** button.

**Settings.** Choose a schedule, the night warmth, and the brightness (which dims
the screen while the night light is on). Open it from the applet, from your app
launcher, or with `cosmic-nightlight --settings`.

**Schedules:**
- **Off**: you turn it on and off yourself.
- **Sunset to Sunrise**: it follows the real sun where you are, adjusting with
  the seasons. Your location comes from your time zone, so there's nothing to
  set up and no location permission. If sun times aren't available, it uses your
  **From**/**To** times instead and tells you why.
- **Custom Schedule**: pick exact **From** and **To** times. A schedule can run
  overnight (`9:00PM`→`6:00AM`) or during the day (`9:00AM`→`5:00PM`).

If you turn it on or off by hand, that lasts until the next scheduled change.

**Keeping the schedule running.** The applet runs your schedule as long as it's
on your panel or dock, so there's nothing else to set up. If you'd rather not
keep the applet on your panel, turn on **Run in Background** in Settings (it
only shows up when needed). Or use the systemd service:

```sh
systemctl --user enable --now cosmic-nightlight.service
```

(From a source install, copy `systemd/cosmic-nightlight.service` into
`~/.config/systemd/user/` first.)

<details>
<summary>Advanced: run the helper directly</summary>

Each call briefly flickers the screen:

```sh
pkexec /usr/bin/cosmic-nightlight-helper --temp 3500            # warm tint
pkexec /usr/bin/cosmic-nightlight-helper --temp 4000 --brightness 0.9
pkexec /usr/bin/cosmic-nightlight-helper --off                 # reset
```

(Use `/usr/local/bin/...` if you installed with `scripts/install.sh`.)
</details>

## Translating

The app is English-only for now, but it's ready for translations. A translation
is one file: a copy of
[`crates/cosmic-nightlight/i18n/en/cosmic_nightlight.ftl`](crates/cosmic-nightlight/i18n/en/cosmic_nightlight.ftl)
with the text translated. No coding needed, and partial translations are
welcome. See [docs/translating.md](docs/translating.md).

## Known limitations

- **The screen flickers briefly** (1–2 seconds) every time the tint changes.
- **Some events can clear the tint**, like changing resolution, plugging in a
  monitor, or the screen waking up. Resume from suspend is handled
  automatically. For the others, turn it off and on again, or wait for the next
  scheduled change.
- **Sunset to Sunrise can be off by a few minutes**, since it goes by your time
  zone rather than your exact location. Use a **Custom Schedule** if you want
  exact times.
- Requires admin rights (membership in `wheel` or `sudo`).

---

## How it works

COSMIC's compositor doesn't yet support the Wayland protocol that night light
tools use to change screen colors ([cosmic-comp#764]). A built-in night light is
planned for COSMIC **Epoch 3** ([cosmic-comp#2059], [cosmic-epoch#2498]).

So this app skips Wayland and writes the color curve straight to the kernel's
graphics layer (DRM), the same way `redshift` does outside a desktop. The
catch is that COSMIC holds an exclusive lock on the display (**DRM master**), so
other programs can't change it.

The workaround, from [jjo/drm-colortemp]: briefly switch to another virtual
terminal, which makes COSMIC release the lock. A root helper grabs it, writes
the colors, and switches back. **The tint stays after switching back.**

That switch is what causes the brief flicker. It will go away once COSMIC
supports a proper color protocol.

## Architecture

Three crates, so the part that runs as root stays small and separate from the
GUI:

| Crate | Runs as | Does |
| --- | --- | --- |
| [`nightlight-core`](crates/nightlight-core) | library | Color math ([`gamma.rs`](crates/nightlight-core/src/gamma.rs)), DRM writes ([`drm.rs`](crates/nightlight-core/src/drm.rs)), VT switching ([`vt.rs`](crates/nightlight-core/src/vt.rs)) |
| [`nightlight-helper`](crates/nightlight-helper) | **root** (via `pkexec`) | Small CLI: reads `--temp`/`--brightness` and calls core |
| [`cosmic-nightlight`](crates/cosmic-nightlight) | your user | The panel applet, `--settings` window and optional `--daemon`; calls the helper |

Any of the three modes can run the schedule. They share a lock and a record of
what's on screen in `$XDG_RUNTIME_DIR`, so running several at once still causes
only one change.

What happens on a tint change:

```
daemon/GUI ──pkexec──▶ cosmic-nightlight-helper (root)
                          │ 1. Switch to a spare VT      (COSMIC releases DRM master)
                          │ 2. Take DRM master and write the color curve to each display
                          │ 3. Release DRM master
                          └ 4. Switch back to your session  (tint stays)
```

The color curve uses Tanner Helland's color temperature formula. 6500 K means no
tint, and lower values are warmer. This allows much finer control than monitor
(DDC/CI) presets, and it works on laptop screens.

## The real fix

This is a stopgap. The proper fix is for COSMIC to support a color control
protocol; follow [cosmic-comp#764] and [cosmic-comp#2059]. When that happens,
this app can drop the workaround and become a normal Wayland app.

[cosmic-comp#764]: https://github.com/pop-os/cosmic-comp/issues/764
[cosmic-comp#2059]: https://github.com/pop-os/cosmic-comp/issues/2059
[cosmic-epoch#2498]: https://github.com/pop-os/cosmic-epoch/issues/2498
[jjo/drm-colortemp]: https://github.com/jjo/drm-colortemp
