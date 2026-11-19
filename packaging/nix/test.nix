{ pkgs, module }:
pkgs.testers.runNixOSTest {
  name = "fabrico11y-user-services";
  nodes.machine =
    { pkgs, ... }:
    let
      spindleConfig = pkgs.writeText "fabrico11y-test-spindle.toml" ''
        version = 1
        socket = "/run/user/1000/spindle/spindle.sock"
        storage = "/var/lib/fabrico11y/spindle"
        quota_bytes = 67108864
        max_connections = 16
        client_timeout_ms = 500
      '';
    in
    {
      imports = [ module ];
      system.stateVersion = "26.11";
      virtualisation.memorySize = 2048;

      users.users.fabrictest = {
        isNormalUser = true;
        uid = 1000;
        linger = true;
      };
      users.users.observer = {
        isNormalUser = true;
        uid = 1001;
        linger = true;
      };

      systemd.tmpfiles.rules = [
        "d /var/lib/fabrico11y 0700 fabrictest users -"
        "d /var/lib/fabrico11y/collector 0700 fabrictest users -"
        "d /var/lib/fabrico11y/scratch 0700 fabrictest users -"
        "C /var/lib/fabrico11y/spindle.toml 0600 fabrictest users - ${spindleConfig}"
      ];

      services.fabrico11y = {
        enable = true;
        user = "fabrictest";
        dataDir = "/var/lib/fabrico11y/collector";
        scratchDir = "/var/lib/fabrico11y/scratch";
        spindleConfigFile = "/var/lib/fabrico11y/spindle.toml";
      };
    };

  testScript = ''
    import json
    import shlex
    import time

    def as_fabric(command):
        return (
            "runuser -u fabrictest -- env XDG_RUNTIME_DIR=/run/user/1000 "
            "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus "
            "sh -c " + shlex.quote(command)
        )

    socket = "/run/user/1000/spindle/spindle.sock"
    fabric = "fabric --data-dir /var/lib/fabrico11y/collector --tmp-dir /var/lib/fabrico11y/scratch"

    machine.start()
    machine.wait_for_unit("user@1000.service")
    machine.wait_for_unit("user@1001.service")
    machine.wait_until_succeeds(as_fabric("systemctl --user is-active spindle.service fabric-daemon.service"))
    machine.fail(as_fabric("systemctl --user is-active fabric-server.service"))
    machine.wait_until_succeeds("systemctl --user --machine=observer@ show-environment")
    machine.fail("systemctl --user --machine=observer@ is-active spindle.service")
    for unit in ["spindle.service", "fabric-daemon.service"]:
        machine.succeed(as_fabric(f"test \"$(systemctl --user show -p Slice --value {unit})\" = fabrico11y.slice"))
    machine.wait_until_succeeds(as_fabric(f"{fabric} production-ready --spindle-socket {socket} --json"))

    marker = "nixos-package-restart-witness"
    record = json.dumps({
        "command": "record",
        "timestamp_ns": time.time_ns(),
        "data": {"kind": "event", "name": "nixos-smoke", "message": marker},
    })
    flush = json.dumps({"command": "flush", "timeout_ms": 2000})
    commands = shlex.quote(record) + " " + shlex.quote(flush)
    for _ in range(10):
        output = machine.succeed(as_fabric(f"printf '%s\n' {commands} | fabric-adapter {socket} nixos-smoke"))
        replies = [json.loads(line) for line in output.splitlines()]
        if replies[0].get("admission") in ("contended", "full"):
            continue
        assert any(reply.get("receipt") for reply in replies), output
        break
    else:
        raise AssertionError("adapter did not admit the restart witness")

    def assert_record():
        records = machine.succeed(as_fabric(f"spindle export {socket}")).splitlines()
        matching = [record for record in records if json.loads(record)["data"].get("message") == marker]
        assert len(matching) == 1, matching
        return matching[0]

    stored = assert_record()
    machine.succeed(as_fabric("systemctl --user restart spindle.service fabric-daemon.service"))
    machine.wait_until_succeeds(as_fabric(f"{fabric} production-ready --spindle-socket {socket} --json"))
    assert assert_record() == stored
    machine.succeed(as_fabric("systemctl --user stop fabric-daemon.service spindle.service"))
    machine.succeed("test -d /var/lib/fabrico11y/spindle")
  '';
}
