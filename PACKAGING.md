<!-- SPDX-License-Identifier: MPL-2.0 -->
# Packaging cosmic-nightlight

This covers the `.deb`, which most of this file is about, the `.rpm` for
Fedora (see [RPM (Fedora)](#rpm-fedora)), the Arch Linux package (see
[Arch Linux](#arch-linux)) and the Nix flake (see [NixOS](#nixos)).

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
a `v0.2.0` Release. The release text comes from `docs/release-notes/v0.2.0.md`
if it exists, or is generated from the changelog if not. Write that file before
tagging, following [docs/release-notes/README.md](docs/release-notes/README.md).

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

## RPM (Fedora)

[`rpm/cosmic-nightlight.spec`](rpm/cosmic-nightlight.spec) builds the same set of
files as the `.deb`. [`.github/workflows/build-rpm.yml`](.github/workflows/build-rpm.yml)
builds it in a `fedora:44` container on every `v*` tag and attaches the `.rpm`
to the Release that the `.deb` workflow creates. It does not run on pushes or
pull requests; start it by hand from the Actions tab to test a spec change.

Because it links the container's glibc, the package installs on Fedora 44 and
newer only. To target another release, change the `container:` line.

The spec's `Version:` is one of the places the version is written, so bump it
with the others; `scripts/check-version.sh` checks it. The spec's header
comment shows how to build it by hand on Fedora.

## Arch Linux

[`arch/PKGBUILD`](arch/PKGBUILD) builds the same set of files as the `.deb`.
[`.github/workflows/build-arch.yml`](.github/workflows/build-arch.yml) builds it
in an `archlinux:latest` container on every `v*` tag. Unlike the `.deb` and
`.rpm`, the `.pkg.tar.zst` is not attached to the Release, to keep the Assets
list short; it is kept as a workflow artifact, and Arch users build their own
with `makepkg -si`. Like the `.rpm` workflow, it does not run on pushes or pull
requests; start it by hand from the Actions tab to test a PKGBUILD change.

On Arch itself, `makepkg -si` in `arch/` downloads the tagged release from
GitHub and builds that. To build the working tree instead, the PKGBUILD's header
comment shows how to hand makepkg an archive of it. CI does the same, which is
why `sha256sums` is `SKIP`. The PKGBUILD is also the starting point for an AUR
package, which would want a real checksum there.

The PKGBUILD's `pkgver=` is one of the places the version is written, so bump it
with the others; `scripts/check-version.sh` checks it.

## NixOS

[`flake.nix`](flake.nix) builds the package from
[`nix/package.nix`](nix/package.nix) and exports a NixOS module,
[`nix/module.nix`](nix/module.nix). The README shows how users add it.

The package installs the same files as the `.deb`, with one difference: the
helper goes in `libexec/`, not `bin/`. It runs as root, so it stays off `PATH`
and is not wrapped like the app is. The app finds it by looking in `../libexec`
beside its own binary.

The module is needed as well as the package. The shipped polkit rule names
`/usr/bin`, and on NixOS the helper is in the Nix store, so the module writes a
rule that names the helper's store path. Without it, every change asks for a
password. It also turns on `security.polkit.enablePkexecWrapper` where that
option exists: newer NixOS leaves out the setuid `pkexec` unless asked, and the
app can't reach the helper without it. The flatpak can't help here either: its host setup writes to
`/etc/polkit-1/rules.d`, which NixOS doesn't let anything write to.

The version comes from `Cargo.toml`, so there is nothing to bump. The git
dependencies are fetched by the revisions in `Cargo.lock`, so there are no
hashes to update either. `flake.lock` pins nixpkgs; run `nix flake update` now
and then to move it forward.

To build it on any system with Nix:

```sh
nix build            # the package, in ./result
nix flake check -L   # also boots a VM to test the module's polkit rule
```

[`.github/workflows/build-nix.yml`](.github/workflows/build-nix.yml) runs both
when the flake, `nix/` or the Cargo files change, and on every `v*` tag. Nothing
is attached to the Release.

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
