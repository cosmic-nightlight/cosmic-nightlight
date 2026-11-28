# SPDX-License-Identifier: MPL-2.0
{
  description = "Night light and screen tint for the COSMIC desktop";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAllSystems (pkgs: {
        default = pkgs.callPackage ./nix/package.nix { };
      });

      # Built against this flake's nixpkgs, as the module's package is. libcosmic
      # needs a newer Rust than a stable NixOS release can have, so building
      # against the user's own nixpkgs failed there.
      overlays.default = final: prev: {
        cosmic-ext-nightlight = self.packages.${final.stdenv.hostPlatform.system}.default;
        # The name before 0.7.0, so overlays that use it keep working.
        cosmic-nightlight = final.cosmic-ext-nightlight;
      };

      nixosModules.default = import ./nix/module.nix { inherit self; };

      checks = forAllSystems (pkgs: {
        module = import ./nix/test.nix { inherit self pkgs; };
      });

      formatter = forAllSystems (pkgs: pkgs.nixfmt);
    };
}
