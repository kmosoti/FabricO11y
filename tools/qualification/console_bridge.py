"""Owned campaign browser bridge using production HTTPS and real WebAuthn.

The caller starts/stops the server and supplies its exact localhost HTTPS origin,
leaf certificate, staged console and protected bootstrap path. Chrome's virtual
CTAP2 authenticator exercises the real browser and server WebAuthn implementation;
this is not evidence for physical authenticators. Keep this browser alive across
server restarts, then call login() explicitly. Credentials never enter receipts.
Use under the resource launcher. By default the combined fixture cap is at
most 4 GB. A larger finite outer campaign requires an explicitly bounded,
initially empty browser subgroup; the caller separately owns server containment.
"""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import socket
import stat
import struct
import subprocess
import sys
import threading
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from resource_group import require_limits, STORAGE

VERSION = "155.0.8059.39"
TOOLS = STORAGE / "tools/ui" / ("chrome-for-testing-" + VERSION)
MAX_BODY = 8 * 1024**2
MAX_REQUEST = 1024**2
MAX_PROFILE = 512 * 1024**2


def validate_route(path, method):
    if method not in ("GET", "POST", "PUT") or not isinstance(path, str) or not re.fullmatch(
            r"/v1/console/[A-Za-z0-9_/-]{1,1024}", path):
        raise ValueError("only canonical production console routes are allowed")
    if any(part in ("", ".", "..") for part in path.split("/")[1:]):
        raise ValueError("noncanonical console route")


def protected_secret(path):
    path = Path(path)
    if not path.is_absolute():
        raise ValueError("absolute protected bootstrap path required")
    path.parent.resolve(strict=True).relative_to(Path(os.environ["FABRIC_SCRATCH_ROOT"]).resolve(strict=True))
    for member in (path, *path.parents):
        if member.is_symlink():
            raise ValueError("bootstrap path must not contain symlinks")
    # O_NOFOLLOW plus fstat avoids following a replaced leaf after admission.
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    try:
        info = os.fstat(descriptor)
        if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid()
                or stat.S_IMODE(info.st_mode) != 0o600 or info.st_nlink != 1
                or not 1 <= info.st_size <= 256):
            raise ValueError("owner-only regular bootstrap file required")
        secret = os.read(descriptor, 257).decode().strip()
        if not secret or len(secret) > 256:
            raise ValueError("invalid bounded bootstrap secret")
        return secret
    finally:
        os.close(descriptor)


