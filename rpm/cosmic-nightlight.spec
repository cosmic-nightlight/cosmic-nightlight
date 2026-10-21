# SPDX-License-Identifier: MPL-2.0
#
# RPM packaging for cosmic-nightlight.
#
# .github/workflows/build-rpm.yml builds this inside a Fedora container. To build
# it by hand on Fedora, rpmbuild wants a source archive rather than a working
# tree, so make one first:
#
#   version=$(rpmspec -q --srpm --qf '%{version}' rpm/cosmic-nightlight.spec)
#   mkdir -p ~/rpmbuild/SOURCES
#   git archive --prefix="cosmic-nightlight-$version/" \
#       -o ~/rpmbuild/SOURCES/"cosmic-nightlight-$version.tar.gz" HEAD
#   rpmbuild -bb rpm/cosmic-nightlight.spec
#
# The version below is one of the places the release version is written. Keep it
# in step with Cargo.toml, debian/changelog and the metainfo;
# scripts/check-version.sh checks all of them at once.

# We ship binaries cargo built, so rpmbuild has no compilation of its own to mine
# symbols out of and would abort on the empty debuginfo package it finds.
%global debug_package %{nil}

Name:           cosmic-nightlight
Version:        0.5.1
Release:        1%{?dist}
Summary:        Night light and screen tint for the COSMIC desktop

License:        MPL-2.0
URL:            https://github.com/cosmic-nightlight/cosmic-nightlight
Source0:        %{name}-%{version}.tar.gz

BuildRequires:  cargo
BuildRequires:  gcc
BuildRequires:  gcc-c++
BuildRequires:  pkgconf-pkg-config
BuildRequires:  libdrm-devel
BuildRequires:  libxkbcommon-devel
BuildRequires:  wayland-devel
BuildRequires:  fontconfig-devel
BuildRequires:  expat-devel
# %{_userunitdir} is not an rpm macro of its own; macros.systemd defines it, and
# without this the expansion is left literal and %files rejects the line.
BuildRequires:  systemd-rpm-macros

# The privileged helper is reached through pkexec, which polkit provides.
Requires:       polkit

%description
A night-light utility for the COSMIC desktop that warms the screen color
temperature on a schedule, either following sunset and sunrise or a schedule you
set.

COSMIC's compositor does not yet expose a gamma-control Wayland protocol, so
this writes the gamma ramp directly to the kernel's DRM/KMS layer. A small
privileged helper, run through polkit, briefly switches virtual terminals so the
compositor releases the DRM master lock, applies the gamma LUTs to every active
display, and switches back. The tint then persists.

%prep
%autosetup

%build
cargo build --release --locked -p nightlight-helper -p cosmic-nightlight

%check
cargo test --release --locked -p nightlight-core

%install
install -D -m0755 target/release/cosmic-nightlight-helper \
    %{buildroot}%{_bindir}/cosmic-nightlight-helper
install -D -m0755 target/release/cosmic-nightlight \
    %{buildroot}%{_bindir}/cosmic-nightlight

install -D -m0644 data/io.github.cosmic_nightlight.desktop \
    %{buildroot}%{_datadir}/applications/io.github.cosmic_nightlight.desktop
install -D -m0644 data/io.github.cosmic_nightlight.settings.desktop \
    %{buildroot}%{_datadir}/applications/io.github.cosmic_nightlight.settings.desktop
install -D -m0644 data/io.github.cosmic_nightlight.metainfo.xml \
    %{buildroot}%{_datadir}/metainfo/io.github.cosmic_nightlight.metainfo.xml
install -D -m0644 data/icons/hicolor/scalable/apps/io.github.cosmic_nightlight.svg \
    %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/io.github.cosmic_nightlight.svg
install -D -m0644 data/icons/hicolor/128x128/apps/io.github.cosmic_nightlight.png \
    %{buildroot}%{_datadir}/icons/hicolor/128x128/apps/io.github.cosmic_nightlight.png
install -D -m0644 polkit/49-cosmic-nightlight.rules \
    %{buildroot}%{_datadir}/polkit-1/rules.d/49-cosmic-nightlight.rules

# Ship the user service but leave it disabled, exactly as the .deb does. The
# applet keeps to the schedule on its own, so the daemon only covers the case of
# the applet not being on the panel; enabling it for every user would run a second
# scheduler at every login for no gain. Anyone who wants one can run
# `systemctl --user enable --now cosmic-nightlight.service`.
# Taken from debian/ rather than systemd/, which points ExecStart at
# /usr/local/bin and so names the wrong path for a packaged build.
install -D -m0644 debian/cosmic-nightlight.user.service \
    %{buildroot}%{_userunitdir}/cosmic-nightlight.service

%files
%license LICENSE
%doc README.md
%{_bindir}/cosmic-nightlight
%{_bindir}/cosmic-nightlight-helper
%{_datadir}/applications/io.github.cosmic_nightlight.desktop
%{_datadir}/applications/io.github.cosmic_nightlight.settings.desktop
%{_datadir}/metainfo/io.github.cosmic_nightlight.metainfo.xml
%{_datadir}/icons/hicolor/scalable/apps/io.github.cosmic_nightlight.svg
%{_datadir}/icons/hicolor/128x128/apps/io.github.cosmic_nightlight.png
%{_datadir}/polkit-1/rules.d/49-cosmic-nightlight.rules
%{_userunitdir}/cosmic-nightlight.service

%changelog
* Thu Oct 15 2026 abu-matterize <abu@matterize.io> - 0.5.1-1
- Initial RPM packaging.
