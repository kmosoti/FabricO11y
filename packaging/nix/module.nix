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
    && lib.hasPrefix "/" value
    && !lib.hasInfix "\n" value
    && !lib.hasInfix "\r" value
    && !lib.any (part: part == "" || part == "." || part == "..") (
      lib.splitString "/" (lib.removePrefix "/" value)
    )
    && !lib.any (root: value == root || lib.hasPrefix "${root}/" value) [
      "/tmp"
      "/var/tmp"
      "/dev"
      "/proc"
      "/sys"
      "/run"
      "/var/run"
      "/var/lock"
      "/nix/store"
    ];
  externalPath =
    description:
    lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      inherit description;
    };

  proxy = cfg.remoteProxy;
  proxyEnabled = proxy.enable;
  isIngress = proxy.role == "ingress";
  isValidIpv4 =
    host:
    let
      octets = lib.splitString "." host;
      validOctet =
        part:
        builtins.match "0|[1-9][0-9]{0,2}" part != null
        && (lib.toInt part) >= 0
        && (lib.toInt part) <= 255;
    in
    builtins.length octets == 4 && lib.all validOctet octets && host != "0.0.0.0";
  isValidIpv6 =
    host:
    let
      charsOk = builtins.match "[0-9a-fA-F:]+" host != null && lib.hasInfix ":" host;
      noTriple = !lib.hasInfix ":::" host;
      edgeOk =
        (!lib.hasPrefix ":" host || lib.hasPrefix "::" host)
        && (!lib.hasSuffix ":" host || lib.hasSuffix "::" host);
      halves = lib.splitString "::" host;
      compressed = builtins.length halves == 2;
      splitCountOk = builtins.length halves <= 2;
      groupsOf = s: if s == "" then [ ] else lib.splitString ":" s;
      groups = lib.concatMap groupsOf halves;
      groupsOk = lib.all (g: builtins.match "[0-9a-fA-F]{1,4}" g != null) groups;
      countOk = if compressed then builtins.length groups <= 7 else builtins.length groups == 8;
      nonWildcard = builtins.match "[0:]+" host == null;
    in
    charsOk && noTriple && edgeOk && splitCountOk && groupsOk && countOk && nonWildcard;
  parsedRemote =
    let
      raw = proxy.remoteEndpoint;
      safeString =
        raw != null
        && !builtins.hasContext raw
        && !lib.hasInfix "\n" raw
        && !lib.hasInfix "\r" raw
        && !lib.hasInfix " " raw
        && !lib.hasInfix "\t" raw;
      bracketParts =
        if safeString && lib.hasPrefix "[" raw then
          lib.splitString "]:" (lib.removePrefix "[" raw)
        else
          null;
      plainParts =
        if safeString && !lib.hasPrefix "[" raw then lib.splitString ":" raw else null;
      isBracketed = bracketParts != null && builtins.length bracketParts == 2;
      isPlain = plainParts != null && builtins.length plainParts == 2;
      parts =
        if isBracketed then
          bracketParts
        else if isPlain then
          plainParts
        else
          null;
      host = if parts == null then "" else builtins.elemAt parts 0;
      portStr = if parts == null then "" else builtins.elemAt parts 1;
      portValidSyntax = builtins.match "[1-9][0-9]*" portStr != null && lib.stringLength portStr <= 5;
      portNum = if portValidSyntax then lib.toInt portStr else 0;
      isIpv6 = isBracketed && isValidIpv6 host;
      isIpv4 = isPlain && isValidIpv4 host;
      hostAllowed = isIpv6 || isIpv4;
    in
    {
      valid = parts != null && hostAllowed && portNum >= 1 && portNum <= 65535;
      inherit host isIpv6;
      port = portNum;
    };
  remoteHostCidr =
    if parsedRemote.isIpv6 then "${parsedRemote.host}/128" else "${parsedRemote.host}/32";
  proxyIpAllow = [
    "127.0.0.0/8"
    "::1/128"
    remoteHostCidr
  ]
  ++ proxy.allowedPeerCidrs;
  validSafeToken =
    value:
    value != null
    && value != ""
    && !builtins.hasContext value
    && !lib.hasInfix "\n" value
    && !lib.hasInfix "\r" value
    && !lib.hasInfix " " value
    && !lib.hasInfix "\t" value;
  formatListenStream =
    host: port:
    if lib.hasInfix ":" host then "[${host}]:${toString port}" else "${host}:${toString port}";
  # systemd-socket-proxyd splits HOST:PORT on the final colon without stripping
  # brackets, so IPv6 targets must stay unbracketed (for example ::1:17440).
  formatProxyTarget = host: port: "${host}:${toString port}";
  localListenStream = formatListenStream proxy.localAddress proxy.localPort;
  localProxyTarget = formatProxyTarget proxy.localAddress proxy.localPort;
  remoteListenStream = formatListenStream parsedRemote.host parsedRemote.port;
  remoteProxyTarget = formatProxyTarget parsedRemote.host parsedRemote.port;

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
    remoteProxy = {
      enable = lib.mkEnableOption ''
        loopback-preserving systemd-socket-proxyd forwarding over a private
        IPv6 or WireGuard mesh so fabric-server remains loopback-only
      '';
      role = lib.mkOption {
        type = lib.types.enum [
          "ingress"
          "egress"
        ];
        default = if receiverEnabled then "ingress" else "egress";
        description = ''
          Proxy role: "ingress" listens on remoteEndpoint and forwards to
          unbracketed loopback (::1:<localPort> or 127.0.0.1:<localPort>);
          "egress" listens on loopback and forwards to remoteEndpoint.
        '';
      };
      localAddress = lib.mkOption {
        type = lib.types.enum [
          "::1"
          "127.0.0.1"
        ];
        default = "::1";
        description = "Loopback address used by local fabric-server or Spindle forwarding.";
      };
      localPort = lib.mkOption {
        type = lib.types.port;
        default = 17440;
        description = "Loopback TCP port for local fabric-server or Spindle forwarding.";
      };
      remoteEndpoint = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = ''
          Private mesh endpoint as [ipv6]:port or ipv4:port. Used as the
          ingress ListenStream when role == "ingress", or as the unbracketed
          systemd-socket-proxyd destination when role == "egress".
        '';
      };
      interface = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "Optional private network interface (such as wg0) for ingress firewall rules.";
      };
      openFirewall = lib.mkOption {
        type = lib.types.bool;
        default = false;
        description = "Whether to open the ingress TCP port on remoteProxy.interface.";
      };
      freeBind = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = "Set FreeBind=true on the ingress socket so binding succeeds before mesh address assignment.";
      };
      connectionsMax = lib.mkOption {
        type = lib.types.ints.positive;
        default = 32;
        description = "Maximum concurrent proxied connections passed to systemd-socket-proxyd.";
      };
      allowedPeerCidrs = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [ ];
        description = ''
          Additional peer CIDRs permitted in IPAddressAllow alongside loopback
          (127.0.0.0/8, ::1/128) and the configured remoteEndpoint literal.
        '';
      };
    };
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
        message = "services.fabrico11y.dataDir, scratchDir and spindleConfigFile require normalized absolute paths outside the Nix store and known volatile directories.";
      }
      {
        assertion = !receiverEnabled || validExternalPath cfg.serverConfigFile;
        message = "services.fabrico11y.serverConfigFile requires a normalized absolute path outside the Nix store and known volatile directories.";
      }
      {
        assertion = !proxyEnabled || parsedRemote.valid;
        message = "services.fabrico11y.remoteProxy.remoteEndpoint must be a strict non-wildcard [ipv6]:port or ipv4:port literal without Nix store context.";
      }
      {
        assertion = !proxyEnabled || !isIngress || receiverEnabled;
        message = "services.fabrico11y.remoteProxy with role = \"ingress\" requires services.fabrico11y.serverConfigFile.";
      }
      {
        assertion = !proxyEnabled || proxy.interface == null || validSafeToken proxy.interface;
        message = "services.fabrico11y.remoteProxy.interface must be a non-empty interface name without whitespace or Nix store context.";
      }
      {
        assertion =
          !(proxyEnabled && isIngress && proxy.openFirewall)
          || validSafeToken proxy.interface;
        message = "services.fabrico11y.remoteProxy.openFirewall requires a non-empty services.fabrico11y.remoteProxy.interface.";
      }
      {
        assertion = !proxyEnabled || lib.all validSafeToken proxy.allowedPeerCidrs;
        message = "services.fabrico11y.remoteProxy.allowedPeerCidrs entries must be non-empty CIDR literals without whitespace or Nix store context.";
      }
      {
        assertion = !proxyEnabled || proxy.connectionsMax <= 256;
        message = "services.fabrico11y.remoteProxy.connectionsMax must not exceed 256.";
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

    networking.firewall.interfaces = lib.mkIf (proxyEnabled && isIngress && proxy.openFirewall) {
      ${proxy.interface}.allowedTCPPorts = [ parsedRemote.port ];
    };

    systemd.slices = lib.mkIf proxyEnabled {
      fabrico11y.description = "FabricO11y confined remote proxy slice";
    };

    systemd.sockets = lib.mkIf proxyEnabled {
      fabrico11y-remote-proxy = {
        description = "FabricO11y ${proxy.role} proxy socket";
        wantedBy = [ "sockets.target" ];
        wants = lib.optionals isIngress [ "network-online.target" ];
        after = lib.optionals isIngress [
          "network-online.target"
          "systemd-networkd.service"
        ];
        listenStreams = [ (if isIngress then remoteListenStream else localListenStream) ];
        socketConfig = {
          NoDelay = true;
        }
        // lib.optionalAttrs (isIngress && proxy.freeBind) {
          FreeBind = true;
        }
        // lib.optionalAttrs (isIngress && proxy.interface != null) {
          BindToDevice = proxy.interface;
        };
      };
    };

    systemd.services = lib.mkIf proxyEnabled {
      fabrico11y-remote-proxy = {
        description = "FabricO11y loopback-preserving ${proxy.role} proxy";
        requires = [ "fabrico11y-remote-proxy.socket" ];
        wants = lib.optionals (!isIngress) [ "network-online.target" ];
        after =
          [ "fabrico11y-remote-proxy.socket" ]
          ++ lib.optionals (!isIngress) [ "network-online.target" ];
        serviceConfig = {
          ExecStart = "${pkgs.systemd}/lib/systemd/systemd-socket-proxyd --exit-idle-time=60s --connections-max=${toString proxy.connectionsMax} ${
            if isIngress then localProxyTarget else remoteProxyTarget
          }";
          Slice = "fabrico11y.slice";
          MemoryMax = "64M";
          TasksMax = 64;
          CPUQuota = "25%";
          DynamicUser = true;
          PrivateTmp = true;
          PrivateDevices = true;
          ProtectKernelTunables = true;
          ProtectKernelModules = true;
          ProtectControlGroups = true;
          NoNewPrivileges = true;
          ProtectSystem = "strict";
          ProtectHome = true;
          CapabilityBoundingSet = "";
          RestrictNamespaces = true;
          MemoryDenyWriteExecute = true;
          LockPersonality = true;
          SystemCallFilter = "@system-service";
          RestrictAddressFamilies = [
            "AF_INET"
            "AF_INET6"
          ];
          IPAddressDeny = "any";
          IPAddressAllow = proxyIpAllow;
        };
      };
    };
  };
}
