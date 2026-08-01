<!-- SPDX-License-Identifier: MPL-2.0 -->
# Flatpak packaging

The manifest here is the one submitted to
[pop-os/cosmic-flatpak](https://github.com/pop-os/cosmic-flatpak), which is what
puts the applet in the COSMIC Store. It is kept in this repo so it stays in step
with the code; submitting means copying it plus `cargo-sources.json` to
`app/io.github.cosmic_nightlight/` over there.

## Why the sandbox needs a host helper

Writing gamma LUTs needs DRM master, which needs `CAP_SYS_ADMIN`. No flatpak
permission grants that, so the sandbox cannot tint the screen however it is
configured — note that the manifest asks for no device access at all. Instead the
applet reaches the host through `flatpak-spawn --host pkexec`, and the privileged
helper runs there.

That means the helper has to run **on the host**. It ships at
`/app/libexec/cosmic-nightlight-helper` and is used two ways: as the payload for
a one-time, user-triggered setup that copies it to
`/usr/local/bin/cosmic-nightlight-helper` and installs the polkit rule beside it,
and — before that setup has ever been run — directly, via the host-side path
`/.flatpak-info` reports as `app-path`. Both `/usr/bin` and `/usr/local/bin` are
whitelisted by `polkit/49-cosmic-nightlight.rules`, so nothing about the rule
changes between the `.deb` and the flatpak.

Until the setup has been run the app still works — pkexec just prompts for a
password on every schedule transition instead of none, because no rule names the
path inside the flatpak. That is the whole of what the setup buys.

## Generating cargo-sources.json

Builds run offline, so every crate has to be declared up front. `Cargo.lock` is
committed, and the one git dependency (libcosmic) is handled by the generator.

```bash
python3 -m venv venv
./venv/bin/pip install aiohttp toml tomlkit
curl -sSLO https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/master/cargo/flatpak-cargo-generator.py
./venv/bin/python flatpak-cargo-generator.py ../Cargo.lock -o cargo-sources.json
```

Regenerate whenever a *dependency* changes. `Cargo.lock` also records our own
crates' version numbers, and a release bump rewrites those three lines without
affecting the file — it only ever describes external crates. Check what actually
moved before spending the download:

```bash
git diff <last-release>..HEAD -- ../Cargo.lock | grep -E '^[+-]name'
```

Empty output means the existing `cargo-sources.json` is still good. The output is
~450KB and is not committed here; cosmic-flatpak carries its own copy.

## Building locally

```bash
sudo apt-get install flatpak-builder just
flatpak install flathub com.system76.Cosmic.BaseApp org.freedesktop.Sdk//25.08 \
    org.freedesktop.Sdk.Extension.rust-stable//25.08
flatpak-builder --force-clean --user --install build io.github.cosmic_nightlight.json
```

To build the working tree rather than the release, swap the `git` source for
`{"type": "dir", "path": ".."}` — or use `io.github.cosmic_nightlight.local.json`,
which is that same manifest with the swap already made and is gitignored.
cosmic-flatpak needs the `git` source, so remember not to commit the swap.

## Re-pinning for a release

The `git` source carries both a `tag` and a `commit`, and **the commit is the one
the tag resolves to, not the one that changes the manifest** — the pin cannot
name the commit it is part of. So the order is: tag the release, then re-pin
here, then copy to cosmic-flatpak.

```bash
git rev-parse v0.5.0^{}   # the commit to put in "commit"
```

Leaving a stale pin is quiet and expensive: the build succeeds and ships the
*previous* release under the new version's metadata.
