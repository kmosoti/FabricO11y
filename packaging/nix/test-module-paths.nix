{
  lib,
  module,
  system,
  stateVersion,
}:
let
  base = lib.nixosSystem {
    inherit system;
    modules = [
      module
      {
        system.stateVersion = stateVersion;
        users.users.fabrictest.isNormalUser = true;
        services.fabrico11y = {
          enable = true;
          user = "fabrictest";
          dataDir = "/var/lib/fabrico11y/collector";
          scratchDir = "/var/lib/fabrico11y/scratch";
          spindleConfigFile = "/var/lib/fabrico11y/spindle.toml";
          serverConfigFile = "/var/lib/fabrico11y/server.toml";
        };
      }
    ];
  };
  passesAssertions =
    overrides:
    let
      tested = base.extendModules {
        modules = [ { services.fabrico11y = lib.mapAttrs (_: lib.mkForce) overrides; } ];
      };
    in
    !lib.any (
      assertion: !assertion.assertion && lib.hasPrefix "services.fabrico11y." assertion.message
    ) tested.config.assertions;
in
lib.assertMsg (passesAssertions { }) "FabricO11y rejected normalized external paths"
&& lib.all (
  overrides:
  lib.assertMsg (!passesAssertions overrides) "FabricO11y accepted invalid paths: ${builtins.toJSON overrides}"
) [
  { spindleConfigFile = "//nix/store/preexisting-config"; }
  { serverConfigFile = "/nix/store/preexisting-config"; }
  { dataDir = "/run/user/1000/fabrico11y-data"; }
  { scratchDir = "/var/run/fabrico11y-scratch"; }
  { scratchDir = "/var/lib//fabrico11y/scratch"; }
  { spindleConfigFile = "/var/lib/fabrico11y/../spindle.toml"; }
]
