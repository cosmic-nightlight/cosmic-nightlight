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

That means the helper has to be **on the host**, which a flatpak cannot install
by itself. It ships at `/app/libexec/cosmic-nightlight-helper` purely as the
payload for a one-time, user-triggered setup that copies it to
`/usr/local/bin/cosmic-nightlight-helper` and installs the polkit rule beside it.
Both of those paths are already whitelisted by
`polkit/49-cosmic-nightlight.rules`, so nothing about the rule changes between
the `.deb` and the flatpak.

Until that setup has been run, the app still works — pkexec just prompts for a
password on every schedule transition instead of none.

## Generating cargo-sources.json

Builds run offline, so every crate has to be declared up front. `Cargo.lock` is
committed, and the one git dependency (libcosmic) is handled by the generator.

```bash
python3 -m venv venv
./venv/bin/pip install aiohttp toml tomlkit
curl -sSLO https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/master/cargo/flatpak-cargo-generator.py
./venv/bin/python flatpak-cargo-generator.py ../Cargo.lock -o cargo-sources.json
```

Regenerate whenever `Cargo.lock` changes. The output is ~450KB and is not
committed here; cosmic-flatpak carries its own copy.

## Building locally

```bash
sudo apt-get install flatpak-builder just
flatpak install flathub com.system76.Cosmic.BaseApp org.freedesktop.Sdk//25.08 \
    org.freedesktop.Sdk.Extension.rust-stable//25.08
flatpak-builder --force-clean --user --install build io.github.cosmic_nightlight.json
```

To build the working tree rather than the pushed branch, swap the `git` source
for `{"type": "dir", "path": ".."}`. Remember to swap it back — cosmic-flatpak
needs the git source, pinned to a release tag rather than a branch.
