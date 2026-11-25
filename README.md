<!-- SPDX-License-Identifier: MPL-2.0 -->
<div align="center">
  <img src="data/icons/hicolor/scalable/apps/io.github.cosmic_ext_nightlight.svg" width="96" alt="">
  <h1>Night Light for COSMIC™</h1>
  <p>
    <a href="#install">Install</a> ·
    <a href="#using-it">Using it</a> ·
    <a href="#known-limitations">Known limitations</a> ·
    <a href="#how-it-works">How it works</a>
  </p>
</div>

Night Light is an applet for the COSMIC desktop that warms your screen's colors
to cut blue light at night. It sits on your panel or dock. Click it to turn
it on or off and set the warmth. In Settings you can have it
follow the real sunset and sunrise, or set your own schedule, and adjust the
brightness.

It exists because COSMIC doesn't support the usual night light tools
(`redshift`, `gammastep`, `wlsunset`) yet. See [How it works](#how-it-works).

This is an unofficial community project. It isn't made or endorsed by System76.

## Screenshots

<div align="center">
<table>
<tr>
<td align="center">
<img src="docs/screenshots/applet_light.png#gh-light-mode-only" height="300" alt="Night Light applet popup">
<img src="docs/screenshots/applet_dark.png#gh-dark-mode-only" height="300" alt="Night Light applet popup">
<br><sub>The panel popup, with the on/off toggle and warmth slider</sub>
</td>
<td align="center">
<img src="docs/screenshots/settings_light.png#gh-light-mode-only" height="300" alt="Night Light settings window">
<img src="docs/screenshots/settings_dark.png#gh-dark-mode-only" height="300" alt="Night Light settings window">
<br><sub>The settings window, with the schedule, warmth and brightness</sub>
</td>
</tr>
</table>
</div>

## Install

**1. Get the package.** On Pop!_OS or Ubuntu:

- Download the latest `cosmic-ext-nightlight_*.deb` from the
  [Releases](https://github.com/cosmic-nightlight/cosmic-nightlight/releases)
  page.
- Open it with the **COSMIC Store** and click **Install**
  (or run `sudo apt install ./cosmic-ext-nightlight_*.deb`).

On another system, open yours below:

<details>
<summary><b>Fedora</b></summary>

You'll need Fedora 44 or newer. Download the latest `cosmic-ext-nightlight-*.rpm`
from the [Releases](https://github.com/cosmic-nightlight/cosmic-nightlight/releases)
page and run:

```sh
sudo dnf install ./cosmic-ext-nightlight-*.rpm
```

On Fedora Atomic or COSMIC Atomic, run this instead, then restart your computer:

```sh
rpm-ostree install ./cosmic-ext-nightlight-*.rpm
```

If you have version 0.6 or earlier installed, it has a different name. Replace
it with:

```sh
rpm-ostree uninstall cosmic-nightlight --install ./cosmic-ext-nightlight-*.rpm
```

</details>

<details>
<summary><b>Arch Linux</b></summary>

Download the latest `cosmic-ext-nightlight-*.pkg.tar.zst` from the
[Releases](https://github.com/cosmic-nightlight/cosmic-nightlight/releases)
page and run:

```sh
sudo pacman -U ./cosmic-ext-nightlight-*.pkg.tar.zst
```

</details>

<details>
<summary><b>NixOS</b></summary>

If something doesn't work, please [open an issue](https://github.com/cosmic-nightlight/cosmic-nightlight/issues).
The flatpak doesn't work on NixOS.

If your system uses a flake, add this line inside `inputs` in `/etc/nixos/flake.nix`:

```nix
cosmic-ext-nightlight.url = "github:cosmic-nightlight/cosmic-nightlight/v0.7.0";
```

Add `cosmic-ext-nightlight` to the list after `outputs = {`, and add these two
lines inside your system's `modules = [ ... ]` list:

```nix
cosmic-ext-nightlight.nixosModules.default
{ programs.cosmic-ext-nightlight.enable = true; }
```

Then rebuild as you normally do.

**Updating to a new release:** change `v0.7.0` in the `cosmic-ext-nightlight.url`
line to the new version, then run
`sudo nix flake update cosmic-ext-nightlight --flake /etc/nixos` and rebuild.

**Trying the Nightly build:** use `nightly` in place of `v0.7.0` in that line.
It gets fixes first but can have bugs. Running the same `nix flake update`
command and rebuilding then brings you the newest one.

<details>
<summary>New to flakes? Step by step</summary>

**1. Turn on flakes.** Add this line to `/etc/nixos/configuration.nix`, inside
the main `{ ... }` block:

```nix
nix.settings.experimental-features = [ "nix-command" "flakes" ];
```

Then run:

```sh
sudo nixos-rebuild switch
```

**2. Create the flake file.** Check which NixOS version you're on:

```sh
nixos-version
```

It prints something like `25.11.1234.abcdef`, and you need the first part
(`25.11` here). Create `/etc/nixos/flake.nix` with this in it, changing `25.11`
to your version:

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";
    cosmic-ext-nightlight.url = "github:cosmic-nightlight/cosmic-nightlight/v0.7.0";
  };

  outputs = { nixpkgs, cosmic-ext-nightlight, ... }: {
    nixosConfigurations.nixos = nixpkgs.lib.nixosSystem {
      modules = [
        ./configuration.nix
        cosmic-ext-nightlight.nixosModules.default
        { programs.cosmic-ext-nightlight.enable = true; }
      ];
    };
  };
}
```

**3. Install it.**

```sh
sudo nixos-rebuild switch --flake /etc/nixos#nixos
```

Use this command from now on whenever you rebuild your system.

</details>

</details>

<details>
<summary><b>Flatpak</b> (optional, any system except NixOS)</summary>

The app isn't in the COSMIC Store yet. A flatpak version is being submitted,
and until it's accepted, the packages above are the recommended install. To
try it anyway:

1. Download `cosmic-ext-nightlight-*.flatpak` from the
   [Releases](https://github.com/cosmic-nightlight/cosmic-nightlight/releases)
   page.
2. `flatpak install --user ./cosmic-ext-nightlight-*.flatpak`
3. Add the applet as in step 2 below, or open the settings with
   `flatpak run io.github.cosmic_ext_nightlight --settings`.

If you have the flatpak from 0.6, this one installs beside it. Run the command
in step 3 once. That moves your settings and panel icon over and removes the old
version for you.

The first time you turn it on, it asks for your password once to install a small
helper on your system. A sandboxed app can't change screen colors on its own
([why](docs/flatpak-design.md)).

To remove it, uninstall it from the COSMIC Store or run
`flatpak uninstall io.github.cosmic_ext_nightlight`. The helper removes itself a few
seconds later.

If you set up the helper with version 0.5.1 or earlier, it stays behind. Remove it with:

```sh
sudo rm /usr/local/bin/cosmic-nightlight-helper /etc/polkit-1/rules.d/49-cosmic-nightlight.rules
```

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

flatpak-builder --force-clean --user --install build io.github.cosmic_ext_nightlight.json
flatpak run io.github.cosmic_ext_nightlight --settings
```

This builds the latest release. To build your local changes instead, run
`python3 generate-local-manifest.py` and build the manifest it creates. See
[flatpak/README.md](flatpak/README.md).

</details>

</details>

<details>
<summary><b>Build from source</b> (for development)</summary>

You'll need a Rust toolchain and `libdrm-dev`:

```sh
./scripts/install.sh --gui     # build and install everything
./scripts/uninstall.sh         # remove it all again
```

To build the `.deb`, `.rpm`, Arch or Nix package, see [PACKAGING.md](PACKAGING.md). For packagers:
[docs/flatpak-design.md](docs/flatpak-design.md) explains the flatpak setup, and
[docs/cosmic-store.md](docs/cosmic-store.md) covers the COSMIC Store listing.

</details>

**2. Add the applet:** **COSMIC Settings → Desktop → Panel** (or **Dock**)
**→ Configure applets → Add applet → Night Light**.

Click the Night Light icon to turn it on or open its settings.

## Using it

**The applet.** Click the icon for the on/off toggle, the warmth slider, and a
**Night Light Settings…** button.

**Settings.** Choose a schedule, the night warmth, and the brightness (which dims
the screen while the night light is on). Open it from the applet, from your app
launcher, or with `cosmic-ext-nightlight --settings`.

**Keyboard shortcut.** Click **Add** next to **Keyboard Shortcut** in Settings,
and Super+Shift+N turns Night Light on and off. (If that key is taken, it picks
another and shows you which.) Change or remove it in COSMIC Settings, under
Keyboard, like any other shortcut.

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
systemctl --user enable --now cosmic-ext-nightlight.service
```

(From a source install, copy `systemd/cosmic-ext-nightlight.service` into
`~/.config/systemd/user/` first.)

<details>
<summary>Advanced: run the helper directly</summary>

```sh
pkexec /usr/bin/cosmic-ext-nightlight-helper --temp 3500            # warm tint
pkexec /usr/bin/cosmic-ext-nightlight-helper --temp 4000 --brightness 0.9
pkexec /usr/bin/cosmic-ext-nightlight-helper --off                 # reset
```

(Use `/usr/local/bin/...` if you installed with `scripts/install.sh`.)
</details>

## Translating

The app is English-only for now, but it's ready for translations. A translation
is one file, a translated copy of
[`crates/cosmic-ext-nightlight/i18n/en/cosmic_nightlight.ftl`](crates/cosmic-ext-nightlight/i18n/en/cosmic_nightlight.ftl). No coding needed, and partial translations are
welcome. See [docs/translating.md](docs/translating.md).

## Known limitations

- **On some systems the screen flickers briefly** (1–2 seconds) when the tint
  changes. Most systems change without it.
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
planned for COSMIC Epoch 3 ([cosmic-comp#2059], [cosmic-epoch#2498]).

So this app skips Wayland and writes the color curve straight to the kernel's
graphics layer (DRM), the same way `redshift` does outside a desktop. COSMIC
holds an exclusive lock on the display (DRM master), so other programs can't
normally change it.

To get around that, a root helper borrows a copy of COSMIC's own handle to the
display and writes the colors through it. The kernel accepts that as coming
from COSMIC, and nothing on screen is interrupted.

If that doesn't work on your system, the helper falls back to the older way,
from [jjo/drm-colortemp]: briefly switch to another virtual terminal, which
makes COSMIC release the lock, write the colors, and switch back. That switch
shows as a brief flicker.

## Architecture

Three crates, so the part that runs as root stays small and separate from the
GUI:

| Crate | Runs as | Does |
| --- | --- | --- |
| [`nightlight-core`](crates/nightlight-core) | library | Color math ([`gamma.rs`](crates/nightlight-core/src/gamma.rs)), DRM writes ([`drm.rs`](crates/nightlight-core/src/drm.rs)), the VT switching fallback ([`vt.rs`](crates/nightlight-core/src/vt.rs)) |
| [`nightlight-helper`](crates/nightlight-helper) | root (via `pkexec`) | Small CLI: reads `--temp`/`--brightness` and calls core |
| [`cosmic-ext-nightlight`](crates/cosmic-ext-nightlight) | your user | The panel applet, `--settings` window and optional `--daemon`; calls the helper |

The applet, the settings window and the daemon can each run the schedule. They
share a lock and a record of what's on screen in `$XDG_RUNTIME_DIR`, so running
several at once still causes only one change.

What happens on a tint change:

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

The color curve uses Tanner Helland's color temperature formula. 6500 K means no
tint, and lower values are warmer.

## Long term

This is a stopgap until COSMIC supports a color control protocol. Follow
[cosmic-comp#764] and [cosmic-comp#2059]. Once it does, this app can drop the
workaround and become a normal Wayland app.

[cosmic-comp#764]: https://github.com/pop-os/cosmic-comp/issues/764
[cosmic-comp#2059]: https://github.com/pop-os/cosmic-comp/issues/2059
[cosmic-epoch#2498]: https://github.com/pop-os/cosmic-epoch/issues/2498
[jjo/drm-colortemp]: https://github.com/jjo/drm-colortemp
