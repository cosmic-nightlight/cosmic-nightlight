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

      overlays.default = final: prev: {
        cosmic-ext-nightlight = final.callPackage ./nix/package.nix { };
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