class ConsoleBridge:
    """One browser, one serialized request, ephemeral owner and authenticator."""
    def __init__(self, origin, leaf_certificate, work, out, *, browser_group=None):
        self.group = require_limits()
        self.browser_group = None
        if browser_group is None:
            if int((self.group / "memory.max").read_text()) > 4_000_000_000:
                raise ValueError("larger outer campaigns require an explicit browser subgroup")
        else:
            service = next((parent for parent in (self.group, *self.group.parents)
                            if parent.name.startswith("fabric-work-") and parent.name.endswith(".service")), None)
            bounded = Path(browser_group).resolve(strict=True)
            if service is None or bounded == service or not bounded.is_relative_to(service):
                raise ValueError("browser subgroup must belong to this owned launcher service")
            values = {key: (bounded / key).read_text().strip() for key in
                      ("memory.max", "memory.high", "memory.swap.max", "pids.max", "cpu.max")}
            maximum, high = int(values["memory.max"]), int(values["memory.high"])
            quota, period = values["cpu.max"].split()
            if (not 0 < high <= maximum <= 4_000_000_000 or values["memory.swap.max"] != "0"
                    or not 0 < int(values["pids.max"]) <= 512 or quota == "max"
                    or not 0 < int(quota) <= 2 * int(period) or int(period) <= 0
                    or (bounded / "cgroup.procs").read_text().strip()
                    or "populated 0" not in (bounded / "cgroup.events").read_text().splitlines()):
                raise ValueError("empty browser subgroup needs explicit memory/swap/CPU/task limits")
            self.browser_group = bounded
        parsed = urllib.parse.urlsplit(origin)
        if (parsed.scheme != "https" or parsed.hostname != "localhost" or not parsed.port
                or parsed.username or parsed.password or parsed.path or parsed.query or parsed.fragment):
            raise ValueError("exact owned HTTPS localhost origin with port required")
        self.origin = origin
        self.work, self.out = Path(work), Path(out)
        scratch = Path(os.environ["FABRIC_SCRATCH_ROOT"]).resolve(strict=True)
        if (not self.work.is_absolute() or self.work.exists()
                or self.work.is_symlink() or not self.work.parent.resolve(strict=True).is_relative_to(scratch)):
            raise ValueError("fresh owned scratch child required")
        if (not self.out.is_absolute() or self.out.exists() or self.out.is_symlink()
                or not self.out.parent.resolve(strict=True).is_relative_to(STORAGE / "results")):
            raise ValueError("fresh data-drive result directory required")
        self.work.mkdir(mode=0o700)
        self.out.mkdir(mode=0o700)
        self.lock = threading.RLock()
        self.session = None
        self.driver = None
        self.log = None
        self.browser_temporary = None
        self.principal_id = None
        self.closed = False
        self.ui_polling = False
        self.ui_samples = []
        self.ui_previous = None
        self.ui_phase_baseline = None
        self.records = []
        self.receipt = {"state": "starting", "origin": origin, "browser": VERSION,
                        "authenticator": "virtual CTAP2 with user verification",
                        "scope": "campaign fixture, not physical authenticator qualification",
                        "requests": self.records, "source_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest()}
        try:
            leaf = Path(leaf_certificate)
            if leaf.is_symlink() or not leaf.is_file() or leaf.stat().st_size > 65536:
                raise ValueError("regular fixture leaf certificate required")
            pub = subprocess.check_output(["openssl", "x509", "-in", str(leaf), "-pubkey", "-noout"], timeout=10)
            der = subprocess.run(["openssl", "pkey", "-pubin", "-outform", "DER"],
                                 input=pub, check=True, capture_output=True, timeout=10).stdout
            spki = base64.b64encode(hashlib.sha256(der).digest()).decode()
            self.receipt["tls_scope"] = "exact owned leaf SPKI trusted only in this isolated Chrome profile"
            self.receipt["leaf_sha256"] = hashlib.sha256(leaf.read_bytes()).hexdigest()
            with socket.socket() as port:
                port.bind(("127.0.0.1", 0))
                driver_port = port.getsockname()[1]
            self.endpoint = f"http://127.0.0.1:{driver_port}"
            env = dict(os.environ)
            # Chrome creates AF_UNIX sockets below TMPDIR. The frozen runner
            # path can exceed sun_path even though its profile is valid. Keep
            # these temporary files in an owned short disk-backed directory.
            self.browser_temporary = Path(tempfile.mkdtemp(prefix="cb-", dir=scratch))
            if len(os.fsencode(str(self.browser_temporary))) > 60:
                raise ValueError("browser scratch prefix is too long for bounded Unix socket paths")
            env["TMPDIR"] = str(self.browser_temporary)
            for name in ("XDG_DATA_HOME", "XDG_CONFIG_HOME", "XDG_CACHE_HOME"):
                directory = self.work / name.lower()
                directory.mkdir(mode=0o700)
                env[name] = str(directory)
            self.log = (self.out / "webdriver.log").open("w")
            command = [str(TOOLS / "chromedriver-linux64/chromedriver"),
                       "--port=" + str(driver_port), "--allowed-ips=127.0.0.1"]
            if self.browser_group is not None:
                helper = Path(__file__).parent / "enter_group.py"
                if not helper.is_file():
                    helper = ROOT / "tools/bench/labs/completion/enter_group.py"
                command = [sys.executable, str(helper), str(self.browser_group), *command]
            self.driver = subprocess.Popen(command,
                                          stdout=self.log, stderr=subprocess.STDOUT, env=env, start_new_session=True)
            deadline = time.monotonic() + 15
            while True:
                try:
                    if self._call("GET", "/status").get("ready"):
                        break
                except (OSError, RuntimeError):
                    pass
                if self.driver.poll() is not None or time.monotonic() >= deadline:
                    raise RuntimeError("owned WebDriver did not start")
                time.sleep(.1)
            info = self._call("POST", "/session", {"capabilities": {"alwaysMatch": {
                "browserName": "chrome", "goog:chromeOptions": {
                    "binary": str(TOOLS / "chrome-linux64/chrome"), "args": [
                        "--headless=new", "--disable-dev-shm-usage", "--no-first-run",
                        "--no-default-browser-check", "--user-data-dir=" + str(self.work / "profile"),
                        "--ignore-certificate-errors-spki-list=" + spki, "--window-size=1440,1100"]}}}})
            self.session = info["sessionId"]
            if info["capabilities"]["browserVersion"] != VERSION:
                raise RuntimeError("browser differs from reviewed version")
            self._wd("/timeouts", {"script": 45000, "pageLoad": 30000, "implicit": 0})
            self._cdp("WebAuthn.enable", {})
            self._cdp("WebAuthn.addVirtualAuthenticator", {"options": {
                "protocol": "ctap2", "transport": "internal", "hasResidentKey": True,
                "hasUserVerification": True, "isUserVerified": True, "automaticPresenceSimulation": True}})
            self._wd("/url", {"url": origin + "/console/"})
            deadline = time.monotonic() + 30
            while not self._js("return typeof window.fabricPasskey==='function'"):
                if time.monotonic() >= deadline:
                    raise RuntimeError("production console passkey helper did not load")
                time.sleep(.1)
            # Observe only native UI Request objects, never bridge string-based
            # requests, response contents, cookies or credentials. Unlike the
            # default Resource Timing buffer these counters cannot fill up.
            self._js('''window.fabricCampaignUiProbe={active:false,attempts:0,headers:0,completed:0,
              statusFailures:0,transportFailures:0,inflight:0,lastAttemptMs:null,lastCompletedMs:null,
              maxAttemptGapMs:0,refreshes:0,phases:0,phaseLastAttemptMs:null};
              const original=window.fetch;window.fetch=function(...args){
                const p=window.fabricCampaignUiProbe;
                const native=p.active && args[0] instanceof Request &&
                  new URL(args[0].url).pathname==='/v1/console/query';
                if(!native)return original.apply(this,args);
                const now=performance.now();if(p.phaseLastAttemptMs!==null)
                  p.maxAttemptGapMs=Math.max(p.maxAttemptGapMs,now-p.phaseLastAttemptMs);
                p.phaseLastAttemptMs=now;p.lastAttemptMs=now;p.attempts++;p.inflight++;
                let finished=false;const finish=(failed)=>{if(finished)return;finished=true;
                  p.inflight--;if(failed)p.transportFailures++;else{p.completed++;p.lastCompletedMs=performance.now();}};
                return original.apply(this,args).then(response=>{
                  p.headers++;if(response.status<200||response.status>=300)p.statusFailures++;
                  if(!response.body){finish(false);return response;}
                  const stream=response.body;const getReader=stream.getReader.bind(stream);
                  stream.getReader=function(...options){const reader=getReader(...options);
                    const read=reader.read.bind(reader);reader.read=function(...values){
                      return read(...values).then(part=>{if(part.done)finish(false);return part;},
                        error=>{finish(true);throw error;});};return reader;};
                  return response;
                },error=>{finish(true);throw error;});};''')
            if self.browser_group is not None:
                actual = next(line[3:] for line in Path(f"/proc/{self.driver.pid}/cgroup").read_text().splitlines() if line.startswith("0::"))
                if Path("/sys/fs/cgroup") / actual.lstrip("/") != self.browser_group:
                    raise RuntimeError("WebDriver did not enter its declared browser subgroup")
                self.receipt["browser_subgroup"] = str(self.browser_group)
                self.receipt["browser_limits"] = {key: (self.browser_group / key).read_text().strip() for key in
                    ("memory.max", "memory.high", "memory.swap.max", "cpu.max", "pids.max")}
            self.receipt["state"] = "ready"
        except BaseException as error:
            self.receipt["startup_failure_type"] = type(error).__name__
            self.close()
            raise

    def _call(self, method, path, body=None):
        data = None if body is None else json.dumps(body).encode()
        req = urllib.request.Request(self.endpoint + path, method=method, data=data,
                                     headers={"Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(req, timeout=50) as response:
                raw = response.read(2 * MAX_BODY + 1)
                if len(raw) > 2 * MAX_BODY:
                    raise RuntimeError("WebDriver response exceeds bound")
                return json.loads(raw)["value"]
        except urllib.error.HTTPError as error:
            # Session creation carries public capabilities only, before any
            # WebAuthn bootstrap/credentials. Retain its bounded cause while
            # suppressing all later diagnostics that can echo secrets.
            if path == "/session" and method == "POST":
                try:
                    failure = json.loads(error.read(16385))
                    self.receipt["startup_driver_message"] = str(failure.get("value", {}).get("message", ""))[:4096]
                except (ValueError, AttributeError):
                    pass
            # Driver diagnostics can echo command arguments including secrets.
            raise RuntimeError(f"WebDriver command failed HTTP {error.code}") from None

    def _wd(self, path, body=None, method="POST"):
        return self._call(method, "/session/" + self.session + path, body)

    def _js(self, script, *args, asynchronous=False):
        return self._wd("/execute/async" if asynchronous else "/execute/sync", {"script": script, "args": args})

    def _cdp(self, command, body):
        return self._wd("/goog/cdp/execute", {"cmd": command, "params": body})

    def _storage_check(self):
        size = sum((Path(base) / name).lstat().st_size
                   for root in (self.work, self.browser_temporary) if root is not None
                   for base, _, names in os.walk(root, followlinks=False) for name in names)
        if size > MAX_PROFILE:
            raise RuntimeError("owned browser profile exceeds 512 MiB")

    def _request(self, path, method="GET", body=None, csrf=None):
        validate_route(path, method)
        raw = None if body is None else json.dumps(body)
        if raw is not None and len(raw.encode()) > MAX_REQUEST:
            raise ValueError("campaign command exceeds 1 MiB")
        self._storage_check()
        start = time.monotonic()
        answer = self._js('''const [path,method,body,csrf,cap]=arguments;const done=arguments[arguments.length-1];
          const controller=new AbortController();const timer=setTimeout(()=>controller.abort(),15000);
          (async()=>{try{const headers={'x-fabric-client-version':'1'};if(csrf)headers['x-fabric-csrf']=csrf;
          if(body!==null)headers['content-type']='application/json';
          const response=await fetch(path,{method,headers,body:body===null?undefined:body,credentials:'same-origin',cache:'no-store',redirect:'error',signal:controller.signal});
          const reader=response.body.getReader();let size=0;const chunks=[];while(true){const part=await reader.read();if(part.done)break;size+=part.value.length;if(size>cap){controller.abort();throw Error('body bound');}chunks.push(part.value);}
          const bytes=new Uint8Array(size);let offset=0;for(const part of chunks){bytes.set(part,offset);offset+=part.length;}
          done({status:response.status,text:new TextDecoder('utf-8',{fatal:true}).decode(bytes)});
          }catch(e){done({error:true});}finally{clearTimeout(timer);}})();''', path, method, raw, csrf, MAX_BODY, asynchronous=True)
        if answer.get("error"):
            raise RuntimeError("console transport failed, expired or exceeded body bound")
        self.records.append({"path": path, "method": method, "status": answer["status"],
                             "duration_s": time.monotonic() - start})
        if len(self.records) > 10000:
            raise RuntimeError("campaign request receipt exceeds 10000 entries")
        # Preserve all u64 integers by decoding response text only in Python.
        return answer["status"], json.loads(answer["text"]) if answer["text"] else None

    def session_view(self):
        with self.lock:
            status, view = self._request("/v1/console/session")
            if status != 200 or view.get("api_version") != 1:
                raise RuntimeError("current browser session unavailable; login explicitly after restart")
            return view

    def _ceremony(self, action, body):
        status, start = self._request(f"/v1/console/auth/{action}/start", "POST", body)
        if status != 200:
            raise RuntimeError(f"passkey ceremony start denied HTTP {status}")
        result = self._js('''const [register,options]=arguments;const done=arguments[arguments.length-1];
          window.fabricPasskey(register,options).then(value=>done({credential:value})).catch(()=>done({error:true}));''',
                          action == "register", json.dumps(start["public_key"]), asynchronous=True)
        if result.get("error"):
            raise RuntimeError("real browser passkey ceremony failed")
        status, _ = self._request(f"/v1/console/auth/{action}/finish", "POST", {
            "ceremony_id": start["ceremony_id"], "credential": json.loads(result["credential"])})
        if status != 200:
            raise RuntimeError(f"passkey ceremony finish denied HTTP {status}")
        view = self.session_view()
        self.principal_id = view["principal_id"]
        if view["fresh_until_unix_s"] <= int(time.time()):
            raise RuntimeError("user verification did not establish current freshness")
        return view

    def bootstrap_owner(self, protected_bootstrap, display_name="Campaign fixture owner"):
        with self.lock:
            return self._ceremony("register", {"bootstrap_secret": protected_secret(protected_bootstrap),
                                               "display_name": display_name})

    def login(self):
        """Reauthenticate the retained authenticator after restart or long outage."""
        with self.lock:
            if not self.principal_id:
                raise RuntimeError("bootstrap the owner before login")
            polling = self.ui_polling
            if polling:
                self.poll_ui(False)
            view = self._ceremony("login", {"principal_id": self.principal_id})
            if polling:
                # Authentication replaces both cookie session and CSRF. Reload
                # the UI session before resuming its requested visible phase.
                self.poll_ui(True)
                self._js("window.fabricCampaignUiProbe.refreshes++")
            return view

    def request(self, path, method="GET", body=None):
        with self.lock:
            validate_route(path, method)
            if path.startswith("/v1/console/auth/"):
                raise ValueError("use the explicit passkey lifecycle methods")
            view = self.session_view()
            if method != "GET" and path != "/v1/console/query" and view["fresh_until_unix_s"] <= int(time.time()) + 60:
                view = self.login()
            return self._request(path, method, body, view["csrf"])

    def issue_workload(self, name, scope, ttl_s):
        return self.request("/v1/console/workloads", "POST", {"name": name, "scope": scope, "ttl_s": ttl_s})

    def poll_ui(self, enabled):
        """Explicitly activate/pause the actual bounded native UI polling view."""
        with self.lock:
            if not enabled:
                self._js("window.fabricCampaignUiProbe.active=false")
                self.ui_polling = False
            if enabled:
                self._js('''const connect=document.querySelector('[data-testid=connect-server]');if(connect)connect.click();else{const check=Array.from(document.querySelectorAll('button')).find(e=>e.textContent.trim()==='Check session');if(check)check.click();}''')
                deadline = time.monotonic() + 15
                while not self._js("return document.body.textContent.includes('Connected') && !document.body.textContent.includes('Working…')"):
                    if time.monotonic() >= deadline:
                        raise RuntimeError("UI session refresh did not finish")
                    time.sleep(.1)
                self.poll_health(require_progress=False)
                self._js("Array.from(document.querySelectorAll('.sidebar .nav-item')).find(e=>e.textContent.includes('Live Tail')).click()")
                self._js("const label=Array.from(document.querySelectorAll('label')).find(e=>e.textContent.trim().startsWith('Signal'));const select=label.querySelector('select');select.value='logs';select.dispatchEvent(new Event('change',{bubbles:true}));")
            self._js('''const wanted=arguments[0]?'Resume 5-second polling':'Pause 5-second polling';const button=Array.from(document.querySelectorAll('button')).find(e=>e.textContent.trim()===wanted);if(button)button.click();''', enabled)
            if not enabled:
                deadline = time.monotonic() + 16
                while self._js("return document.body.textContent.includes('Working…')"):
                    if time.monotonic() >= deadline:
                        raise RuntimeError("paused UI read has not completed")
                    time.sleep(.1)
            else:
                self._js("const p=window.fabricCampaignUiProbe;p.active=true;p.phaseLastAttemptMs=null;p.phases++")
                self.ui_polling = True
                self.ui_previous = None
                self.ui_phase_baseline = self.poll_health(require_progress=False)
                self.await_poll_ready()

    def await_poll_ready(self):
        """Observe one actual native UI body completion within twenty seconds."""
        with self.lock:
            if not self.ui_polling or self.ui_phase_baseline is None:
                raise RuntimeError('visible UI polling must be requested before readiness')
            baseline = self.ui_phase_baseline
            began = time.monotonic()
            while True:
                sample = self.poll_health(require_progress=False, record=False)
                elapsed = time.monotonic() - began
                if (elapsed <= 20 and sample['attempts'] > baseline['attempts']
                        and sample['completed'] > baseline['completed']):
                    self.receipt.setdefault('ui_ready_phases', []).append({
                        'phase': sample['phases'], 'elapsed_s': elapsed,
                        'baseline_attempts': baseline['attempts'],
                        'baseline_completed': baseline['completed'],
                        'ready_attempts': sample['attempts'], 'ready_completed': sample['completed']})
                    return self.poll_health(require_progress=False)
                if elapsed >= 20:
                    raise RuntimeError('actual native UI body readiness exceeded twenty seconds')
                time.sleep(min(.25, 20 - elapsed))

    def poll_health(self, *, require_progress=True, record=True):
        """Bounded actual UI body-consumption/status evidence, not owner API success."""
        with self.lock:
            sample = self._js('''const p=window.fabricCampaignUiProbe;const text=document.body.textContent;
              return {...p,visibility:document.visibilityState,connected:text.includes('Connected'),
                pauseVisible:Array.from(document.querySelectorAll('button')).some(e=>
                  e.textContent.trim()==='Pause 5-second polling'),nowMs:performance.now()};''')
            sample['requested_polling'] = self.ui_polling
            if record:
                if len(self.ui_samples) >= 1280:
                    raise RuntimeError('bounded UI polling receipt exceeds 1280 samples')
                self.ui_samples.append(sample)
                self.receipt['ui_polling'] = self.ui_samples
            if self.ui_polling:
                if (sample['visibility'] != 'visible' or not sample['connected'] or not sample['pauseVisible']
                        or sample['statusFailures'] or sample['transportFailures'] or sample['inflight'] > 1):
                    raise RuntimeError('actual visible UI polling failed or became locked')
                previous = self.ui_previous
                # A just-restored phase needs one five-second poll plus its
                # existing fifteen-second transport deadline before progress
                # is observable. Campaign samples are otherwise sixty seconds.
                if (require_progress and previous is not None
                        and sample['nowMs'] - previous['nowMs'] >= 20000):
                    if (sample['attempts'] <= previous['attempts']
                            or sample['completed'] <= previous['completed']):
                        raise RuntimeError('actual requested UI polling made no observed progress')
                self.ui_previous = sample
            return sample

    def close(self):
        if self.closed:
            return
        self.closed = True
        if self.session:
            try:
                self.poll_health(require_progress=False)
            except (OSError, RuntimeError):
                self.receipt['ui_observation_failed_at_close'] = True
            try:
                self._call("DELETE", "/session/" + self.session)
            except (OSError, RuntimeError):
                pass
        if self.driver:
            try:
                os.killpg(self.driver.pid, signal.SIGTERM)
                self.driver.wait(timeout=10)
            except subprocess.TimeoutExpired:
                os.killpg(self.driver.pid, signal.SIGKILL)
                self.driver.wait(timeout=10)
            except ProcessLookupError:
                pass
        if self.browser_group is not None:
            if "populated 1" in (self.browser_group / "cgroup.events").read_text().splitlines():
                (self.browser_group / "cgroup.kill").write_text("1")
                deadline = time.monotonic() + 5
                while "populated 1" in (self.browser_group / "cgroup.events").read_text().splitlines():
                    if time.monotonic() >= deadline:
                        raise RuntimeError("owned browser subgroup remained populated")
                    time.sleep(.1)
            self.receipt["browser_resources"] = {key: (self.browser_group / key).read_text().strip() for key in
                ("memory.peak", "memory.events", "cpu.stat", "cgroup.events")}
        if self.log:
            self.log.close()
        shutil.rmtree(self.work)
        if self.browser_temporary is not None:
            shutil.rmtree(self.browser_temporary)
        self.receipt["state"] = "closed"
        self.receipt["cleanup"] = "owned driver/browser stopped; profile/cache removed; caller owns server"
        (self.out / "receipt.json").write_text(json.dumps(self.receipt, indent=2) + "\n")

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--origin", required=True)
    parser.add_argument("--leaf", type=Path, required=True)
    parser.add_argument("--socket", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--seconds", type=int, default=300)
    parser.add_argument("--browser-group", type=Path)
    args = parser.parse_args()
    if not 1 <= args.seconds <= 7200:
        raise ValueError("finite bridge duration must be 1–7200 seconds")
    scratch = Path(os.environ["FABRIC_SCRATCH_ROOT"]).resolve(strict=True)
    path = args.socket
    if (not path.is_absolute() or path.exists() or path.is_symlink()
            or not path.parent.resolve(strict=True).is_relative_to(scratch)
            or stat.S_IMODE(path.parent.stat().st_mode) != 0o700):
        raise ValueError("fresh socket in an owned mode0700 scratch directory required")
    with ConsoleBridge(args.origin, args.leaf, path.parent / "browser", args.out, browser_group=args.browser_group) as bridge:
        with socket.socket(socket.AF_UNIX) as server:
            server.bind(str(path)); path.chmod(0o600); server.listen(1); server.settimeout(1)
            deadline = time.monotonic() + args.seconds
            try:
                while time.monotonic() < deadline:
                    bridge._storage_check()
                    try:
                        client, _ = server.accept()
                    except socket.timeout:
                        continue
                    with client:
                        _, uid, _ = struct.unpack("3i", client.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
                        if uid != os.getuid():
                            continue
                        client.settimeout(20)
                        payload = bytearray()
                        while not payload.endswith(b"\n"):
                            part = client.recv(min(65536, MAX_REQUEST + 1 - len(payload)))
                            if not part:
                                raise ValueError("incomplete bridge command")
                            payload.extend(part)
                            if len(payload) > MAX_REQUEST:
                                raise ValueError("bridge command exceeds bound")
                        try:
                            command = json.loads(payload)
                            operation = command["operation"]
                            if operation == "bootstrap": result = bridge.bootstrap_owner(command["bootstrap_path"])
                            elif operation == "login": result = bridge.login()
                            elif operation == "request": result = bridge.request(command["path"], command.get("method", "GET"), command.get("body"))
                            elif operation == "poll": bridge.poll_ui(bool(command["enabled"])); result = {"polling": command["enabled"]}
                            elif operation == "stop": break
                            else: raise ValueError("unknown bridge operation")
                            # This protected local response can contain one-time credentials.
                            client.sendall((json.dumps({"result": result}) + "\n").encode())
                        except (ValueError, RuntimeError, KeyError, OSError) as error:
                            client.sendall((json.dumps({"error": type(error).__name__}) + "\n").encode())
            finally:
                path.unlink(missing_ok=True)


if __name__ == "__main__":
    main()
