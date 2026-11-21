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
  evalWith =
    overrides:
    base.extendModules {
      modules = [
        {
          services.fabrico11y = lib.mapAttrs (
            k: v: if k == "remoteProxy" then lib.mapAttrs (_: lib.mkForce) v else lib.mkForce v
          ) overrides;
        }
      ];
    };
  passesAssertions =
    overrides:
    let
      tested = evalWith overrides;
    in
    !lib.any (
      assertion: !assertion.assertion && lib.hasPrefix "services.fabrico11y." assertion.message
    ) tested.config.assertions;

  ingressEval = evalWith {
    remoteProxy = {
      enable = true;
      role = "ingress";
      localAddress = "::1";
      localPort = 17440;
      remoteEndpoint = "[2001:db8::10]:17440";
      interface = "wg0";
      openFirewall = true;
      allowedPeerCidrs = [ "2001:db8::/64" ];
    };
  };

  egressEval = evalWith {
    serverConfigFile = null;
    remoteProxy = {
      enable = true;
      role = "egress";
      localAddress = "::1";
      localPort = 17440;
      remoteEndpoint = "[2001:db8::10]:17440";
    };
  };

  ipv4EgressEval = evalWith {
    serverConfigFile = null;
    remoteProxy = {
      enable = true;
      role = "egress";
      localAddress = "127.0.0.1";
      localPort = 17440;
      remoteEndpoint = "10.200.0.1:17440";
    };
  };

  hasHardening =
    svcCfg:
    svcCfg.Slice == "fabrico11y.slice"
    && svcCfg.MemoryMax == "64M"
    && svcCfg.TasksMax == 64
    && svcCfg.CPUQuota == "25%"
    && svcCfg.PrivateDevices == true
    && svcCfg.ProtectKernelTunables == true
    && svcCfg.ProtectKernelModules == true
    && svcCfg.ProtectControlGroups == true
    && svcCfg.CapabilityBoundingSet == ""
    && svcCfg.RestrictNamespaces == true
    && svcCfg.MemoryDenyWriteExecute == true
    && svcCfg.LockPersonality == true
    && svcCfg.SystemCallFilter == "@system-service"
    && svcCfg.IPAddressDeny == "any"
    && lib.hasInfix "--exit-idle-time=60s" svcCfg.ExecStart;
