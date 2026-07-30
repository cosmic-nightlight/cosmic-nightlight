<!-- SPDX-License-Identifier: MPL-2.0 -->
# Packaging cosmic-nightlight as a `.deb`

This tool needs root (DRM master + VT switching), so its natural distribution
channel is a **native `.deb`**. A `.deb` shows up in the COSMIC Store as the
"System" version of the app once it's in a repo (a PPA or the Pop!_OS repos).

A flatpak sandbox still cannot get the capabilities the gamma workaround
requires — that has not changed — but the applet can be sandboxed anyway if the
privileged helper runs on the host instead. That is how it reaches the COSMIC
Store; see [docs/flatpak-design.md](docs/flatpak-design.md).

The `debian/` directory here produces a single binary package,
`cosmic-nightlight`, that installs:

| Path | What |
| --- | --- |
| `/usr/bin/cosmic-nightlight-helper` | privileged DRM/VT helper (run via pkexec) |
| `/usr/bin/cosmic-nightlight` | libcosmic GUI + `--daemon` scheduler |
| `/usr/share/polkit-1/rules.d/49-cosmic-nightlight.rules` | passwordless pkexec for `wheel`/`sudo` |
| `/usr/share/applications/io.github.cosmic_nightlight.desktop` | launcher entry |
| `/usr/lib/systemd/user/cosmic-nightlight.service` | per-user scheduler unit |

## Build dependencies

```sh
sudo apt install build-essential debhelper cargo rustc pkg-config \
    libdrm-dev libxkbcommon-dev libwayland-dev libfontconfig-dev libexpat1-dev
```

The GUI links libcosmic/wgpu; if the build fails on a missing `-dev` library,
install it and add it to `Build-Depends` in [`debian/control`](debian/control).

## Quick local build (network available)

libcosmic is a **git dependency**, so cargo must fetch it. On your own machine
(with network) this just works:

```sh
dpkg-buildpackage -b -us -uc
# -> ../cosmic-nightlight_0.1.0-1_amd64.deb
sudo apt install ../cosmic-nightlight_0.1.0-1_amd64.deb
```

## Automated builds & releases (GitHub Actions)

[`.github/workflows/build-deb.yml`](.github/workflows/build-deb.yml) builds the
`.deb` on an `ubuntu-24.04` (noble) runner — the same base this package targets:

- **Every push to `main` and every pull request** builds the `.deb` and uploads
  it as a workflow **artifact** (download it from the run's *Summary* page). This
  is the CI check that proves a change still packages.
- **Pushing a tag `v*`** builds the `.deb` and publishes a **GitHub Release**
  with the `.deb` attached, so users can grab it from the *Releases* page.

The runner has network, so cargo fetches the libcosmic git dependency directly —
no vendoring is needed (that's only for the offline path below).

### Cutting a release

The `.deb` version comes from `debian/changelog`, not the git tag, so bump it
first, then tag to match:

```sh
# 1. Add a new changelog entry (e.g. 0.2.0-1). dch is from the `devscripts` pkg.
dch -v 0.2.0-1 "Describe the changes"      # or edit debian/changelog by hand
git commit -am "Release 0.2.0-1"
git push

# 2. Tag it; the push triggers the release build.
git tag v0.2.0
git push --tags
```

The workflow then builds `cosmic-nightlight_0.2.0-1_amd64.deb` and attaches it to
a `v0.2.0` Release with auto-generated notes.

## Clean-room / offline build (PPA, sbuild, Launchpad)

Official build environments have **no network**, and cargo cannot fetch the
libcosmic git dependency there. Vendor the dependencies once and commit them:

```sh
mkdir -p .cargo
cargo vendor --locked vendor > .cargo/config.toml.fragment
# Merge the printed [source.*] stanzas into .cargo/config.toml, e.g.:
cat .cargo/config.toml.fragment >> .cargo/config.toml
git add vendor .cargo/config.toml Cargo.lock
```

With `vendor/` committed and `.cargo/config.toml` redirecting crates to it, the
`--locked` build in [`debian/rules`](debian/rules) runs fully offline.

> Note: `vendor/` is large. For a real upstream you'd typically keep it out of
> the main branch and generate it in the packaging branch / orig tarball
> instead.

## Getting it into the COSMIC Store

The Store draws from three catalogs, and only one of them takes submissions:

| Catalog | How you get in |
| --- | --- |
| System (DEP-11 from apt repos) | Requires Ubuntu universe or System76 packaging it. No submission process. |
| Flathub | Blocked on the app ID: three components where `io.github.*` needs four. |
| **COSMIC Flatpak** | **A normal pull request.** The target. |

So the route in is the flatpak, not the `.deb` — see
[docs/flatpak-design.md](docs/flatpak-design.md) for how a sandboxed build reaches
the privileged helper, and [docs/cosmic-store.md](docs/cosmic-store.md) for what
the Store reads off the metainfo once it is listed.

Hosting the `.deb` in an apt repository is still worth doing for people who want
a native package — a Launchpad PPA is the easiest (`debuild -S` then
`dput ppa:you/cosmic-nightlight`), or self-host with `reprepro`. Note that it
does **not** lead to a Store listing: PPAs publish no DEP-11, so the System
catalog cannot see them. Inclusion in the first-party Pop!_OS repos is a System76
decision, worth opening upstream only once the PPA is proven.
