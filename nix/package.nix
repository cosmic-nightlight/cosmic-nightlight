# SPDX-License-Identifier: MPL-2.0
#
# The Nix package for cosmic-nightlight. flake.nix builds it; see PACKAGING.md.
#
# It installs the same files as the .deb, with one difference: the helper goes in
# libexec rather than bin. It runs as root through pkexec, so it stays off PATH
# and out of the libcosmic wrapper, and the app finds it there by looking beside
# its own binary. Changing the screen without a password also needs the polkit
# rule that nix/module.nix writes, since that rule has to name the store path.
#
# The helper is built on its own, without libcosmicAppHook. The hook links the
# graphics libraries libcosmic loads at runtime into every binary it builds, and
# a program that runs as root should load nothing it does not use.
{
  lib,
  rustPlatform,
  libcosmicAppHook,
  pkg-config,
  libxkbcommon,
  wayland,
  fontconfig,
  expat,
  freetype,
}:

let
  # The version comes from Cargo.toml, so this file is not one more place to
  # bump it in.
  cargoToml = lib.importTOML ../Cargo.toml;

  common = {
    inherit (cargoToml.workspace.package) version;

    src = lib.fileset.toSource {
      root = ../.;
      fileset = lib.fileset.unions [
        ../Cargo.toml
        ../Cargo.lock
        ../crates
        ../data
        ../debian/cosmic-nightlight.user.service
      ];
    };

    cargoLock = {
      lockFile = ../Cargo.lock;
      # libcosmic and a few of its dependencies come from git. Fetching them by
      # the revisions in Cargo.lock saves keeping a hash for each here, which
      # would have to change with every lock file update.
      allowBuiltinFetchGit = true;
    };
  };

  helper = rustPlatform.buildRustPackage (
    common
    // {
      pname = "cosmic-nightlight-helper";
      cargoBuildFlags = [
        "-p"
        "nightlight-helper"
      ];
      # The same tests the other packages run.
      cargoTestFlags = [
        "-p"
        "nightlight-core"
      ];
    }
  );
in
rustPlatform.buildRustPackage (
  common
  // {
    pname = "cosmic-nightlight";

    nativeBuildInputs = [
      libcosmicAppHook
      pkg-config
    ];

    buildInputs = [
      libxkbcommon
      wayland
      fontconfig
      expat
      freetype
    ];

    cargoBuildFlags = [
      "-p"
      "cosmic-nightlight"
    ];
    # The helper's build runs the tests.
    doCheck = false;

    postInstall = ''
      install -Dm0755 ${helper}/bin/cosmic-nightlight-helper -t $out/libexec

      install -Dm0644 data/io.github.cosmic_nightlight.desktop \
        -t $out/share/applications
      install -Dm0644 data/io.github.cosmic_nightlight.settings.desktop \
        -t $out/share/applications
      install -Dm0644 data/io.github.cosmic_nightlight.metainfo.xml \
        -t $out/share/metainfo
      install -Dm0644 data/icons/hicolor/scalable/apps/io.github.cosmic_nightlight.svg \
        -t $out/share/icons/hicolor/scalable/apps
      install -Dm0644 data/icons/hicolor/128x128/apps/io.github.cosmic_nightlight.png \
        -t $out/share/icons/hicolor/128x128/apps

      # Shipped but not enabled, as in the other packages. The applet keeps to the
      # schedule on its own, so the daemon is only for when it is off the panel.
      install -Dm0644 debian/cosmic-nightlight.user.service \
        $out/lib/systemd/user/cosmic-nightlight.service
      substituteInPlace $out/lib/systemd/user/cosmic-nightlight.service \
        --replace-fail /usr/bin/cosmic-nightlight $out/bin/cosmic-nightlight
    '';

    # The hook would wrap everything in bin and libexec. Wrap only the app, so the
    # helper that runs as root is the plain binary the polkit rule names.
    dontWrapLibcosmicApp = true;
    postFixup = ''
      wrapLibcosmicApp $out/bin/cosmic-nightlight
    '';

    meta = {
      description = "Night light and screen tint for the COSMIC desktop";
      homepage = "https://github.com/cosmic-nightlight/cosmic-nightlight";
      license = lib.licenses.mpl20;
      platforms = lib.platforms.linux;
      mainProgram = "cosmic-nightlight";
    };
  }
)
