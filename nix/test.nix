# SPDX-License-Identifier: MPL-2.0
#
# Boots a NixOS VM with the module enabled and checks the polkit rule it writes:
# a wheel user runs the helper through pkexec with no password, and anyone else
# is turned down. flake.nix exposes it as checks.<system>.module, so
# `nix flake check` runs it.
#
# pkexec is given the helper's store path, as the app gives it. With no polkit
# agent in the VM, anything short of the rule saying yes fails, so a pass means
# the rule matched.
{ self, pkgs }:

let
  helper = "${
    self.packages.${pkgs.stdenv.hostPlatform.system}.default
  }/libexec/cosmic-nightlight-helper";
in
pkgs.testers.runNixOSTest {
  name = "cosmic-nightlight-module";

  nodes.machine = {
    imports = [ self.nixosModules.default ];
    programs.cosmic-nightlight.enable = true;

    users.users.admin = {
      isNormalUser = true;
      extraGroups = [ "wheel" ];
    };
    users.users.guest.isNormalUser = true;
  };

  testScript = ''
    machine.wait_for_unit("multi-user.target")

    # --version needs no screen, so it shows pkexec ran the helper as root.
    machine.succeed("su - admin -c 'pkexec ${helper} --version'")
    machine.fail("su - guest -c 'pkexec ${helper} --version'")

    # The user service points at the package, not /usr/bin.
    machine.succeed("grep -q 'ExecStart=/nix/store/.*/bin/cosmic-nightlight --daemon' /etc/systemd/user/cosmic-nightlight.service")
  '';
}
