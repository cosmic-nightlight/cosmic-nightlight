# SPDX-License-Identifier: MPL-2.0
#
# RPM packaging for cosmic-ext-nightlight.
#
# .github/workflows/build-rpm.yml builds this inside a Fedora container. To build
# it by hand on Fedora, rpmbuild wants a source archive rather than a working
# tree, so make one first:
#
#   version=$(rpmspec -q --srpm --qf '%{version}' rpm/cosmic-ext-nightlight.spec)
#   mkdir -p ~/rpmbuild/SOURCES
#   git archive --prefix="cosmic-ext-nightlight-$version/" \
#       -o ~/rpmbuild/SOURCES/"cosmic-ext-nightlight-$version.tar.gz" HEAD
#   rpmbuild -bb rpm/cosmic-ext-nightlight.spec
#
# The version below is one of the places the release version is written. Keep it
# in step with Cargo.toml, debian/changelog and the metainfo;
# scripts/check-version.sh checks all of them at once.

# We ship binaries cargo built, so rpmbuild has no compilation of its own to mine
# symbols out of and would abort on the empty debuginfo package it finds.
%global debug_package %{nil}

Name:           cosmic-ext-nightlight
Version:        0.7.0
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

# The name before 0.7.0. Installing this replaces it.
Obsoletes:      cosmic-nightlight < 0.7.0
Provides:       cosmic-nightlight = %{version}-%{release}

%description
Night Light warms your screen's colors at night to cut blue light. It's an
applet for the COSMIC desktop that sits on your panel or dock.

It can follow sunset and sunrise where you are, or a schedule you set, and can
dim the screen too.

This is an unofficial community project. It isn't made or endorsed by System76.

%prep
%autosetup

%build
cargo build --release --locked -p nightlight-helper -p cosmic-ext-nightlight

%check
cargo test --release --locked -p nightlight-core

%install
install -D -m0755 target/release/cosmic-ext-nightlight-helper \
    %{buildroot}%{_bindir}/cosmic-ext-nightlight-helper
# The helper's name before 0.7.0. An applet started before the upgrade keeps
# calling it until the next login, and would fail without it.
ln -s cosmic-ext-nightlight-helper %{buildroot}%{_bindir}/cosmic-nightlight-helper
install -D -m0755 target/release/cosmic-ext-nightlight \
    %{buildroot}%{_bindir}/cosmic-ext-nightlight
# The app's name before 0.7.0. The panel restarts an applet with the command it
# first ran, so the applet %%posttrans restarts comes back through this link as
# the new version.
ln -s cosmic-ext-nightlight %{buildroot}%{_bindir}/cosmic-nightlight

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
install -D -m0644 polkit/49-cosmic-ext-nightlight.rules \
    %{buildroot}%{_datadir}/polkit-1/rules.d/49-cosmic-ext-nightlight.rules

# Ship the user service but leave it disabled, exactly as the .deb does. The
# applet keeps to the schedule on its own, so the daemon only covers the case of
# the applet not being on the panel; enabling it for every user would run a second
# scheduler at every login for no gain. Anyone who wants one can run
# `systemctl --user enable --now cosmic-ext-nightlight.service`.
# Taken from debian/ rather than systemd/, which points ExecStart at
# /usr/local/bin and so names the wrong path for a packaged build.
install -D -m0644 debian/cosmic-ext-nightlight.user.service \
    %{buildroot}%{_userunitdir}/cosmic-ext-nightlight.service
%{_userunitdir}/cosmic-nightlight.service
# The unit's name before 0.7.0, so a service enabled under it still starts.
# The app moves the enablement over to the new name the first time it runs.
ln -s cosmic-ext-nightlight.service %{buildroot}%{_userunitdir}/cosmic-nightlight.service

# $1 is how many copies of the package remain: 0 on a removal, 1 on an upgrade.
%preun
if [ "$1" -eq 0 ]; then
    # Put the screen's colors back while the helper is still here to do it. The
    # helper does nothing if the screen is already clear.
    %{_bindir}/cosmic-ext-nightlight-helper --off >/dev/null 2>&1 || :
fi

%postun
if [ "$1" -eq 0 ]; then
    # Each session's note of what it last put on the screen is wrong now that
    # the tint is cleared. A reinstall in the same session would trust it.
    rm -f /run/user/*/cosmic-nightlight-shown
fi

%posttrans
# Restart panel applets still running a binary this upgrade replaced, so the
# new version shows up now instead of at the next login. The panel starts the
# applet with no arguments and brings it back within milliseconds. The settings
# window and the service have arguments and are left alone.
for exe in /proc/[0-9]*/exe; do
    case "$(readlink "$exe" 2>/dev/null)" in
        "%{_bindir}/cosmic-ext-nightlight (deleted)" | "%{_bindir}/cosmic-nightlight (deleted)") ;;
        *) continue ;;
    esac
    dir=${exe%%/exe}
    [ "$(tr -cd '\0' <"$dir/cmdline" 2>/dev/null | wc -c)" -eq 1 ] || continue
    kill "${dir#/proc/}" 2>/dev/null || :
done

%files
%license LICENSE
%doc README.md
%{_bindir}/cosmic-ext-nightlight
%{_bindir}/cosmic-ext-nightlight-helper
%{_bindir}/cosmic-nightlight
%{_bindir}/cosmic-nightlight-helper
%{_datadir}/applications/io.github.cosmic_nightlight.desktop
%{_datadir}/applications/io.github.cosmic_nightlight.settings.desktop
%{_datadir}/metainfo/io.github.cosmic_nightlight.metainfo.xml
%{_datadir}/icons/hicolor/scalable/apps/io.github.cosmic_nightlight.svg
%{_datadir}/icons/hicolor/128x128/apps/io.github.cosmic_nightlight.png
%{_datadir}/polkit-1/rules.d/49-cosmic-ext-nightlight.rules
%{_userunitdir}/cosmic-ext-nightlight.service

%changelog
* Wed Nov 11 2026 Daniel Watts <danielcwtts@gmail.com> - 0.7.0-1
- Update to 0.7.0.

* Mon Nov 09 2026 Daniel Watts <danielcwtts@gmail.com> - 0.6.1-1
- Update to 0.6.1.

* Fri Oct 23 2026 Daniel Watts <danielcwtts@gmail.com> - 0.6.0-1
- Update to 0.6.0.

* Thu Oct 15 2026 abu-matterize <abu@matterize.io> - 0.5.1-1
- Initial RPM packaging.
