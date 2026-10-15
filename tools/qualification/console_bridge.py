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
sys.path.insert(0, str(ROOT / 'tools/ui'))
from fixture_cadence import FixtureCadence, PEER_ADMISSION

VERSION = "155.0.8059.39"
TOOLS = STORAGE / "tools/ui" / ("chrome-for-testing-" + VERSION)
FIREFOX_VERSION = "157.0"
GECKODRIVER = STORAGE / "tools/ui/geckodriver"
CERTUTIL = STORAGE / "tools/ui/nss-tools-3.129.0-1.fc44/usr/bin/certutil"
MAX_BODY = 8 * 1024**2
MAX_REQUEST = 1024**2
MAX_PROFILE = 512 * 1024**2


def validate_route(path, method):
    if method not in ("GET", "POST", "PUT") or not isinstance(path, str) or not re.fullmatch(
            r"/v1/console/[A-Za-z0-9_/-]{1,1024}", path):
        raise ValueError("only canonical production console routes are allowed")
    if any(part in ("", ".", "..") for part in path.split("/")[1:]):
        raise ValueError("noncanonical console route")


def bridge_origin(origin, backend, private_dns):
    parsed = urllib.parse.urlsplit(origin)
    common = (parsed.scheme == "https" and parsed.port and not any(
        (parsed.username, parsed.password, parsed.path, parsed.query, parsed.fragment)))
    if backend == "chrome":
        valid = parsed.hostname == "localhost" and private_dns is None
    elif backend == "firefox":
        valid = (isinstance(private_dns, str) and parsed.hostname == private_dns
                 and re.fullmatch(r"(?=.{1,253}$)(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z]{2,63}", private_dns))
    else:
        valid = False
    if not common or not valid:
        raise ValueError("exact owned HTTPS browser origin and confined DNS mapping required")
    return parsed


