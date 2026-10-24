# SPDX-License-Identifier: MPL-2.0
#
# NixOS module for cosmic-nightlight. flake.nix exports it as
# nixosModules.default; see the README for how to use it.
#
# The package alone is not enough on NixOS. The polkit rule the other packages
# ship names /usr/bin, and here the helper lives in the Nix store, so this module
# writes a rule naming the helper's store path instead. Without it every change
# to the screen asks for a password.
{ self }:
{
  config,
  options,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.programs.cosmic-nightlight;
in
{
  options.programs.cosmic-nightlight = {
    enable = lib.mkEnableOption "Night Light for the COSMIC desktop";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      defaultText = lib.literalExpression "cosmic-nightlight.packages.\${system}.default";
      description = "The cosmic-nightlight package to use.";
    };
  };

  config = lib.mkIf cfg.enable (
    lib.mkMerge [
      {
        environment.systemPackages = [ cfg.package ];

        # The user service, left disabled like the other packages leave it.
        systemd.packages = [ cfg.package ];

        # The same rule as polkit/49-cosmic-nightlight.rules: members of wheel run the
        # helper without a password. pkexec comes from polkit.
        security.polkit.enable = true;
        security.polkit.extraConfig = ''
          polkit.addRule(function (action, subject) {
              if (
                  action.id == "org.freedesktop.policykit.exec" &&
                  action.lookup("program") == "${cfg.package}/libexec/cosmic-nightlight-helper" &&
                  subject.isInGroup("wheel")
              ) {
                  return polkit.Result.YES;
              }
          });
        '';
      }

      # The app runs the helper through pkexec, which only works setuid. Newer
      # NixOS leaves that wrapper out unless asked for; older releases always
      # install it and have no option for it.
      (lib.optionalAttrs (options.security.polkit ? enablePkexecWrapper) {
        security.polkit.enablePkexecWrapper = true;
      })
    ]
  );
}
