{ package }:
{
  config,
  lib,
  pkgs,
  utils,
  ...
}:
let
  cfg = config.services.fabrico11y;
  receiverEnabled = cfg.serverConfigFile != null;
  spindleSocket = "%t/spindle/spindle.sock";
  escape = utils.escapeSystemdExecArgs;
  validExternalPath =
    value:
    value != null
    && !builtins.hasContext value
    && value != "/"
    && lib.hasPrefix "/" value
    && !lib.hasInfix "\n" value
    && !lib.hasInfix "\r" value
    && !lib.any (part: part == "." || part == "..") (lib.splitString "/" value)
    && !lib.any (root: value == root || lib.hasPrefix "${root}/" value) [
      "/tmp"
      "/dev/shm"
      "/run/shm"
      "/nix/store"
    ];
  externalPath =
    description:
    lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      inherit description;
    };

  # Null options are rejected by assertions before units can be installed.
  user = if cfg.user == null then "" else cfg.user;
  path = value: if value == null then "" else value;
  bin = "${cfg.package}/bin";
  service = {
    wantedBy = [ "default.target" ];
    unitConfig.ConditionUser = user;
    environment.TMPDIR = path cfg.scratchDir;
    serviceConfig = {
      Slice = "fabrico11y.slice";
      KillMode = "control-group";
      NoNewPrivileges = true;
      UMask = "0077";
      Restart = "on-failure";
      RestartSec = 2;
      TimeoutStopSec = 10;
    };
  };
  notify = {
    Type = "notify";
    NotifyAccess = "main";
  };
  localReceiver = lib.optional receiverEnabled "fabric-server.service";
in
{
  options.services.fabrico11y = {
    enable = lib.mkEnableOption "the FabricO11y user services";
    package = lib.mkOption {
      type = lib.types.package;
      default = package;
      description = "The FabricO11y package used by the services.";
    };
    user = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = "Existing regular user whose systemd manager runs the services.";
    };
    dataDir = externalPath "Absolute writable directory for the collector state on disk.";
    scratchDir = externalPath "Absolute writable scratch directory on disk.";
    spindleConfigFile = externalPath ''
      Absolute path to the operator-managed Spindle configuration. Its socket
      must be /run/user/<uid>/spindle/spindle.sock for the selected user.
    '';
    serverConfigFile = externalPath ''
      Absolute path to the operator-managed Fabric Server configuration.
      Setting it enables the canonical receiver. Credential files referenced
      by the configuration must stay outside the Nix store.
    '';
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = pkgs.stdenv.hostPlatform.system == "x86_64-linux";
        message = "The FabricO11y NixOS module supports x86_64-linux.";
      }
      {
        assertion = user != "" && (config.users.users.${user}.isNormalUser or false);
        message = "services.fabrico11y.user must name an existing regular user.";
      }
      {
        assertion = lib.all validExternalPath [
          cfg.dataDir
          cfg.scratchDir
          cfg.spindleConfigFile
        ];
        message = "FabricO11y requires absolute external dataDir, scratchDir and spindleConfigFile paths outside temporary filesystems and the Nix store.";
      }
      {
        assertion = !receiverEnabled || validExternalPath cfg.serverConfigFile;
        message = "FabricO11y serverConfigFile must be an absolute external path outside temporary filesystems and the Nix store.";
      }
    ];

    environment.systemPackages = [ cfg.package ];
    systemd.user.units."fabrico11y.slice".text = builtins.readFile ../../config/fabrico11y.slice;

    systemd.user.services = {
      spindle = service // {
        description = "Spindle local observation custody";
        after = localReceiver;
        wants = localReceiver;
        serviceConfig = service.serviceConfig // notify // {
          ExecStart = escape [ "${bin}/spindle" "serve" (path cfg.spindleConfigFile) ];
          ExecStartPost = "${escape [ "${bin}/spindle" "status" ]} ${spindleSocket}";
          ExecStop = "${escape [ "${bin}/spindle" "stop" ]} ${spindleSocket}";
          RuntimeDirectory = "spindle";
          RuntimeDirectoryMode = "0700";
        };
      };

      fabric-daemon = service // {
        description = "FabricO11y host and agent telemetry collector";
        after = [ "spindle.service" ];
        requires = [ "spindle.service" ];
        serviceConfig = service.serviceConfig // {
          ExecStart =
            escape [
              "${bin}/fabric"
              "--data-dir"
              (path cfg.dataDir)
              "--tmp-dir"
              (path cfg.scratchDir)
              "daemon"
              "--spindle-socket"
            ]
            + " ${spindleSocket}";
          RuntimeDirectory = "fabrico11y";
          RuntimeDirectoryMode = "0700";
        };
      };
    }
    // lib.optionalAttrs receiverEnabled {
      fabric-server = service // {
        description = "FabricO11y canonical custody and projection";
        serviceConfig = service.serviceConfig // notify // {
          ExecStart = escape [ "${bin}/fabric-server" "serve" (path cfg.serverConfigFile) ];
        };
      };
    };
  };
}
