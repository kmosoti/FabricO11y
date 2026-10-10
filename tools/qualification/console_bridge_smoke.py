"""Finite production bridge prerequisite, not a registered long campaign result."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import secrets
import shutil
import signal
import ssl
import subprocess
import sys
import time
import urllib.error
import urllib.request

from console_bridge import ConsoleBridge, ROOT, STORAGE, require_limits
from delivery_faults import free_port, openssl
sys.path.insert(0, str(ROOT / "tools/packaging"))
from stage_console import stage


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--build", type=Path, required=True)
    parser.add_argument("--bin-dir", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--poll-seconds", type=int, default=12)
    args = parser.parse_args()
    if not 12 <= args.poll_seconds <= 600:
        raise ValueError('finite polling smoke must be 12–600 seconds')
    group = require_limits()
    subprocess.run(["systemctl", "--user", "set-property", "--runtime", group.name,
                    "MemoryHigh=3500000000", "MemoryMax=4000000000", "MemorySwapMax=0",
                    "CPUQuota=200%", "TasksMax=512"], check=True, capture_output=True)
    if int((group / "memory.max").read_text()) > 4_000_000_000:
        raise RuntimeError("4 GB combined fixture cap was not enforced")
    out = args.out
    if (not out.is_absolute() or out.exists() or out.is_symlink()
            or not out.parent.resolve(strict=True).is_relative_to(STORAGE / "results")):
        raise ValueError("fresh DATA/results receipt directory required")
    bins = args.bin_dir.resolve(strict=True)
    original_binaries = [bins / "fabric-server", bins / "fabric-node"]
    binaries = original_binaries
    if not bins.is_relative_to(STORAGE) or any(p.is_symlink() or not p.is_file() for p in binaries):
        raise ValueError("actual data-drive production binaries required")
    out.mkdir(mode=0o700)
    work = Path(os.environ["FABRIC_SCRATCH_ROOT"]) / "console-bridge-smoke"
    work.mkdir(mode=0o700)
    receipt = {"command": sys.argv, "exit": 1, "checks": [], "cgroup": str(group),
               "scope": "finite bridge smoke only; no long campaign or physical-device claim",
               "binaries": {str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in binaries},
               "source_sha256": {p.name: hashlib.sha256(p.read_bytes()).hexdigest()
                                 for p in (Path(__file__), Path(__file__).with_name("console_bridge.py"))}}
    snapshot = work / "bin"
    snapshot.mkdir(mode=0o700)
    for original in original_binaries:
        target = snapshot / original.name
        shutil.copy2(original, target)
        if hashlib.file_digest(target.open("rb"), "sha256").hexdigest() != receipt["binaries"][str(original)]:
            raise RuntimeError("native candidate changed during immutable snapshot")
    binaries = [snapshot / p.name for p in original_binaries]
    receipt["native_snapshot"] = "all server and companion launches use verified immutable owned copies"
    processes, logs = [], []
    def check(value, label):
        receipt["checks"].append({"label": label, "passed": bool(value)})
        if not value: raise RuntimeError(label)
    def launch_server():
        log = (out / ("server-%d.log" % len(processes))).open("w")
        logs.append(log)
        proc = subprocess.Popen([str(binaries[0]), "serve", str(work / "server.conf")],
                                stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        processes.append(proc)
        deadline = time.monotonic() + 20
        while True:
            try:
                with urllib.request.urlopen(origin + "/console/index.html", context=tls, timeout=2) as response:
                    if response.status == 200: break
            except (OSError, urllib.error.URLError) as error:
                receipt["readiness_last_error"] = {"type": type(error).__name__, "status": getattr(error, "code", None),
                    "reason_type": type(getattr(error, "reason", None)).__name__}
                if isinstance(getattr(error, "reason", None), ssl.SSLCertVerificationError):
                    receipt["readiness_last_error"]["verify_code"] = error.reason.verify_code
            if proc.poll() is not None or time.monotonic() >= deadline:
                raise RuntimeError("production server did not start")
            time.sleep(.1)
        return proc
    def stop_server(proc):
        proc.send_signal(signal.SIGTERM)
        proc.wait(timeout=20)
        check(proc.returncode == 0, "orderly production server exit")
    try:
        stage(args.build.resolve(strict=True), work / "console")
        (work / "san.ext").write_text("subjectAltName=DNS:localhost,IP:127.0.0.1\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\n")
        openssl(work, "req", "-x509", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:P-256", "-nodes", "-keyout", "ca.key", "-out", "ca.pem", "-days", "1", "-subj", "/CN=Owned bridge CA", "-addext", "basicConstraints=critical,CA:TRUE", "-addext", "keyUsage=critical,keyCertSign,cRLSign")
        openssl(work, "req", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:P-256", "-nodes", "-keyout", "server.key", "-out", "server.csr", "-subj", "/CN=localhost")
        openssl(work, "x509", "-req", "-in", "server.csr", "-CA", "ca.pem", "-CAkey", "ca.key", "-CAcreateserial", "-out", "server.pem", "-days", "1", "-extfile", "san.ext")
        for name in ("ca.key", "server.key"):(work / name).chmod(0o600)
        admin = work / "legacy-token-unused"
        admin.write_text(secrets.token_hex(32) + "\n"); admin.chmod(0o600)
        port = free_port(); origin = f"https://localhost:{port}"
        tls = ssl.create_default_context(cafile=str(work / "ca.pem"))
        (work / "server.conf").write_text(f"listen=127.0.0.1:{port}\ntls_cert={work}/server.pem\ntls_key={work}/server.key\nstate_dir={work}/state\nadmin_token_file={admin}\nconsole_dir={work}/console\naccess_origin={origin}\naccess_rp_id=localhost\njournal_bytes=8388608\njournal_file_bytes=1048576\nself_spindle_ca={work}/ca.pem\n")
        server = launch_server()
        with ConsoleBridge(origin, work / "server.pem", work / "browser", out / "bridge") as bridge:
            view = bridge.bootstrap_owner(work / "state/access/access-bootstrap.secret")
            check(view["kind"] == "human" and not (work / "state/access/access-bootstrap.secret").exists(), "real passkey owner consumed bootstrap")
            status, enrolled = bridge.request("/v1/console/nodes", "POST", {"name": "campaign-source", "metric_interval_s": 15})
            check(status == 200 and "enrollment_id" in enrolled, "fresh human enrolled scoped source")
            scope = dict(view["scope"])
            scope.update(actions=["telemetry_read", "inventory_read"], installation_wide=False,
                         enrollments=[enrolled["enrollment_id"]], signals=["logs"],
                         max_query_rows=100, max_query_window_s=60, allowed_log_paths=[],
                         enrollment_namespace=None, max_enrollments=0)
            status, issued = bridge.issue_workload("campaign-reader", scope, 300)
            check(status == 200 and bool(issued.get("token")), "real human issued scoped workload credential")
            token = issued.pop("token")
            def bearer(path, method="GET", body=None):
                req = urllib.request.Request(origin + path, method=method,
                     data=None if body is None else json.dumps(body).encode(), headers={
                         "authorization": "Bearer " + token, "x-fabric-client-version": "1", "content-type": "application/json"})
                try:
                    with urllib.request.urlopen(req, context=tls, timeout=15) as response:
                        return response.status, json.load(response)
                except urllib.error.HTTPError as error:
                    return error.code, json.loads(error.read())
            status, _ = bearer("/v1/console/nodes")
            check(status == 200, "scoped workload inventory")
            status, _ = bearer("/v1/console/nodes", "POST", {"name": "forbidden"})
            check(status == 403, "workload cannot enroll sources")
            end = time.time_ns()
            status, _ = bearer("/v1/console/query", "POST", {"kind": "logs", "node": "campaign-source", "from_ns": end - 30*10**9, "to_ns": end, "limit": 100})
            check(status == 200, "scoped production workload query")
            token = None
            bridge.poll_ui(True)
            deadline = time.monotonic() + args.poll_seconds
            while time.monotonic() < deadline:
                time.sleep(min(15, max(0, deadline - time.monotonic())))
                if args.poll_seconds >= 330:
                    status, _ = bridge.request('/v1/console/nodes/campaign-source/config', 'PUT',
                        {'logs': [], 'metric_interval_s': 15})
                    check(status == 200, 'finite smoke fresh owner management')
                bridge.poll_health()
            bridge.poll_ui(False)
            health = bridge.poll_health(require_progress=False)
            check(health['completed'] >= 2 and health['statusFailures'] == 0
                  and health['transportFailures'] == 0 and health['inflight'] == 0,
                  'actual bounded visible UI body-completion poll phase')
            if args.poll_seconds >= 330:
                check(health['refreshes'] >= 1, 'actual UV refresh restored native UI session and polling')
            receipt['ui_polling_observation'] = health
            stop_server(server)
            launch_server()
            view = bridge.login()
            check(view["fresh_until_unix_s"] > int(time.time()), "explicit real passkey login after server restart")
            status, _ = bridge.request("/v1/console/nodes")
            check(status == 200, "production controls available after explicit restart login")
        stop_server(processes[-1])
        receipt["exit"] = 0
    except BaseException as error:
        receipt["failure_type"] = type(error).__name__
        raise
    finally:
        for proc in processes:
            if proc.poll() is None:
                os.killpg(proc.pid, signal.SIGTERM)
                try:proc.wait(timeout=20)
                except subprocess.TimeoutExpired:
                    os.killpg(proc.pid, signal.SIGKILL);proc.wait(timeout=10)
        for log in logs:log.close()
        shutil.rmtree(work)
        receipt["cleanup"] = "owned production servers stopped; certificates/state/browser fixtures removed"
        (out / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")


if __name__ == "__main__":
    main()