in
lib.assertMsg (passesAssertions { }) "FabricO11y rejected normalized external paths"
&& lib.all (
  overrides:
  lib.assertMsg (!passesAssertions overrides) "FabricO11y accepted invalid configuration: ${builtins.toJSON overrides}"
) [
  { spindleConfigFile = "//nix/store/preexisting-config"; }
  { serverConfigFile = "/nix/store/preexisting-config"; }
  { dataDir = "/run/user/1000/fabrico11y-data"; }
  { scratchDir = "/var/run/fabrico11y-scratch"; }
  { scratchDir = "/var/lib//fabrico11y/scratch"; }
  { spindleConfigFile = "/var/lib/fabrico11y/../spindle.toml"; }
  { remoteProxy = { enable = true; }; }
  { remoteProxy = { enable = true; remoteEndpoint = "2001:db8::10"; }; }
  { remoteProxy = { enable = true; remoteEndpoint = "[::]:17440"; }; }
  { remoteProxy = { enable = true; remoteEndpoint = "[::0]:17440"; }; }
  { remoteProxy = { enable = true; remoteEndpoint = "[0:0:0:0:0:0:0:0]:17440"; }; }
  { remoteProxy = { enable = true; remoteEndpoint = "[0::0]:17440"; }; }
  { remoteProxy = { enable = true; remoteEndpoint = "0.0.0.0:17440"; }; }
  { remoteProxy = { enable = true; remoteEndpoint = "256.0.0.1:17440"; }; }
  { remoteProxy = { enable = true; remoteEndpoint = "example.com:17440"; }; }
  { remoteProxy = { enable = true; remoteEndpoint = "localhost:17440"; }; }
  { remoteProxy = { enable = true; remoteEndpoint = "[2001:db8::10]:70000"; }; }
  { remoteProxy = { enable = true; role = "ingress"; remoteEndpoint = "[2001:db8::10]:17440"; openFirewall = true; }; }
  { serverConfigFile = null; remoteProxy = { enable = true; role = "ingress"; remoteEndpoint = "[2001:db8::10]:17440"; }; }
]
&& lib.assertMsg (
  passesAssertions {
    remoteProxy = {
      enable = true;
      role = "ingress";
      localAddress = "::1";
      localPort = 17440;
      remoteEndpoint = "[2001:db8::10]:17440";
      interface = "wg0";
      openFirewall = true;
      allowedPeerCidrs = [ "2001:db8::/64" ];
    };
  }
) "FabricO11y rejected valid ingress remoteProxy config"
&& lib.assertMsg (
  ingressEval.config.systemd.sockets.fabrico11y-remote-proxy.listenStreams == [ "[2001:db8::10]:17440" ]
  && ingressEval.config.systemd.sockets.fabrico11y-remote-proxy.socketConfig.BindToDevice == "wg0"
  && lib.hasSuffix " ::1:17440" ingressEval.config.systemd.services.fabrico11y-remote-proxy.serviceConfig.ExecStart
  && !lib.hasInfix "[::1]:17440" ingressEval.config.systemd.services.fabrico11y-remote-proxy.serviceConfig.ExecStart
  && hasHardening ingressEval.config.systemd.services.fabrico11y-remote-proxy.serviceConfig
  && ingressEval.config.systemd.services.fabrico11y-remote-proxy.serviceConfig.IPAddressAllow == [
    "127.0.0.0/8"
    "::1/128"
    "2001:db8::10/128"
    "2001:db8::/64"
  ]
  && ingressEval.config.networking.firewall.interfaces.wg0.allowedTCPPorts == [ 17440 ]
) "Ingress remoteProxy did not produce expected socket/hardening/BindToDevice/IPAddressAllow/firewall rules"
&& lib.assertMsg (
  passesAssertions {
    serverConfigFile = null;
    remoteProxy = {
      enable = true;
      role = "egress";
      localAddress = "::1";
      localPort = 17440;
      remoteEndpoint = "[2001:db8::10]:17440";
    };
  }
) "FabricO11y rejected valid egress remoteProxy config"
&& lib.assertMsg (
  egressEval.config.systemd.sockets.fabrico11y-remote-proxy.listenStreams == [ "[::1]:17440" ]
  && !(egressEval.config.systemd.sockets.fabrico11y-remote-proxy.socketConfig ? BindToDevice)
  && lib.hasSuffix " 2001:db8::10:17440" egressEval.config.systemd.services.fabrico11y-remote-proxy.serviceConfig.ExecStart
  && !lib.hasInfix "[2001:db8::10]:17440" egressEval.config.systemd.services.fabrico11y-remote-proxy.serviceConfig.ExecStart
  && hasHardening egressEval.config.systemd.services.fabrico11y-remote-proxy.serviceConfig
  && egressEval.config.systemd.services.fabrico11y-remote-proxy.serviceConfig.IPAddressAllow == [
    "127.0.0.0/8"
    "::1/128"
    "2001:db8::10/128"
  ]
) "Egress remoteProxy did not produce expected loopback socket/hardening/unbracketed IPv6 target"
&& lib.assertMsg (
  ipv4EgressEval.config.systemd.sockets.fabrico11y-remote-proxy.listenStreams == [ "127.0.0.1:17440" ]
  && ipv4EgressEval.config.systemd.services.fabrico11y-remote-proxy.serviceConfig.IPAddressAllow == [
    "127.0.0.0/8"
    "::1/128"
    "10.200.0.1/32"
  ]
) "IPv4 egress remoteProxy did not produce expected /32 IPAddressAllow"
