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
      specialisation.canonical.configuration.services.fabrico11y.serverConfigFile =
        "/var/lib/fabrico11y/server.toml";
    };

  testScript = ''
    import datetime as dt
    import json
    import shlex
    import time

    def as_fabric(command):
        return (
            "runuser -u fabrictest -- env XDG_RUNTIME_DIR=/run/user/1000 "
            "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus "
            "sh -c " + shlex.quote(command)
        )

    state = "/var/lib/fabrico11y"
    socket = "/run/user/1000/spindle/spindle.sock"
    fabric = f"fabric --data-dir {state}/collector --tmp-dir {state}/scratch"
    local_ready = f"{fabric} production-ready --spindle-socket {socket} --json"
    deadline = dt.timedelta(seconds=90)
    observer = "systemctl --user --machine=observer@"

    machine.start()
    machine.wait_for_unit("user@1000.service")
    machine.wait_for_unit("user@1001.service")
    machine.wait_until_succeeds(as_fabric(local_ready), timeout=deadline)
    machine.fail(as_fabric("systemctl --user is-active fabric-server.service"))
    machine.wait_until_succeeds(f"{observer} show-environment", timeout=deadline)
    machine.fail(f"{observer} is-active spindle.service")

    def assert_slice(unit):
        assert machine.succeed(
            as_fabric(f"systemctl --user show -p Slice --value {unit}")
        ).strip() == "fabrico11y.slice", unit

    assert_slice("spindle.service")
    assert_slice("fabric-daemon.service")

    origin = json.loads(machine.succeed(as_fabric(f"spindle status {socket}")))["status"]["origin"]
    run_id = "f404b128-2c19-412f-bcd0-823f9c1f1b31"
    report_id = "edd65cef-e5aa-4f95-8ae6-6b30f9edb662"
    record = json.dumps({
        "command": "record",
        "report_id": report_id,
        "context": {"run_id": run_id},
        "timestamp_ns": time.time_ns(),
        "data": {
            "kind": "agent_run", "agent": "nixos-smoke", "run_id": run_id,
            "duration_ns": 1000, "status": "ok",
        },
    })
    send_report = as_fabric(
        f"printf '%s\n' {shlex.quote(record)} | "
        f"fabric-adapter {socket} nixos-smoke --batch-outbox {state}/outbox"
    )

    def submit():
        first_reply, *_ = machine.succeed(send_report).splitlines()
        reply = json.loads(first_reply)
        assert reply.get("report_id") == report_id and reply.get("receipt"), reply
        return reply["receipt"]

    def assert_record():
        records = machine.succeed(as_fabric(f"spindle export {socket}")).splitlines()
        matching = [row for row in records if json.loads(row)["data"].get("run_id") == run_id]
        assert len(matching) == 1, matching
        return matching[0]

    receipt = submit()
    stored = assert_record()
    assert json.loads(stored)["identity"] == receipt["identity"]
    machine.succeed(as_fabric("systemctl --user restart spindle.service fabric-daemon.service"))
    machine.wait_until_succeeds(as_fabric(local_ready), timeout=deadline)
    assert submit() == receipt
    assert assert_record() == stored

    # Enroll the existing local origin in the optional receiver using private VM fixtures.
    endpoint = "127.0.0.1:17440"
    node_key = f"{state}/node.key"
    reader_key = f"{state}/reader.key"
    server_config = "\n".join([
        "version = 1",
        f'listen = "{endpoint}"',
        f'storage = "{state}/canonical"',
        f'projection = "{state}/projection"',
        "quota_bytes = 268435456",
        "projection_quota_bytes = 268435456",
        f'reader_credential_file = "{reader_key}"',
        "[[nodes]]",
        f"origin = {json.dumps(origin)}",
        f'credential_file = "{node_key}"',
    ])
    for key in [node_key, reader_key]:
        machine.succeed(as_fabric(
            f"umask 077; od -An -N32 -tx1 /dev/urandom | tr -d ' \\n' > {key}"
        ))
    machine.succeed(as_fabric(
        f"umask 077; printf '%s\n' {shlex.quote(server_config)} > {state}/server.toml"
    ))
    machine.succeed("/run/booted-system/specialisation/canonical/bin/switch-to-configuration test")
    machine.succeed(as_fabric("systemctl --user daemon-reload"))
    machine.succeed(as_fabric("systemctl --user restart spindle.service fabric-daemon.service"))
    machine.succeed(as_fabric("systemctl --user is-active fabric-server.service"))
    assert_slice("fabric-server.service")
    machine.wait_until_succeeds(f"{observer} show-environment", timeout=deadline)
    machine.fail(f"{observer} is-active fabric-server.service")

    connection = f"FABRIC_ENDPOINT={endpoint} FABRIC_CREDENTIAL_FILE={reader_key}"
    progress = json.loads(machine.succeed(as_fabric(f"{connection} fabric-query progress")))
    authority = progress["authority"]
    forwarding = "\n".join([
        "",
        "[forwarding]",
        f'endpoint = "{endpoint}"',
        f"authority = {json.dumps(authority)}",
        f'credential_file = "{node_key}"',
        "reclaim_confirmed = false",
    ])
    machine.succeed(as_fabric(f"printf '%s\n' {shlex.quote(forwarding)} >> {state}/spindle.toml"))
    machine.succeed(as_fabric("systemctl --user restart spindle.service fabric-daemon.service"))
    receiver = f"--server-endpoint {endpoint} --reader-credential-file {reader_key}"
    production = f"{local_ready} --require-canonical {receiver}"
    ready = json.loads(machine.wait_until_succeeds(as_fabric(production), timeout=deadline))["data"]
    assert ready["canonical_authority"] == authority
    projected = json.loads(machine.wait_until_succeeds(as_fabric(
        f"printf '%s\n' {shlex.quote(json.dumps(receipt))} | "
        f"{fabric} projection-ready --target canonical --authority {shlex.quote(authority)} "
        f"--origin {shlex.quote(origin)} --receipt - --wait 15s --json {receiver}"
    ), timeout=deadline))["data"]
    assert projected["status"] == "visible", projected
    assert projected["record_id"] == receipt["id"] and projected["identity"] == receipt["identity"], projected
    assert projected["origin"] == origin and projected["authority"] == authority, projected

    machine.succeed(as_fabric("systemctl --user stop fabric-daemon.service spindle.service fabric-server.service"))
    machine.succeed(f"test -d {state}/spindle && test -d {state}/canonical && test -d {state}/projection")
  '';
}