def firefox_capabilities(profile, hostname):
    return {"browserName": "firefox", "acceptInsecureCerts": False,
            "moz:firefoxOptions": {"binary": "/usr/bin/firefox", "args": ["-headless", "-profile", str(profile)],
                "prefs": {"security.enterprise_roots.enabled": False,
                          "security.webauth.webauthn_enable_softtoken": True,
                          "security.webauth.webauthn_enable_usbtoken": False,
                          "network.dns.localDomains": hostname, "network.trr.mode": 5,
                          "network.proxy.type": 0}}}


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
    supervised_cleanup = True  # Capability; each R3 instance must select it explicitly.
    """One browser, one serialized request, ephemeral owner and authenticator."""
    def __init__(self, origin, leaf_certificate, work, out, *, browser_group=None,
                 browser_parent=None, browser_deadline=None, backend="chrome",
                 trusted_ca=None, private_dns=None):
        bridge_origin(origin, backend, private_dns)
        if backend == "chrome" and trusted_ca is not None:
            raise ValueError("trusted CA requires the Firefox backend")
        if backend == "firefox" and (trusted_ca is None or browser_parent is None or browser_group is None):
            raise ValueError("Firefox requires a public CA and supervised browser budget")
        self.backend, self.authenticators = backend, []
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
        if backend == "firefox" and int((self.browser_group / "memory.max").read_text()) > 1024**3:
            raise ValueError("Firefox walkthrough browser budget must be at most1GiB")
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
        self.supervisor = None
        self.log = None
        self.browser_temporary = None
        self.principal_id = None
        self.closed = False
        self.ui_polling = False
        self.ui_samples = []
        self.ui_previous = None
        self.ui_phase_baseline = None
        self.records = []
        self.receipt = {"state": "starting", "origin": origin, "browser": VERSION if backend == "chrome" else FIREFOX_VERSION,
                        "backend": backend,
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
            if backend == "firefox":
                ca = Path(trusted_ca)
                if (not ca.is_absolute() or ca.is_symlink() or not ca.resolve().is_relative_to(STORAGE)
                        or not ca.is_file() or not 1 <= ca.stat().st_size <= 65536):
                    raise ValueError("bounded public DATA CA certificate required")
                ca_raw = ca.read_bytes()
                if ca_raw.count(b"-----BEGIN CERTIFICATE-----") != 1 or b"PRIVATE KEY" in ca_raw:
                    raise ValueError("one public CA certificate only")
                constraint = subprocess.run(["openssl", "x509", "-noout", "-ext", "basicConstraints"],
                    input=ca_raw, check=True, capture_output=True, timeout=10).stdout
                if b"CA:TRUE" not in constraint:
                    raise ValueError("actual CA basic constraint required")
                profile = self.work / "profile"; profile.mkdir(mode=0o700)
                ca_copy = self.work / "trusted-ca.pem"; ca_copy.write_bytes(ca_raw); ca_copy.chmod(0o600)
                for cmd in ([str(CERTUTIL), "-N", "--empty-password", "-d", "sql:" + str(profile)],
                            [str(CERTUTIL), "-A", "-d", "sql:" + str(profile), "-n", "Owned Fabric fixture CA",
                             "-t", "C,,", "-i", str(ca_copy)]):
                    subprocess.run(cmd, check=True, capture_output=True, timeout=10)
                self.receipt.update(tls_scope="isolated NSS public CA; no bypass; exact hostname",
                                    ca_sha256=hashlib.sha256(ca_raw).hexdigest(), private_dns=private_dns)
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
            if backend == "firefox":
                command = [str(GECKODRIVER), "--host", "127.0.0.1", "--port", str(driver_port), "--allow-system-access"]
            if browser_parent is not None:
                if self.browser_group is None or browser_parent['path'] != self.browser_group or browser_deadline is None:
                    raise ValueError('R3 supervisor needs the actual held browser budget and run deadline')
                from browser_supervisor import BrowserSupervisor
                self.supervisor = BrowserSupervisor(command, env, self.log, browser_deadline,
                                                     self.out / 'supervisor', browser_parent=browser_parent)
                self.driver = self.supervisor
                self.receipt['supervised_browser'] = True
            elif self.browser_group is not None:
                helper = Path(__file__).parent / "enter_group.py"
                if not helper.is_file():
                    helper = ROOT / "tools/bench/labs/completion/enter_group.py"
                command = [sys.executable, str(helper), str(self.browser_group), *command]
            if self.supervisor is None:
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
            capabilities = {"browserName": "chrome", "goog:chromeOptions": {
                "binary": str(TOOLS / "chrome-linux64/chrome"), "args": [
                    "--headless=new", "--disable-dev-shm-usage", "--no-first-run",
                    "--no-default-browser-check", "--user-data-dir=" + str(self.work / "profile"),
                    "--ignore-certificate-errors-spki-list=" + spki, "--window-size=1440,1100"]}}
            if backend == "firefox":
                capabilities = firefox_capabilities(self.work / "profile", private_dns)
            info = self._call("POST", "/session", {"capabilities": {"alwaysMatch": capabilities}})
            self.session = info["sessionId"]
            actual = info["capabilities"]
            self.receipt['browser_capabilities_actual'] = {
                key: actual.get(key) for key in ('browserName', 'browserVersion', 'acceptInsecureCerts')}
            if actual["browserVersion"] != (VERSION if backend == "chrome" else FIREFOX_VERSION):
                raise RuntimeError("browser differs from reviewed version")
            if backend == "firefox" and actual.get("acceptInsecureCerts") is not False:
                raise RuntimeError("Firefox did not retain strict certificate verification")
            self.receipt["browser_version_actual"] = actual["browserVersion"]
            self._wd("/timeouts", {"script": 45000, "pageLoad": 30000, "implicit": 0})
            if backend == "chrome":
                self._cdp("WebAuthn.enable", {})
            self.add_authenticator()
            self._tls_startup_pending = True
            self._wd("/url", {"url": origin + "/console/"})
            deadline = time.monotonic() + 30
            while not self._js("return typeof window.fabricPasskey==='function'"):
                if time.monotonic() >= deadline:
                    raise RuntimeError("production console passkey helper did not load")
                time.sleep(.1)
            self._tls_startup_pending = False
            # Observe only native UI Request objects, never bridge string-based
            # requests, response contents, cookies or credentials. Unlike the
            # default Resource Timing buffer these counters cannot fill up.
            self._js('''window.fabricCampaignUiProbe={active:false,attempts:0,headers:0,completed:0,
              statusFailures:0,transportFailures:0,inflight:0,lastAttemptMs:null,lastCompletedMs:null,
              maxAttemptGapMs:0,refreshes:0,phases:0,phaseLastAttemptMs:null,
              documentId:crypto.randomUUID(),events:[],eventCount:0,eventOverflow:false,tickCount:0,latestTick:null};
              window.fabricCampaignUiEvent=function(event){const p=window.fabricCampaignUiProbe;
                event.seq=++p.eventCount;if(p.events.length<8192)p.events.push(event);else p.eventOverflow=true;};
              const nativeInterval=window.setInterval;let observedTimer=0;
              window.setInterval=function(callback,delay,...args){if(delay!==5000)return nativeInterval.call(this,callback,delay,...args);
                const timer=++observedTimer;window.fabricCampaignUiEvent({event:'timer',timer,delay,atMs:performance.now()});
                return nativeInterval.call(this,function(...values){const p=window.fabricCampaignUiProbe;
                  const check=Array.from(document.querySelectorAll('button')).find(e=>e.textContent.trim()==='Check session');
                  const tick=++p.tickCount;p.latestTick=tick;
                  window.fabricCampaignUiEvent({event:'tick',timer,tick,atMs:performance.now(),
                    visibility:document.visibilityState,busy:check?check.disabled:null,
                    connected:document.body.textContent.includes('Connected'),
                    running:Array.from(document.querySelectorAll('button')).some(e=>e.textContent.trim()==='Pause 5-second polling')});
                  return callback.apply(this,values);},delay,...args);};
              const original=window.fetch;window.fetch=function(...args){
                const p=window.fabricCampaignUiProbe;
                const native=p.active && args[0] instanceof Request &&
                  new URL(args[0].url).pathname==='/v1/console/query';
                if(!native)return original.apply(this,args);
                const now=performance.now();if(p.phaseLastAttemptMs!==null)
                  p.maxAttemptGapMs=Math.max(p.maxAttemptGapMs,now-p.phaseLastAttemptMs);
                p.phaseLastAttemptMs=now;p.lastAttemptMs=now;p.attempts++;p.inflight++;
                const id=p.attempts;window.fabricCampaignUiEvent({event:'attempt',id,tick:p.latestTick,atMs:now});
                let finished=false;const finish=(failed)=>{if(finished)return;finished=true;
                  p.inflight--;window.fabricCampaignUiEvent({event:'complete',id,atMs:performance.now(),failed});
                  if(failed)p.transportFailures++;else{p.completed++;p.lastCompletedMs=performance.now();}};
                return original.apply(this,args).then(response=>{
                  p.headers++;window.fabricCampaignUiEvent({event:'headers',id,atMs:performance.now(),status:response.status});
                  if(response.status<200||response.status>=300)p.statusFailures++;
                  if(!response.body){finish(false);return response;}
                  const stream=response.body;const getReader=stream.getReader.bind(stream);
                  stream.getReader=function(...options){const reader=getReader(...options);
                    const read=reader.read.bind(reader);reader.read=function(...values){
                      return read(...values).then(part=>{if(part.done)finish(false);return part;},
                        error=>{finish(true);throw error;});};return reader;};
                  return response;
                },error=>{finish(true);throw error;});};''')
            if self.browser_group is not None:
                driver_pid = self.supervisor.receipt['ready']['driver_pid'] if self.supervisor else self.driver.pid
                actual = next(line[3:] for line in Path(f"/proc/{driver_pid}/cgroup").read_text().splitlines() if line.startswith("0::"))
                actual_group = Path("/sys/fs/cgroup") / actual.lstrip("/")
                if ((self.supervisor is None and actual_group != self.browser_group) or
                        (self.supervisor is not None and not actual_group.is_relative_to(self.supervisor.domain['path']))):
                    raise RuntimeError("WebDriver did not enter its declared browser subgroup")
                self.receipt['actual_driver_group'] = str(actual_group)
                self.receipt["browser_subgroup"] = str(self.browser_group)
                self.receipt["browser_limits"] = {key: (self.browser_group / key).read_text().strip() for key in
                    ("memory.max", "memory.high", "memory.swap.max", "cpu.max", "pids.max")}
            self.receipt["state"] = "ready"
        except BaseException as error:
            self.receipt["startup_failure_type"] = type(error).__name__
            if backend == "firefox" and self.session and getattr(self, "_tls_startup_pending", False):
                try:
                    self.receipt["tls_navigation_failure"] = self._js("""const text=document.documentElement.textContent;
                      return {uri:document.documentURI.slice(0,2048), codes:
                        ['SSL_ERROR_BAD_CERT_DOMAIN','SEC_ERROR_UNKNOWN_ISSUER','MOZILLA_PKIX_ERROR_SELF_SIGNED_CERT']
                        .filter(code=>text.includes(code))};""")
                except (OSError, RuntimeError):
                    self.receipt["tls_navigation_failure"] = {"codes": [], "inconclusive": True}
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

    def _gecko_auth(self, expression, *arguments):
        self._wd("/moz/context", {"context": "chrome"})
        try:
            return self._js('const auth=ChromeUtils.importESModule("chrome://remote/content/marionette/webauthn.sys.mjs").webauthn;' + expression, *arguments)
        finally:
            self._wd("/moz/context", {"context": "content"})

    def add_authenticator(self):
        options = {"protocol": "ctap2", "transport": "internal", "hasResidentKey": True,
                   "hasUserVerification": True, "isUserVerified": True}
        if self.backend == "firefox":
            options.update(protocol="ctap2_1", transport="usb", isUserConsenting=True)
            identifier = self._gecko_auth('return auth.addVirtualAuthenticator(arguments[0])', options)
        else:
            options["automaticPresenceSimulation"] = True
            identifier = self._cdp("WebAuthn.addVirtualAuthenticator", {"options": options})["authenticatorId"]
        self.authenticators.append(identifier)
        self.receipt.setdefault("virtual_devices", []).append({"event": "added", "ordinal": 1 + sum(row["event"] == "added" for row in self.receipt.get("virtual_devices", [])),
                                                               "identifier": identifier,
                                                               "resident": True, "uv_configured": True})
        return identifier

    def remove_authenticator(self, identifier):
        if identifier not in self.authenticators:
            raise ValueError("owned virtual authenticator required")
        if self.backend == "firefox":
            self._gecko_auth('auth.removeVirtualAuthenticator(arguments[0]);return null', identifier)
        else:
            self._cdp("WebAuthn.removeVirtualAuthenticator", {"authenticatorId": identifier})
        self.authenticators.remove(identifier)
        self.receipt.setdefault("virtual_devices", []).append({"event": "removed", "identifier": identifier})

    def replace_authenticator(self):
        for identifier in list(self.authenticators):
            self.remove_authenticator(identifier)
        return self.add_authenticator()

    def resident_credentials(self, identifier):
        if identifier not in self.authenticators:
            raise ValueError("owned virtual authenticator required")
        if self.backend == "firefox":
            credentials = self._gecko_auth('return auth.getCredentials(arguments[0]).map(c=>({resident:c.isResidentCredential,rp:c.rpId}))', identifier)
        else:
            credentials = [{"resident": value.get("isResidentCredential"), "rp": value.get("rpId")}
                           for value in self._cdp("WebAuthn.getCredentials", {"authenticatorId": identifier})["credentials"]]
        public = [{"resident": value.get("resident"), "rp": value.get("rp")} for value in credentials]
        return {"count": len(public), "credentials": public, "uv_configured": True}

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
            observer = getattr(self, 'raw_observer', None)
            if observer is not None:
                observer({'path': path, 'method': method, 'answer': answer,
                          'ended_ns': time.monotonic_ns()})
            raise RuntimeError("console transport failed, expired or exceeded body bound")
        observer = getattr(self, 'raw_observer', None)
        if observer is not None:
            # Credential-issuing routes are intentionally excluded: the R3
            # consumer attaches this sink only after all setup credentials exist.
            observer({'path': path, 'method': method, 'answer': answer,
                      'ended_ns': time.monotonic_ns()})
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
            continuous = polling and getattr(self, 'continuous_polling', False)
            if polling and not continuous:
                self.poll_ui(False)
            view = self._ceremony("login", {"principal_id": self.principal_id})
            if polling:
                # Authentication replaces both cookie session and CSRF. Reload
                # the UI session before resuming its requested visible phase.
                if continuous:
                    # R3 keeps the actual view running while the real passkey
                    # ceremony replaces the cookie. Update application session
                    # state through its native control; preserve every inflight
                    # answer and the original phase/cadence/event counters.
                    self._js('''const button=Array.from(document.querySelectorAll('button')).find(e=>
                      e.textContent.trim()==='Check session');if(!button)throw Error('native session control missing');button.click();''')
                    deadline = time.monotonic() + 15
                    while not self._js("return document.body.textContent.includes('Connected') && !document.body.textContent.includes('Working…')"):
                        if time.monotonic() >= deadline:
                            raise RuntimeError('continuous native UI session refresh did not finish')
                        time.sleep(.1)
                    self.poll_health(require_progress=False)
                else:
                    self.poll_ui(True)
                self._js("window.fabricCampaignUiProbe.refreshes++")
            return view

    def request(self, path, method="GET", body=None):
        with self.lock:
            validate_route(path, method)
            if path.startswith("/v1/console/auth/"):
                raise ValueError("use the explicit passkey lifecycle methods")
            # A command sends a session GET and the requested operation. The
            # peer admits eight requests/s; two commands/s leave room for the
            # registered three visibility probes/s and five-second UI poll.
            # Serialize the pacing with the command, and keep every refusal.
            if not hasattr(self, '_private_cadence'):
                self._private_cadence = FixtureCadence(
                    lambda: time.monotonic(), lambda delay: time.sleep(delay),
                    interval=PEER_ADMISSION['private']['command_interval_s'])
            self._private_cadence.pace()
            view = self.session_view()
            r3_control = getattr(self, 'r3_control_session', False) and (
                (method == 'GET' and path == '/v1/console/nodes') or
                (method == 'PUT' and re.fullmatch(r'/v1/console/nodes/sim[0-9]{4}/config', path)))
            if method != "GET" and path != "/v1/console/query" and not r3_control and view["fresh_until_unix_s"] <= int(time.time()) + 60:
                view = self.login()
            return self._request(path, method, body, view["csrf"])

    def issue_reader(self, name, ttl_s):
        return self.request("/v1/console/readers", "POST", {"name": name, "ttl_s": ttl_s})

    def poll_ui(self, enabled):
        """Explicitly activate/pause the actual bounded native UI polling view."""
        with self.lock:
            if not enabled:
                self._js("window.fabricCampaignUiEvent({event:'polling',active:false,atMs:performance.now()})")
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
                self._js("window.fabricCampaignUiEvent({event:'polling',active:true,atMs:performance.now()})")
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
            sample = self._js('''const p=window.fabricCampaignUiProbe;const {events,...summary}=p;const text=document.body.textContent;
              return {...summary,visibility:document.visibilityState,connected:text.includes('Connected'),
                pauseVisible:Array.from(document.querySelectorAll('button')).some(e=>
                  e.textContent.trim()==='Pause 5-second polling'),nowMs:performance.now()};''')
            sample['requested_polling'] = self.ui_polling
            if record:
                if len(self.ui_samples) >= getattr(self, 'ui_sample_limit', 1280):
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

    def continuity_snapshot(self):
        """Bind newly observed native UI events to a host monotonic bracket.

        This captures evidence; the independent R3 validator checks complete
        sequence/cadence/body/status/session/document continuity later. A native
        timestamp bracket is uncertainty, never exact browser clock precision.
        """
        with self.lock:
            before = time.monotonic_ns()
            cursor = getattr(self, '_continuity_cursor', 0)
            value = self._js('''const p=window.fabricCampaignUiProbe;
              return {documentId:p.documentId,timeOriginMs:performance.timeOrigin,nowMs:performance.now(),
                visibility:document.visibilityState,active:p.active,phases:p.phases,refreshes:p.refreshes,
                eventCount:p.eventCount,eventOverflow:p.eventOverflow,
                events:p.events.filter(e=>e.seq>arguments[0])};''', cursor)
            after = time.monotonic_ns()
            value.update(mono_before_ns=before, mono_after_ns=after, webdriver_session=self.session)
            if value['eventOverflow']:
                raise RuntimeError('native UI attempt ledger exceeded 8192 events')
            self._continuity_cursor = value['eventCount']
            return value

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
        supervisor = getattr(self, 'supervisor', None)
        if supervisor is not None:
            supervisor.close(deadline=getattr(self, 'shutdown_deadline', None))
            if not supervisor.cleanup_confirmed:
                raise RuntimeError('browser supervisor stop not confirmed; whole campaign stop required')
            self.receipt['supervised_cleanup'] = supervisor.receipt
        elif self.driver:
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
                if supervisor is not None:
                    raise RuntimeError('supervised browser budget remains populated; whole campaign stop required')
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
