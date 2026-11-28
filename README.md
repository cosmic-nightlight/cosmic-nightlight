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
at night to cut blue light. It can follow your local sunset and sunrise or a
schedule you set, and it can also dim the screen when the night light turns on.

COSMIC won't have a built-in night light until Epoch 3. The usual tools
(`redshift`, `gammastep`, `wlsunset`) don't work on it either. See
[How it works](#how-it-works).

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

On Fedora Atomic, use the Flatpak below instead.

</details>

<details>
<summary><b>Arch Linux</b></summary>

Download the latest `cosmic-ext-nightlight-*.pkg.tar.zst` from the
[Releases](https://github.com/cosmic-nightlight/cosmic-nightlight/releases)
page and run:

```sh
sudo pacman -U ./cosmic-ext-nightlight-*.pkg.tar.zst
```

If you have an older version installed, type `y` when pacman asks to remove
`cosmic-nightlight`.

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

**Updating to a new release:** change `v0.x.x` in the `cosmic-ext-nightlight.url`
line to the new version, then run
`sudo nix flake update cosmic-ext-nightlight --flake /etc/nixos` and rebuild.

**Set up before 0.7.0?** Do the same with the `cosmic-nightlight.url` line, then
run `sudo nix flake update cosmic-nightlight --flake /etc/nixos` and rebuild.

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

Download the latest `cosmic-ext-nightlight-*.flatpak` from the
[Releases](https://github.com/cosmic-nightlight/cosmic-nightlight/releases)
page and run:

```sh
flatpak install --user ./cosmic-ext-nightlight-*.flatpak
```

Open **Night Light Settings** from your app launcher, turn it on, and type your
password once to install a small helper.

To remove it, run `flatpak uninstall io.github.cosmic_ext_nightlight`. This also
removes the helper.

### Upgrading

If you used the flatpak from 0.5.1 or earlier, remove its old helper:

```sh
sudo rm /usr/local/bin/cosmic-nightlight-helper /etc/polkit-1/rules.d/49-cosmic-nightlight.rules
```

If you have the flatpak from 0.6, open Night Light Settings once after installing
the new version. It moves your settings and panel icon over. The first time you
turn Night Light on or off, it asks for your password and removes the old version.

</details>

<details>
<summary><b>Build from source</b> (for development)</summary>

You'll need a Rust toolchain and these packages (Debian and Ubuntu names):

```sh
sudo apt install build-essential pkg-config libdrm-dev libxkbcommon-dev \
    libwayland-dev libfontconfig-dev libexpat1-dev
```

Then run:

```sh
./scripts/install.sh --gui     # build and install everything
./scripts/uninstall.sh         # remove it all again
```

To build the `.deb`, `.rpm`, Arch or Nix package, see [PACKAGING.md](PACKAGING.md). For packagers:
[docs/flatpak-design.md](docs/flatpak-design.md) explains the flatpak setup, and
[docs/cosmic-store.md](docs/cosmic-store.md) covers the COSMIC Store listing.

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
./venv/bin/pip install aiohttp==3.14.5 tomlkit==0.15.2
curl -sSLO https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/41c20aa10819cdb2a4f3ca171758a96d1955c018/cargo/flatpak-cargo-generator.py
./venv/bin/python flatpak-cargo-generator.py ../Cargo.lock -o cargo-sources.json

flatpak-builder --force-clean --user --install build io.github.cosmic_ext_nightlight.json
flatpak run io.github.cosmic_ext_nightlight
```

This builds the latest release. To build your local changes instead, run
`python3 generate-local-manifest.py` and build the manifest it creates. See
[flatpak/README.md](flatpak/README.md).

</details>

</details>

**2. Add the applet:** **COSMIC Settings → Desktop → Panel** (or **Dock**)
**→ Configure applets → Add applet → Night Light**.

Click the Night Light icon to turn it on or open its settings.

## Using it

Click the applet to turn Night Light on or off and set the warmth.

**Night Light Settings…** opens from the applet or your app launcher.

There you can lower the brightness while the tint is on, and add a keyboard
shortcut (defaults to Super+Shift+N).

You can also pick a schedule:

- **Sunset to Sunrise** follows the sun. It works out where you are from your
  time zone, so it never asks for your location.
- **Custom Schedule** turns it on and off at times you pick.
- **Off** stays manual.

If you switch it on or off by hand, the schedule takes over again at its next
change.

<details>
<summary>Running the schedule without the applet</summary>

The applet runs your schedule while it's on your panel or dock. If you remove it, turn on **Run in Background** in Settings
(it appears once the applet is gone), or use the systemd service:

```sh
systemctl --user enable --now cosmic-ext-nightlight.service
```

(From a source install, copy `systemd/cosmic-ext-nightlight.service` into
`~/.config/systemd/user/` first.)
</details>

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
is one file, and no coding is needed. Partial translations are welcome. See
[docs/translating.md](docs/translating.md).

## Known limitations

- On some systems the screen flickers briefly (1–2 seconds) when the tint
  changes. Most systems change without it.
- A monitor you plug in can take up to a minute to get the tint.
- Sunset to Sunrise can be off by a few minutes, since it goes by your time
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

If that doesn't work on your system, the helper falls back to the method used
by [jjo/drm-colortemp]. It switches to another virtual terminal so COSMIC
releases the lock, writes the colors, then switches back. You may see a brief
flicker when this happens.

The color curve uses Tanner Helland's color temperature formula. For how the
code is laid out, see [docs/architecture.md](docs/architecture.md).

[cosmic-comp#764]: https://github.com/pop-os/cosmic-comp/issues/764
[cosmic-comp#2059]: https://github.com/pop-os/cosmic-comp/issues/2059
[cosmic-epoch#2498]: https://github.com/pop-os/cosmic-epoch/issues/2498
[jjo/drm-colortemp]: https://github.com/jjo/drm-colortemp
