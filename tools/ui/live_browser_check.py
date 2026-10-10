"""Finite virtual-passkey checks against the production TLS server and Spindle."""
import argparse
import base64
import hashlib
import json
import os
import re
from pathlib import Path
import secrets
import shutil
import signal
import socket
import ssl
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request
from browser_producer import TRACE_ID, traces

ROOT = Path(__file__).resolve().parents[2]
DATA = Path('/run/media/kmosoti/data/FabricO11y')
sys.path.insert(0, str(ROOT / 'tools'))
from resource_group import require_limits
sys.path.insert(0, str(ROOT / 'tools/packaging'))
from stage_console import stage

VERSION = '155.0.8059.39'
TOOLS = DATA / 'tools/ui' / f'chrome-for-testing-{VERSION}'


def semantic_rejection(answer, status, message):
    code, body = answer
    return code == status and isinstance(body, dict) and body.get('error') == message


def free_port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--build', type=Path, required=True)
    parser.add_argument('--server', type=Path, required=True)
    parser.add_argument('--spindle', type=Path, required=True)
    parser.add_argument('--packaged-console',type=Path,help='use extracted package assets after cross-checking the frozen build')
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--inject-rejection-defect',choices=('error','status'))
    parser.add_argument('--inject-api-cache-defect', action='store_true')
    parser.add_argument('--browser', choices=('chrome','firefox'), default='chrome')
    parser.add_argument('--certutil',type=Path,default=Path(shutil.which('certutil') or str(DATA/'tools/ui/nss-tools-3.129.0-1.fc44/usr/bin/certutil')))
    parser.add_argument('--expired-cursor-probe',action='store_true')
    parser.add_argument('--recovery',action='store_true')
    parser.add_argument('--shell-lifecycle',action='store_true')
    parser.add_argument('--pressure',action='store_true',help='finite authenticated body/admission timeout probe')
    parser.add_argument('--poll-seconds',type=int,choices=(0,120),default=0)
    args = parser.parse_args()
    limits = require_limits()
    # The combined browser/server fixture uses a stricter cap than build jobs.
    subprocess.run(['systemctl','--user','set-property','--runtime',limits.name,
                    'MemoryHigh=3500000000','MemoryMax=4000000000','MemorySwapMax=0',
                    'CPUQuota=200%','TasksMax=512'], check=True, capture_output=True)
    memory_max = int((limits/'memory.max').read_text().strip())
    if not 3999995904 <= memory_max <= 4000000000 or (limits/'memory.swap.max').read_text().strip()!='0':
        raise SystemExit('combined fixture 4 GB cap was not enforced')
    out = args.out.resolve()
    build = args.build.resolve()
    if not out.is_relative_to(DATA / 'results') or out.exists() or not build.is_relative_to(DATA / 'results'):
        raise SystemExit('fresh owned data-drive result directories required')
    for binary in (args.server, args.spindle):
        if not binary.resolve().is_relative_to(DATA) or not binary.is_file():
            raise SystemExit('project binaries must be existing data-drive artifacts')
    out.mkdir()
    work = Path(os.environ['FABRIC_SCRATCH_ROOT']) / 'live-console-browser'
    work.mkdir(mode=0o700)
    checks = []
    receipt = {'command': sys.argv, 'exit': 1, 'checks': checks, 'build': str(build),
               'browser':args.browser,'browser_version_expected': VERSION if args.browser=='chrome' else '157.0', 'fixture_storage': str(work), 'combined_memory_cap_bytes':4000000000,'fixture_storage_cap_bytes':512*1024*1024,
               'enforced_limits':{key:(limits/key).read_text().strip() for key in ('memory.max','memory.high','memory.swap.max','cpu.max','pids.max')},
               'scope': 'finite virtual CTAP2 desktop/narrow browser integration; not hardware/mobile/installed-app qualification',
               'tls_scope': 'exact fixture leaf SPKI trusted by this isolated browser only; Spindle trusts fixture CA',
               'injected_api_cache_defect': args.inject_api_cache_defect,
               'injected_rejection_defect':args.inject_rejection_defect,
               'source_sha256': {str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest()
                                 for p in (Path(__file__), ROOT / 'tools/ui/browser_producer.py', ROOT / 'tools/packaging/stage_console.py')},
               'binaries': {str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in (args.server, args.spindle)},
               'browser_download': json.loads((TOOLS / 'download.json').read_text()) if args.browser=='chrome' else json.loads((DATA/'tools/ui/geckodriver.json').read_text())}
    processes = []
    logs = []
    session = None
    driver_port = free_port()
    endpoint = f'http://127.0.0.1:{driver_port}'
    server_port = free_port()
    origin = f'https://localhost:{server_port}'
    context = None
    deadline = time.monotonic() + (1200 if args.expired_cursor_probe else 240)
    samples = []
    stop_samples = threading.Event()
    storage_violation = threading.Event()

    def sample():
        while not stop_samples.wait(.5):
            group = limits
            footprint=0
            for path in work.rglob('*'):
                try:
                    if path.is_file():footprint+=path.stat().st_size
                except FileNotFoundError:pass
            entry = {'monotonic_ns': time.monotonic_ns(), 'processes': {}, 'fixture_bytes': footprint}
            if entry['fixture_bytes'] > 512 * 1024 * 1024:
                entry['storage_bound_exceeded'] = True
                storage_violation.set()
            for label, proc in processes:
                try:
                    fields = dict(line.split(':', 1) for line in Path(f'/proc/{proc.pid}/status').read_text().splitlines() if ':' in line)
                    entry['processes'][label] = {key: fields.get(key, '').strip() for key in ('VmRSS', 'VmHWM', 'Threads')}
                except FileNotFoundError:
                    pass
            if group and group.exists():
                entry['cgroup'] = {key: (group / key).read_text().strip() for key in ('memory.current', 'memory.peak', 'memory.events', 'memory.swap.current', 'pids.current', 'cpu.stat')}
            samples.append(entry)

    def wait(predicate, label, seconds=25):
        until = min(deadline, time.monotonic() + seconds)
        while time.monotonic() < until:
            if storage_violation.is_set():raise RuntimeError("owned fixture exceeds 512 MiB storage ceiling")
            if predicate():
                return
            time.sleep(.1)
        raise AssertionError('timeout: ' + label)

    def check(condition, label):
        if not condition:
            raise AssertionError(label)
        checks.append(label)

    def call(method, path, body=None):
        if storage_violation.is_set():raise RuntimeError("owned fixture exceeds 512 MiB storage ceiling")
        data = None if body is None else json.dumps(body).encode()
        request = urllib.request.Request(endpoint + path, data=data, method=method,
                                         headers={'Content-Type': 'application/json'})
        try:
            with urllib.request.urlopen(request, timeout=35) as response:
                return json.load(response).get('value')
        except urllib.error.HTTPError as error:
            raise RuntimeError(error.read().decode()) from error

    def wd(path, body=None, method='POST'):
        return call(method, f'/session/{session}' + path, body)

    def js(script, *values):
        return wd('/execute/sync', {'script': script, 'args': values})

    def async_js(script, *values):
        return wd('/execute/async', {'script': script, 'args': values})

    def click(text):
        button=js('const button=Array.from(document.querySelectorAll("button")).find(e=>e.textContent.trim()===arguments[0]); if(!button)throw Error("button absent:"+arguments[0]);return button', text)
        wd('/element/'+button['element-6066-11e4-a52e-4f735466cecf']+'/click',{})

    def field(label, value):
        js('const label=Array.from(document.querySelectorAll("label")).find(e=>e.textContent.trim().startsWith(arguments[0]));if(!label)throw Error("label absent:"+arguments[0]);const input=label.querySelector("input,textarea,select");input.value=arguments[1];input.dispatchEvent(new Event(input.tagName==="SELECT"?"change":"input",{bubbles:true}))', label, value)

    def nav(text):
        js('Array.from(document.querySelectorAll(".sidebar .nav-item")).find(e=>e.textContent.includes(arguments[0])).click()', text)

    last_api_call = 0.0
    def pace_api():
        nonlocal last_api_call
        delay = .1 - (time.monotonic() - last_api_call)
        if delay > 0: time.sleep(delay)
        last_api_call = time.monotonic()

    def expect(answer, status, message, label):
        code, body = answer
        receipt.setdefault('semantic_rejections', []).append({'label':label,'status':code,'error':body.get('error') if isinstance(body,dict) else None})
        check(semantic_rejection(answer,status,message), label)

    def api(path, method='GET', body=None, version=True):
        query_csrf=None
        if path=='/v1/console/query':
            session_status,session_view=api('/v1/console/session')
            if session_status==200:query_csrf=session_view.get('csrf')
        pace_api()
        # Decode returned text in Python, never JS JSON.parse on telemetry.
        answer = async_js('''const [path,method,body,version,csrf]=arguments;const done=arguments[arguments.length-1];
          const headers={};if(csrf)headers['x-fabric-csrf']=csrf;if(version)headers['x-fabric-client-version']='1';if(body!==null)headers['content-type']='application/json';
          fetch(path,{method,headers,body:body===null?undefined:body,cache:'no-store',credentials:'same-origin'})
          .then(async response=>done({status:response.status,text:await response.text()})).catch(error=>done({error:String(error)}));''',
          path, method, None if body is None else json.dumps(body), version, query_csrf)
        if 'error' in answer:
            raise RuntimeError(answer['error'])
        body=json.loads(answer['text']) if answer['text'] else None
        if answer['status']>=400:receipt.setdefault('api_rejections',[]).append({'path':path.split('?')[0],'status':answer['status'],'error':body.get('error') if isinstance(body,dict) else None})
        if args.inject_rejection_defect and path=='/v1/console/auth/register/start' and body=={'error':'access denied'}:
            receipt['rejection_injection_actual_witness']={'status':answer['status'],'error':body['error']}
            if args.inject_rejection_defect=='error':body={'error':'different rejection mechanism'}
            else:answer['status']=429
        return answer['status'],body

    def bearer_api(token,path,method='GET',body=None):
        pace_api()
        request=urllib.request.Request(origin+path,method=method,data=None if body is None else json.dumps(body).encode(),headers={
            'Authorization':'Bearer '+token,'x-fabric-client-version':'1','Content-Type':'application/json'})
        try:
            with urllib.request.urlopen(request,context=context,timeout=15) as response:
                return response.status,json.load(response)
        except urllib.error.HTTPError as error:
            text=error.read().decode()
            return error.code,json.loads(text) if text else None

    def cdp(command, params):
        return wd('/goog/cdp/execute', {'cmd': command, 'params': params})

    def add_authenticator():
        options={'protocol':'ctap2','transport':'internal','hasResidentKey':True,'hasUserVerification':True,'isUserVerified':True}
        if args.browser=='firefox':
            # Gecko's CTAP 2.0 test token omits the credential ID after filtering
            # an original two-key allowlist to one. Its bridge restores the ID
            # only for an originally single-key list. CTAP 2.1 retains that ID.
            options['protocol']='ctap2_1'
            options['transport']='usb'
            options['isUserConsenting']=True
            # geckodriver #2239 renames fields that Gecko expects in camelCase.
            # Use Gecko's actual virtual authenticator implementation from the
            # privileged fixture context; content still runs real WebAuthn.
            wd('/moz/context',{'context':'chrome'})
            try:
                return js('return ChromeUtils.importESModule("chrome://remote/content/marionette/webauthn.sys.mjs").webauthn.addVirtualAuthenticator(arguments[0])',options)
            finally:
                wd('/moz/context',{'context':'content'})
        options['automaticPresenceSimulation']=True
        return cdp('WebAuthn.addVirtualAuthenticator',{'options':options})['authenticatorId']

    def remove_authenticator(identifier):
        if args.browser=='firefox':
            wd('/moz/context',{'context':'chrome'})
            try:
                return js('ChromeUtils.importESModule("chrome://remote/content/marionette/webauthn.sys.mjs").webauthn.removeVirtualAuthenticator(arguments[0]);return null',identifier)
            finally:
                wd('/moz/context',{'context':'content'})
        return cdp('WebAuthn.removeVirtualAuthenticator',{'authenticatorId':identifier})

    def launch(label, command):
        log = (out / f'{label}.log').open('w')
        logs.append(log)
        environment=os.environ.copy()
        for key in ('XDG_DATA_HOME','XDG_CONFIG_HOME','XDG_CACHE_HOME'):
            directory=work/key.lower();directory.mkdir(exist_ok=True);environment[key]=str(directory)
        proc = subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT, start_new_session=True,env=environment)
        processes.append((label, proc))
        return proc

    sampler = threading.Thread(target=sample, daemon=True)
    sampler.start()
    try:
        artifact_dir=work/'binaries';artifact_dir.mkdir(mode=0o700)
        snapshots={}
        for role in ('server','spindle'):
            source=getattr(args,role)
            destination=artifact_dir/role
            shutil.copyfile(source,destination)
            destination.chmod(0o700)
            identity=hashlib.sha256(destination.read_bytes()).hexdigest()
            if identity!=receipt['binaries'][str(source)]:
                raise ValueError('native binary changed before fixture snapshot')
            snapshots[role]={'source':str(source),'sha256':identity,'bytes':destination.stat().st_size}
            setattr(args,role,destination)
        receipt['immutable_binary_snapshots']=snapshots
        check(semantic_rejection((403,{'error':'access denied'}),403,'access denied') and not semantic_rejection((429,{'error':'access denied'}),403,'access denied') and not semantic_rejection((403,{'error':'wrong error'}),403,'access denied'),'semantic rejection grader rejects wrong status and wrong error negative controls')
        stage(build, work / 'console')
        if args.packaged_console:
            packaged=args.packaged_console.resolve()
            if not packaged.is_relative_to(DATA) or not packaged.is_dir():
                raise ValueError('extracted console must be an existing data-drive directory')
            staged=work/'console'
            if {path.name for path in packaged.iterdir()}!={path.name for path in staged.iterdir()}:
                raise ValueError('extracted package asset inventory differs from frozen build')
            identities={}
            for expected in staged.iterdir():
                source=packaged/expected.name
                if source.is_symlink() or not source.is_file() or source.stat().st_size>8*1024*1024:
                    raise ValueError('unsafe extracted package asset')
                body=source.read_bytes()
                if expected.name in ('console-headers.json','asset-manifest.json'):
                    if json.loads(body)!=json.loads(expected.read_bytes()):
                        raise ValueError('extracted package metadata differs from frozen build')
                elif body!=expected.read_bytes():
                    raise ValueError('extracted package asset differs from frozen build')
                identities[source.name]={'bytes':len(body),'sha256':hashlib.sha256(body).hexdigest()}
                shutil.copyfile(source,expected)
            receipt['extracted_packaged_console']={'path':str(packaged),'assets':identities}
            check(True,'actual extracted package console assets match frozen build before serving')
        if args.inject_api_cache_defect:
            worker=work/'console/service-worker.js'
            worker.write_text(worker.read_text()+'''
self.addEventListener("fetch",event=>{
  if(new URL(event.request.url).pathname.startsWith("/v1/")&&event.request.method==="GET")
    event.respondWith(fetch(event.request).then(async response=>{
      await(await caches.open("fabric-shell-injected-defect")).put(event.request,response.clone());return response;
    }));
});
''')
            manifest_path=work/'console/asset-manifest.json'
            manifest=json.loads(manifest_path.read_text())
            body=worker.read_bytes()
            manifest['service-worker.js']={'bytes':len(body),'sha256':hashlib.sha256(body).hexdigest()}
            manifest_path.write_text(json.dumps(manifest)+'\n')
        (work / 'san.ext').write_text('subjectAltName=DNS:localhost,IP:127.0.0.1\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\n')
        commands = [
            ['req','-x509','-newkey','ec','-pkeyopt','ec_paramgen_curve:P-256','-nodes','-keyout','ca.key','-out','ca.pem','-days','1','-subj','/CN=Fabric browser fixture CA','-addext','basicConstraints=critical,CA:TRUE','-addext','keyUsage=critical,keyCertSign,cRLSign'],
            ['req','-newkey','ec','-pkeyopt','ec_paramgen_curve:P-256','-nodes','-keyout','server.key','-out','server.csr','-subj','/CN=localhost'],
            ['x509','-req','-in','server.csr','-CA','ca.pem','-CAkey','ca.key','-CAcreateserial','-out','server.pem','-days','1','-extfile','san.ext']]
        for command in commands:
            subprocess.run(['openssl', *command], cwd=work, capture_output=True, check=True, timeout=10)
        public = subprocess.run(['openssl','x509','-in',str(work/'server.pem'),'-pubkey','-noout'], capture_output=True, check=True).stdout
        der = subprocess.run(['openssl','pkey','-pubin','-outform','DER'],input=public,capture_output=True,check=True).stdout
        spki = base64.b64encode(hashlib.sha256(der).digest()).decode()
        receipt['fixture_leaf_spki_sha256'] = spki
        admin = work / 'admin-token'
        admin.write_text(secrets.token_hex(32) + '\n')
        admin.chmod(0o600)
        config = work / 'server.conf'
        config.write_text(f'listen=127.0.0.1:{server_port}\ntls_cert={work}/server.pem\ntls_key={work}/server.key\nstate_dir={work}/state\nadmin_token_file={admin}\nconsole_dir={work}/console\naccess_origin={origin}\naccess_rp_id=localhost\njournal_bytes=8388608\njournal_file_bytes=1048576\nretention_s=86400\nretention_bytes=33554432\nseal_workers=1\nself_spindle_url={origin}\nself_spindle_ca={work}/ca.pem\nself_spindle_executable={args.spindle.resolve()}\n')
        server = launch('server', [str(args.server.resolve()), 'serve', str(config)])
        context = ssl.create_default_context(cafile=str(work / 'ca.pem'))
        def healthy():
            if server.poll() is not None:
                raise RuntimeError('server exited; inspect server.log')
            try:
                with urllib.request.urlopen(origin + '/v1/health', context=context, timeout=1) as response:
                    return response.status == 200
            except (OSError, urllib.error.URLError):
                return False
        wait(healthy, 'production TLS server')
        build_receipt=json.loads((build/'build.json').read_text())
        served_assets=[]
        for name,identity in build_receipt['assets'].items():
            if args.inject_api_cache_defect and name=='service-worker.js':
                identity=json.loads((work/'console/asset-manifest.json').read_text())[name]
                receipt['injected_worker_identity']=identity
            with urllib.request.urlopen(origin+'/console/'+name,context=context,timeout=15) as response:
                content=response.read(8*1024*1024+1)
                mime=response.headers.get_content_type()
                expected_mime={'html':'text/html','js':'text/javascript','css':'text/css','wasm':'application/wasm','webmanifest':'application/manifest+json','png':'image/png','svg':'image/svg+xml'}[name.rsplit('.',1)[1]]
                # HTTP permits application/javascript as the legacy registered JS MIME.
                correct_mime=mime==expected_mime or expected_mime=='text/javascript' and mime=='application/javascript'
                check(len(content)==identity['bytes'] and hashlib.sha256(content).hexdigest()==identity['sha256'] and correct_mime,'actual TLS shell asset hash size and MIME: '+name)
                check(all(response.headers.get(header)==value for header,value in build_receipt['headers'].items()),'actual shell headers match frozen build: '+name)
                served_assets.append({'name':name,'bytes':len(content),'mime':mime})
        receipt['served_assets']=served_assets
        with urllib.request.urlopen(origin+'/console/manifest.webmanifest',context=context,timeout=5) as response:pwa_manifest=json.load(response)
        check(pwa_manifest['scope']=='/console/' and pwa_manifest['start_url']=='/console/','actual manifest limits shell scope and start URL')
        bootstrap_file = work / 'state/access/access-bootstrap.secret'
        wait(lambda: bootstrap_file.is_file(), 'one-time bootstrap')
        bootstrap = bootstrap_file.read_text().strip()
        if args.browser=='firefox':
            profile=work/'profile';profile.mkdir()
            certutil=args.certutil.resolve()
            for command in ([str(certutil),'-N','--empty-password','-d','sql:'+str(profile)],
                            [str(certutil),'-A','-d','sql:'+str(profile),'-n','Owned Fabric fixture CA','-t','C,,','-i',str(work/'ca.pem')]):
                subprocess.run(command,check=True,capture_output=True,timeout=10)
            receipt['tls_scope']='fixture CA trusted only in owned Firefox NSS profile; acceptInsecureCerts false; Spindle trusts fixture CA'
            receipt['virtual_authenticator_driver_workaround']='geckodriver2239 camelCase mismatch; privileged fixture invokes Gecko WebAuthn module, then restores content context'
            driver=launch('webdriver',[str(DATA/'tools/ui/geckodriver'),'--host','127.0.0.1','--port',str(driver_port),'--allow-system-access'])
        else:
            driver = launch('webdriver', [str(TOOLS / 'chromedriver-linux64/chromedriver'), '--port=' + str(driver_port), '--allowed-ips=127.0.0.1'])
        def driver_ready():
            if driver.poll() is not None:
                raise RuntimeError('driver exited; inspect webdriver.log')
            try:
                return call('GET', '/status').get('ready') is True
            except (OSError, RuntimeError):
                return False
        wait(driver_ready, 'Chrome WebDriver')
        chrome_capabilities={'browserName':'chrome','goog:chromeOptions': {
            'binary': str(TOOLS / 'chrome-linux64/chrome'),
            'args': ['--headless=new','--disable-dev-shm-usage','--user-data-dir=' + str(work/'profile'),
                     '--ignore-certificate-errors-spki-list=' + spki, '--window-size=1440,1100', '--no-first-run','--no-default-browser-check']}}
        if args.shell_lifecycle:
            chrome_capabilities['goog:chromeOptions']['args'].append('--remote-debugging-pipe')
        firefox_capabilities={'browserName':'firefox','acceptInsecureCerts':False,'moz:firefoxOptions':{
            'binary':'/usr/bin/firefox','args':['-headless','-profile',str(work/'profile')],
            # Firefox 157 dispatches USB before the software manager. A virtual
            # authenticator belongs to the software manager, so select that
            # fixture transport explicitly; the server still requires UV.
            'prefs':{'security.enterprise_roots.enabled':False,
                     'security.webauth.webauthn_enable_softtoken':True,
                     'security.webauth.webauthn_enable_usbtoken':False}}}
        info = call('POST','/session',{'capabilities':{'alwaysMatch':firefox_capabilities if args.browser=='firefox' else chrome_capabilities}})
        session = info['sessionId']
        receipt['browser_version_actual'] = info['capabilities']['browserVersion']
        check(receipt['browser_version_actual'] == receipt['browser_version_expected'], 'actual browser matches reviewed version')
        if args.browser=='firefox':
            wd('/moz/context',{'context':'chrome'})
            try:
                receipt['virtual_authenticator_transport_prefs']=js('return {software:Services.prefs.getBoolPref("security.webauth.webauthn_enable_softtoken",false),usb:Services.prefs.getBoolPref("security.webauth.webauthn_enable_usbtoken",true)}')
            finally:
                wd('/moz/context',{'context':'content'})
            check(receipt['virtual_authenticator_transport_prefs']=={'software':True,'usb':False},
                  'virtual authenticator dispatch selects software transport instead of physical USB')
        wd('/timeouts', {'script': 35000, 'pageLoad': 30000, 'implicit': 0})
        if args.browser=='chrome':cdp('WebAuthn.enable', {})
        authenticator = add_authenticator()
        wd('/window/rect',{'width':1440,'height':1100})
        receipt['virtual_authenticator'] = {
            'protocol':'ctap2_1' if args.browser=='firefox' else 'ctap2',
            'transport':'usb' if args.browser=='firefox' else 'internal',
            'user_verified':True,'resident_key_capability':True}
        wd('/url', {'url': origin + '/console/'})
        wait(lambda: js('return !!document.querySelector("[data-testid=connect-server]")'), 'WASM mounted')
        check(js('return !!document.querySelector("[data-testid=locked-screen]")'), 'initial view is locked')
        receipt['cold_shell_timing']=js('return {navigation:performance.getEntriesByType("navigation")[0]?.toJSON(),assets:performance.getEntriesByType("resource").map(e=>({path:new URL(e.name).pathname,duration_ms:e.duration,transfer_bytes:e.transferSize,encoded_bytes:e.encodedBodySize}))}')
        async_js('const done=arguments[arguments.length-1];navigator.serviceWorker.ready.then(()=>done(true)).catch(error=>done({error:String(error)}))')
        # Gecko WebDriver refresh bypasses worker control. Ordinary navigation
        # exercises the installed public-shell worker in both engines.
        js('location.assign(location.href)')
        wait(lambda: js('return !!document.querySelector("[data-testid=connect-server]") && !!navigator.serviceWorker.controller'), 'public worker controls reloaded shell')
        check(async_js('const done=arguments[arguments.length-1];navigator.serviceWorker.getRegistration().then(r=>done(new URL(r.scope).pathname==="/console/" && new URL(r.active.scriptURL).pathname==="/console/service-worker.js"))'),'actual worker registration and script remain inside console scope')
        receipt['warm_shell_timing']=js('return {navigation:performance.getEntriesByType("navigation")[0]?.toJSON(),assets:performance.getEntriesByType("resource").map(e=>({path:new URL(e.name).pathname,duration_ms:e.duration,transfer_bytes:e.transferSize,encoded_bytes:e.encodedBodySize}))}')
        js('window.fixtureApiHeaders=[];window.fixtureApiHeadersTruncated=false;const original=window.fetch;window.fetch=async(...args)=>{const response=await original(...args);const url=typeof args[0]==="string"?args[0]:args[0].url;const path=new URL(url,location.href).pathname;if(path.startsWith("/v1/")){if(window.fixtureApiHeaders.length>=512)window.fixtureApiHeadersTruncated=true;else window.fixtureApiHeaders.push({path,cache_control:response.headers.get("cache-control"),csp:response.headers.get("content-security-policy"),coop:response.headers.get("cross-origin-opener-policy"),corp:response.headers.get("cross-origin-resource-policy")});}return response}')
        click('Connect to server')
        wait(lambda: js('return document.body.textContent.includes("Sign in with a passkey") && !document.body.textContent.includes("Working…")'), 'live sign-in locked view')
        js('''const original=window.fabricPasskey;window.fabricPasskey=async function(register,text){
          const options=JSON.parse(text).publicKey;window.fixtureCeremonyDiagnostic={register,attestation:options.attestation,authenticatorSelection:options.authenticatorSelection};
          try{const result=await original(register,text);window.fixtureCeremonyDiagnostic.complete=true;return result;}
          catch(error){window.fixtureCeremonyDiagnostic.errorName=error.name;window.fixtureCeremonyDiagnostic.errorMessage=error.message;throw error;}}''')
        js('Array.from(document.querySelectorAll("details")).find(e=>e.querySelector("button")?.textContent.trim()==="Create owner passkey").open=true')
        expect(api('/v1/console/auth/register/start','POST',{'bootstrap_secret':'','display_name':'First visitor'}), 403, 'access denied', 'first visitor cannot claim owner without protected bootstrap')
        field('Display name', 'Browser fixture owner')
        field('One-time setup secret', bootstrap)
        bootstrap_replay=bootstrap
        bootstrap = None
        click('Create owner passkey')
        wait(lambda: js('return document.body.textContent.includes("Browser fixture owner · human")'), 'actual WebAuthn owner registration',seconds=65 if args.browser=='firefox' else 25)
        check(not bootstrap_file.exists(), 'first owner consumed protected one-time bootstrap')
        expect(api('/v1/console/auth/register/start','POST',{'bootstrap_secret':bootstrap_replay,'display_name':'Replayed owner'}), 403, 'access denied', 'consumed one-time bootstrap cannot start another owner')
        bootstrap_replay=None
        cookies = wd('/cookie', method='GET')
        cookie = next(item for item in cookies if item['name'].startswith('__Host-'))
        check(cookie.get('secure') and cookie.get('httpOnly') and cookie.get('sameSite') == 'Strict' and cookie.get('path') == '/', 'session cookie Secure HttpOnly SameSite Strict host path')
        code, view = api('/v1/console/session')
        check(code == 200 and view['api_version'] == 1, 'real server session compatibility')
        principal_id = view['principal_id']
        expect(api('/v1/console/logout','POST'), 403, 'access denied', 'cookie mutation without CSRF rejected')
        check(api('/v1/console/session')[0] == 200, 'rejected CSRF mutation preserves session')
        request=urllib.request.Request(origin+'/v1/console/logout',data=b'',method='POST',headers={
            'Cookie':cookie['name']+'='+cookie['value'],'x-fabric-client-version':'1',
            'x-fabric-csrf':view['csrf'],'Origin':'https://untrusted.invalid'})
        try:
            urllib.request.urlopen(request,context=context,timeout=10)
            raise AssertionError('cross-origin cookie mutation unexpectedly accepted')
        except urllib.error.HTTPError as error:
            expect((error.code,json.load(error)),403,'access denied','cookie mutation with foreign Origin rejected')
        check(api('/v1/console/session')[0] == 200, 'rejected Origin mutation preserves session')
        if args.pressure:
            held=[]
            try:
                for _ in range(2):
                    stream=context.wrap_socket(socket.create_connection(('127.0.0.1',server_port),timeout=5),server_hostname='localhost')
                    stream.settimeout(20)
                    head=(f'POST /v1/console/query HTTP/1.1\r\nHost: localhost:{server_port}\r\n'
                          f'Cookie: {cookie["name"]}={cookie["value"]}\r\nx-fabric-client-version: 1\r\n'
                          f'Origin: {origin}\r\nx-fabric-csrf: {view["csrf"]}\r\n'
                          'Content-Type: application/json\r\nContent-Length: 65536\r\nConnection: close\r\n\r\n{')
                    stream.sendall(head.encode());held.append(stream)
                wait(lambda:api('/v1/console/query','POST',{})[0]==503,'finite protected query pool saturation',seconds=4)
                expect(bearer_api('0'*64,'/v1/console/session'), 401, 'sign in required', 'invalid credential rejected before occupied protected query pool')
                started=time.monotonic()
                response=held[0].recv(8192)
                elapsed=time.monotonic()-started
                receipt['protected_body_timeout']={'elapsed_s':elapsed,'response_status_line':response.split(b'\r\n',1)[0].decode(errors='replace')}
                check(b'408' in response.split(b'\r\n',1)[0],'actual partial authenticated body reaches finite server deadline')
                check(elapsed<=18,'native authenticated body wait stays within bounded deadline allowance')
            finally:
                for stream in held:stream.close()
            wait(lambda:api('/v1/console/session')[0]==200,'protected admission recovers after timeout/cancellation',seconds=4)
        code, _ = api('/v1/console/session', version=False)
        expect((code,_),426,'console update required','missing client version rejected')
        unsupported=async_js("const done=arguments[arguments.length-1];fetch('/v1/console/query',{method:'POST',headers:{'x-fabric-client-version':'999','content-type':'application/json'},body:'{}'}).then(async r=>done({status:r.status,text:await r.text()}));")
        expect((unsupported['status'],json.loads(unsupported['text'])),426,'console update required','unsupported client cannot submit incompatible query')
        nav('Settings')
        click('Refresh inventory')
        wait(lambda: js('return document.body.textContent.includes("fabric-server-self")'), 'actual companion scoped inventory')
        check(js('return document.body.textContent.includes("enrollment_id")'), 'inventory exposes immutable enrollment IDs')
        click('Refresh status')
        wait(lambda: js('return document.body.textContent.includes("retention_bytes")'), 'effective measured storage policy')
        click('List passkeys')
        wait(lambda: js('return document.querySelectorAll(".live-json").length > 0 && !document.body.textContent.includes("Working…")'), 'passkey listing')
        # excludeCredentials intentionally rejects enrolling the same authenticator
        # twice. Model a second physical authenticator for the additional passkey.
        remove_authenticator(authenticator)
        authenticator = add_authenticator()
        click('Add another passkey')
        wait(lambda: api('/v1/console/passkeys')[0] == 200 and len(api('/v1/console/passkeys')[1]) == 2, 'additional WebAuthn passkey')
        code, passkeys = api('/v1/console/passkeys')
        check(code == 200 and len(passkeys) == 2, 'two real virtual-authenticator passkeys enrolled')
        # Enroll and run an actual second Spindle for an untrusted-text fixture.
        fixture_log=work/'untrusted.log'
        fixture_log.write_text('<img src=x onerror="window.fabricXssMarker=1"><script>window.fabricXssMarker=1</script> browser-xss-fixture\n'+
            ''.join(f'browser-page-fixture ordinal={i:03d} trace_id={TRACE_ID}\n' for i in range(250)))
        field('Absolute log paths, one per line',str(fixture_log))
        field('Metric interval (1–3600 s)','1')
        js('Array.from(document.querySelectorAll("details")).find(e=>e.textContent.includes("Enroll a new Spindle")).open=true')
        field('New Spindle name','browser-edge')
        click('Enroll with configuration above')
        wait(lambda:js('return document.body.textContent.includes("One-time credential or invitation")'),'scoped Spindle enrollment')
        node_token=js('return Array.from(document.querySelectorAll("code")).map(e=>e.textContent.trim()).find(value=>/^[a-f0-9]{64}$/.test(value))')
        token_file=work/'edge-token';token_file.write_text(node_token+'\n');token_file.chmod(0o600);node_token=None
        click('Clear displayed credential')
        node_config=work/'edge.conf'
        trace_port=free_port()
        node_config.write_text(f'spool_dir={work}/edge-spool\nlog={fixture_log}\nmetric_interval_s=1\nspool_bytes=33554432\nserver_url={origin}\nserver_ca={work}/ca.pem\ntoken_file={token_file}\nmax_output_bytes_per_s=65536\ntraces_listen=127.0.0.1:{trace_port}\n')
        edge=launch('edge',[str(args.spindle.resolve()),'run',str(node_config)])
        wait(lambda:(out/'edge.log').is_file() and 'status=ack' in (out/'edge.log').read_text(),'native edge exact delivery ACK')
        trace_origin=time.time_ns()-1_000_000_000
        trace_body,trace_rows=traces(trace_origin)
        request=urllib.request.Request(f'http://127.0.0.1:{trace_port}/v1/traces',data=trace_body,headers={'Content-Type':'application/x-protobuf'})
        with urllib.request.urlopen(request,timeout=15) as response:
            check(response.status==200,'independent OTLP trace producer received committed-Spool success')
        receipt['producer_expected_spans']=trace_rows
        receipt['producer_schema']='https://github.com/open-telemetry/opentelemetry-proto/blob/v1.9.0/opentelemetry/proto/trace/v1/trace.proto'
        canary_b='browser-canary-B-private'
        b_log=work/'canary-b-private.log';b_log.write_text(canary_b+'\n')
        field('Absolute log paths, one per line',str(b_log))
        field('Metric interval (1–3600 s)','7')
        field('New Spindle name','browser-canary-b')
        click('Enroll with configuration above')
        wait(lambda:js('return document.body.textContent.includes("One-time credential or invitation")'),'second canary immutable enrollment')
        b_token=js('return Array.from(document.querySelectorAll("code")).map(e=>e.textContent.trim()).find(value=>/^[a-f0-9]{64}$/.test(value))')
        b_token_file=work/'canary-b-token';b_token_file.write_text(b_token+'\n');b_token_file.chmod(0o600);b_token=None
        click('Clear displayed credential')
        b_config=work/'canary-b.conf'
        b_config.write_text(f'spool_dir={work}/canary-b-spool\nlog={b_log}\nmetric_interval_s=7\nspool_bytes=4194304\nserver_url={origin}\nserver_ca={work}/ca.pem\ntoken_file={b_token_file}\nmax_output_bytes_per_s=65536\n')
        launch('canary-b',[str(args.spindle.resolve()),'run',str(b_config)])
        wait(lambda:(out/'canary-b.log').is_file() and 'status=ack' in (out/'canary-b.log').read_text(),'independent second canary source delivers through actual ACK')
        canary_query={'kind':'logs','node':'browser-canary-b','from_ns':trace_origin-60_000_000_000,'to_ns':time.time_ns()+60_000_000_000,'contains':canary_b,'limit':200,'page':None}
        wait(lambda:api('/v1/console/query','POST',canary_query)[0]==200 and len(api('/v1/console/query','POST',canary_query)[1]['rows'])==1,'owner positive witness for second canary history')
        b_code,b_answer=api('/v1/console/query','POST',canary_query)
        check(b_code==200 and b_answer['rows'][0]['body']==canary_b,'second source matches distinct producer canary constant')
        receipt['second_canary_observed_ns']=b_answer['rows'][0]['observed_ns']
        receipt['second_canary_freshness_ns']=b_answer['freshness']['browser-canary-b']
        field('Existing Spindle name','browser-edge')
        click('pause')
        wait(lambda:not js('return document.body.textContent.includes("Working…")'),'scoped pause')
        code,current=api('/v1/console/nodes')
        check(next(n for n in current['nodes'] if n['name']=='browser-edge')['status']=='paused','actual scoped Spindle pause applied to control')
        click('resume')
        wait(lambda:not js('return document.body.textContent.includes("Working…")'),'scoped resume')
        code,current=api('/v1/console/nodes')
        check(next(n for n in current['nodes'] if n['name']=='browser-edge')['status']=='active','actual scoped Spindle resume applied to control')
        nav('Explore')
        signal_view=js('const select=Array.from(document.querySelectorAll("label")).find(e=>e.textContent.trim().startsWith("Signal")).querySelector("select");return {selected:select.value,label:Array.from(document.querySelectorAll("label")).map(e=>e.textContent.trim()).find(text=>text.startsWith("Body substring")||text.startsWith("Exact metric name")||text.startsWith("Exact trace ID"))}')
        receipt['initial_signal_view']=signal_view
        check(signal_view['label'].startswith({'logs':'Body substring','metrics':'Exact metric name','spans':'Exact trace ID'}.get(signal_view['selected'],'invalid selection')),'initial granted signal selection agrees with actual query filter label')
        field('Signal','logs')
        field('Source name (blank = authorized scope)','browser-edge')
        field('Body substring (case-sensitive)','browser-xss-fixture')
        click('Run query')
        wait(lambda:js('return document.querySelector(".results")?.textContent.includes("browser-xss-fixture")'),'actual untrusted source query')
        check(js('return document.querySelectorAll(".results img,.results script").length===0 && !window.fabricXssMarker'),'stored untrusted telemetry rendered as inert text')
        field('Body substring (case-sensitive)','browser-page-fixture')
        click('Last 15 minutes')
        click('Run query')
        wait(lambda:js('return document.querySelectorAll(".results tbody tr").length===200'),'bounded first actual log page')
        first_page=js('return Array.from(document.querySelectorAll(".results tbody tr pre")).map(e=>e.textContent)')
        check(all(f'ordinal={i:03d}' in row for i,row in enumerate(first_page)), 'first page matches independent producer ordinals')
        click('Next snapshot page')
        wait(lambda:js('return document.querySelectorAll(".results tbody tr").length===50'),'bounded continuation actual log page')
        second_page=js('return Array.from(document.querySelectorAll(".results tbody tr pre")).map(e=>e.textContent)')
        check(all(f'ordinal={i+200:03d}' in row for i,row in enumerate(second_page)), 'continuation has exact remaining producer ordinals')
        field('Signal','metrics')
        field('Exact metric name','system.cpu.time')
        click('Run query')
        wait(lambda:js('return document.querySelectorAll(".live-metric-plot").length>0'),'actual native metric series chart')
        check(js('return document.querySelectorAll(".live-metric-plot").length<=8'),'metric charts remain bounded to eight distinct series')
        nav('Trace detail')
        field('Signal','spans')
        field('Exact trace ID (optional)',TRACE_ID)
        click('Run query')
        wait(lambda:js('return document.querySelectorAll(".live-span").length===3'),'actual recorded trace waterfall')
        waterfall=js('return Array.from(document.querySelectorAll(".live-span")).map(e=>e.textContent)')
        check(all(any(r['name'] in text and f"[{r['start_ns']}, {r['end_ns']}]" in text for text in waterfall) for r in trace_rows), 'waterfall preserves independently produced adjacent integer timestamps')
        check(any('browser-orphan' in text and 'parent unavailable in this page' in text for text in waterfall),'missing parent remains explicit in actual trace page')
        geometry=js('return Array.from(document.querySelectorAll(".live-span")).map(e=>{const track=e.querySelector(".live-span-track").getBoundingClientRect(),bar=e.querySelector(".live-span-bar").getBoundingClientRect();return {name:e.querySelector("b").textContent,left_percent:100*(bar.left-track.left)/track.width,width_percent:100*bar.width/track.width}})')
        receipt['trace_geometry']=geometry
        first_start=min(row['start_ns'] for row in trace_rows);span_range=max(row['end_ns'] for row in trace_rows)-first_start
        check(len(geometry)==3 and all(any(item['name']==row['name'] and abs(item['left_percent']-100*(row['start_ns']-first_start)/span_range)<.03 and abs(item['width_percent']-100*(row['end_ns']-row['start_ns'])/span_range)<.03 for item in geometry) for row in trace_rows),'actual waterfall geometry matches independent integer offsets and durations')
        field('Exact span name (optional)','browser-child');click('Run query')
        wait(lambda:js('return document.querySelectorAll(".live-span").length===1 && document.querySelectorAll(".results tbody tr").length===1'),'backend-supported exact span-name UI returns only independent child span')
        named_spans=[json.loads(value) for value in js('return Array.from(document.querySelectorAll(".results tbody .live-json")).map(e=>e.textContent)')]
        check(len(named_spans)==1 and named_spans[0]['name']=='browser-child' and named_spans[0]['start_ns']==trace_rows[1]['start_ns'] and named_spans[0]['end_ns']==trace_rows[1]['end_ns'],'exact span-name filter preserves independent integer producer constants')
        related_button=wd('/element',{'using':'css selector','value':'[data-testid=live-related-logs]'})['element-6066-11e4-a52e-4f735466cecf']
        wd('/element/'+related_button+'/click',{})
        wait(lambda:js('return document.querySelectorAll(".results tbody tr").length===200 && document.querySelector(".breadcrumb")?.textContent.includes("Explore")'),'actual related-log action opens producer-correlated source rows')
        related_rows=[json.loads(value) for value in js('return Array.from(document.querySelectorAll(".results tbody .live-json")).map(e=>e.textContent)')]
        check(len(related_rows)==200 and all(TRACE_ID in row['body'] and row['node']=='browser-edge' for row in related_rows),'related-log navigation matches independent trace/log correlation constant')
        nav('Overview')
        click('Refresh sources')
        wait(lambda:js('return Array.from(document.querySelectorAll(".source-row b")).some(e=>e.textContent==="fabric-server-self")'),'overview shows readable actual companion inventory')
        click('Load process observations')
        wait(lambda:js('return Array.from(document.querySelectorAll(".stat h3")).some(e=>e.textContent.includes("PID"))'),'overview shows measured process samples from real companion')
        check(js('return document.body.textContent.includes("RSS") && document.body.textContent.includes("ticks/s") && document.body.textContent.includes("collector observed")'),'overview labels measured memory CPU and observation clocks')
        nav('Pipeline')
        click('Refresh status')
        wait(lambda:js('return document.body.textContent.includes("retention_bytes")'),'pipeline effective measured policy')
        check(js('return document.querySelectorAll(".pipeline-stages li").length===5 && document.body.textContent.includes("Server ACK follows durable commit")'),'pipeline states actual custody boundary through readable stages')
        nav('Live Tail')
        field('Signal','logs')
        field('Source name (blank = authorized scope)','browser-edge')
        field('Body substring (case-sensitive)','browser-xss-fixture')
        click('Resume 5-second polling')
        wait(lambda:js('return document.querySelectorAll(".tail-line").length>0'),'bounded actual tail snapshot',seconds=12)
        click('Pause 5-second polling')
        paused_tail=js('return document.querySelector(".tail-terminal").textContent')
        check(js('return document.body.textContent.includes("Resume 5-second polling")'),'actual tail pause control')
        click('Resume 5-second polling')
        check(js('return document.body.textContent.includes("Pause 5-second polling")'),'actual tail resume control')
        if args.browser=='chrome':
            js('window.fixtureTailFetch=window.fetch;window.fixtureTailQueryCount=0;window.fetch=(...args)=>{const url=typeof args[0]==="string"?args[0]:args[0].url;if(new URL(url,location.href).pathname==="/v1/console/query")window.fixtureTailQueryCount++;return window.fixtureTailFetch(...args)}')
            wait(lambda:js('return window.fixtureTailQueryCount>0 && !document.body.textContent.includes("Working…")'),'visible tail has positive polling witness',seconds=7)
            tail_handle=wd('/window',method='GET')
            previous_handles=set(wd('/window/handles',method='GET'))
            check(js('window.fixtureHiddenTab=window.open("about:blank","fixture-hidden-tail");return !!window.fixtureHiddenTab'),'owned background-polling tab created')
            hidden_handle=next(handle for handle in wd('/window/handles',method='GET') if handle not in previous_handles)
            wd('/window',{'handle':hidden_handle})
            wait(lambda:js('return window.opener.document.visibilityState==="hidden"'),'actual tail document becomes hidden')
            before_hidden=js('return window.opener.fixtureTailQueryCount')
            hidden_start=time.monotonic()
            while time.monotonic()-hidden_start<12:
                check(js('return window.opener.fixtureTailQueryCount')==before_hidden,'hidden tail sends no new protected query')
                time.sleep(1)
            receipt['hidden_tail_polling']={'duration_s':time.monotonic()-hidden_start,'queries_before':before_hidden,'queries_after':js('return window.opener.fixtureTailQueryCount')}
            wd('/window',method='DELETE');wd('/window',{'handle':tail_handle})
            wait(lambda:js('return window.fixtureTailQueryCount>arguments[0]',before_hidden),'tail polling resumes only after visible document returns',seconds=7)
            js('window.fetch=window.fixtureTailFetch;delete window.fixtureTailFetch')
        if args.poll_seconds:
            field('Body substring (case-sensitive)','browser-poll-fixture')
            polling=[];start_poll=time.monotonic();next_write=start_poll;ordinal=0
            while time.monotonic()-start_poll<args.poll_seconds:
                if time.monotonic()>deadline or storage_violation.is_set():raise RuntimeError('bounded polling fixture deadline/storage limit')
                if time.monotonic()>=next_write:
                    with fixture_log.open('a') as source:source.write(f'browser-poll-fixture ordinal={ordinal:03d}\n')
                    ordinal+=1;next_write+=5
                observation=js('return {rows:document.querySelectorAll(".tail-line").length,bytes:new TextEncoder().encode(document.querySelector(".tail-terminal").textContent).length,status:Array.from(document.querySelectorAll("[role=status]")).map(e=>e.textContent)}')
                observation['elapsed_s']=time.monotonic()-start_poll
                if observation['rows']>200 or observation['bytes']>262144:raise AssertionError('sustained tail snapshot exceeded display budget')
                polling.append(observation)
                time.sleep(1)
            receipt['bounded_polling']=polling
            check(any(item['rows']>0 for item in polling),'two-minute real tail workload observed independently appended producer rows')
            check(all(item['rows']<=200 and item['bytes']<=262144 for item in polling),'two-minute tail retains explicit row/UTF-8 display bounds')
            click('Pause 5-second polling')
        nav('Explore')
        field('Source name (blank = authorized scope)', 'fabric-server-self')
        field('Body substring (case-sensitive)', '')
        click('Run query')
        wait(lambda: js('return document.body.textContent.includes("Evidence and exact applied request") && !document.body.textContent.includes("Working…")'), 'actual retained-history response')
        check(js('return document.body.textContent.includes("retained_from_ns") && document.body.textContent.includes("snapshot")'), 'query evidence remains visible')
        wait(lambda:js('return document.querySelectorAll(".results td.mono").length>0'), 'real companion observations retained')
        exact_titles=js('return Array.from(document.querySelectorAll(".results td.mono")).map(e=>e.title)')
        exact_rows=[json.loads(value) for value in js('return Array.from(document.querySelectorAll(".results tbody .live-json")).map(e=>e.textContent)')]
        check(bool(exact_titles) and len(exact_titles)==len(exact_rows) and exact_titles==[str(row['observed_ns']) for row in exact_rows],'human-readable timestamp titles match exact typed response integers')
        check(js('return !!document.querySelector("details.exact-range") && !document.querySelector("details.exact-range").open && document.querySelectorAll(".results details").length>0 && Array.from(document.querySelectorAll(".results details")).every(e=>!e.open)'),'exact query bounds evidence and row JSON start collapsed')
        (out/'desktop.png').write_bytes(base64.b64decode(wd('/screenshot', method='GET')))
        contrast_script='''const foreground=getComputedStyle(document.querySelector(".results h2")).color;
          const background=getComputedStyle(document.querySelector(".results")).backgroundColor;
          const luminance=value=>{const c=value.match(/[0-9.]+/g).slice(0,3).map(Number).map(x=>{x/=255;return x<=.04045?x/12.92:((x+.055)/1.055)**2.4});return .2126*c[0]+.7152*c[1]+.0722*c[2]};
          const a=luminance(foreground),b=luminance(background);return {foreground,background,ratio:(Math.max(a,b)+.05)/(Math.min(a,b)+.05)};'''
        theme_button=js('return document.querySelector(".theme-button").textContent.trim()')
        first_contrast=js(contrast_script)
        click(theme_button)
        wait(lambda:js('return document.querySelector(".theme-button").textContent.trim()')!=theme_button,'color theme toggled')
        async_js('const done=arguments[arguments.length-1];Promise.all(document.getAnimations().filter(a=>a.effect.getTiming().iterations!==Infinity).map(a=>a.finished.catch(()=>{}))).then(()=>done(true))')
        second_contrast=js(contrast_script)
        receipt['selected_text_contrast']=[first_contrast,second_contrast]
        check(all(item['ratio']>=4.5 for item in receipt['selected_text_contrast']),'selected measured heading text contrast at least 4.5 in both themes')
        click(js('return document.querySelector(".theme-button").textContent.trim()'))
        body_element=wd('/element',{'using':'css selector','value':'body'})['element-6066-11e4-a52e-4f735466cecf']
        js('document.body.setAttribute("tabindex","-1");document.body.focus()')
        wd('/element/'+body_element+'/value',{'text':'\ue004','value':['\ue004']})
        js('document.body.removeAttribute("tabindex")')
        check(js('return document.activeElement.classList.contains("skip-link")'),'keyboard Tab reaches visible skip link')
        check(js('return document.activeElement.getBoundingClientRect().top>=0'),'focused skip link visible')
        field('Body substring (case-sensitive)','x'*4097)
        check(js('return !!document.querySelector("[role=status][aria-live=polite]") && document.querySelector("[role=alert]")?.textContent.includes("Filter exceeds 4096 UTF-8 bytes")'),'actual invalid filter error and request status have announcement roles')
        field('Body substring (case-sensitive)','');click('Run query')
        wait(lambda:js('return document.querySelectorAll(".results tbody tr").length>0'),'valid query restores readable table after announced validation error')
        wd('/window/rect', {'width':390,'height':844})
        check(js('return document.documentElement.scrollWidth <= window.innerWidth + 2'), 'narrow viewport has no document overflow')
        (out/'narrow.png').write_bytes(base64.b64decode(wd('/screenshot', method='GET')))
        js('document.documentElement.style.zoom="200%"')
        receipt['zoom_geometry']=js('return {viewport:innerWidth,documentWidth:document.documentElement.scrollWidth,overflow:Array.from(document.querySelectorAll("body *")).map(e=>({tag:e.tagName,cls:e.className?.baseVal??e.className,text:e.tagName==="BUTTON"?e.textContent:null,left:e.getBoundingClientRect().left,right:e.getBoundingClientRect().right})).filter(e=>e.right>innerWidth+2).slice(0,40)}')
        check(js('return document.documentElement.scrollWidth <= window.innerWidth + 2'),'200 percent content zoom retains document reflow')
        # Hosted CI exposed an unconstrained standalone pagination action.
        # Force a larger user font on that actual action, outside query-form.
        js("window.fixtureNextAction=Array.from(document.querySelectorAll('button')).find(e=>e.textContent.trim()==='Next snapshot page');if(!window.fixtureNextAction)throw Error('standalone pagination action missing');window.fixtureNextStyle=window.fixtureNextAction.style.cssText;window.fixtureNextAction.style.fontSize='18px';")
        receipt['pagination_reflow_candidate']=js("const e=window.fixtureNextAction;return {font:getComputedStyle(e).fontSize,viewport:innerWidth,documentWidth:document.documentElement.scrollWidth,buttonLeft:e.getBoundingClientRect().left,buttonRight:e.getBoundingClientRect().right,maxInlineSize:getComputedStyle(e).maxInlineSize,whiteSpace:getComputedStyle(e).whiteSpace}")
        check(js("return getComputedStyle(window.fixtureNextAction).fontSize==='18px' && !window.fixtureNextAction.closest('.query-form')"),'actual standalone pagination action uses independent 18px font witness')
        check(js('return document.documentElement.scrollWidth <= innerWidth + 2 && window.fixtureNextAction.getBoundingClientRect().right<=innerWidth+2'),'standalone pagination fits at 200 percent zoom and 18px font')
        try:
            js("window.fixtureNextAction.style.maxInlineSize='none';window.fixtureNextAction.style.whiteSpace='nowrap';window.fixtureNextAction.style.overflowWrap='normal';")
            receipt['pagination_reflow_negative']=js("const e=window.fixtureNextAction;return {viewport:innerWidth,documentWidth:document.documentElement.scrollWidth,buttonRight:e.getBoundingClientRect().right,maxInlineSize:getComputedStyle(e).maxInlineSize,whiteSpace:getComputedStyle(e).whiteSpace}")
            check(js('return document.documentElement.scrollWidth > innerWidth + 2 && window.fixtureNextAction.getBoundingClientRect().right>innerWidth+2'),'pagination reflow grader rejects removed width cap and forced nowrap')
        finally:
            js("window.fixtureNextAction.style.cssText=window.fixtureNextStyle;delete window.fixtureNextAction;delete window.fixtureNextStyle;")
        check(js('return document.documentElement.scrollWidth <= innerWidth + 2'),'pagination negative control restores coherent responsive layout')
        js('document.documentElement.style.zoom=""')
        wd('/window/rect', {'width':1440,'height':1100})
        code, _ = api('/v1/console/query', 'POST', {'kind':'rate','name':'test','from_ns':1,'to_ns':2})
        expect((code,_),400,'unbounded rate queries are unavailable in the console','rate endpoint is unavailable in scoped console')
        cache_keys = async_js('const done=arguments[arguments.length-1];caches.keys().then(async keys=>done((await Promise.all(keys.map(async key=>(await(await caches.open(key)).keys()).map(r=>new URL(r.url).pathname)))).flat()))')
        check(all(path.startswith('/console/') for path in cache_keys), 'worker caches only public shell paths')
        check(js('return localStorage.length===0 && sessionStorage.length===0'), 'no credentials or telemetry in Web Storage')
        js('window.fixtureSupersededFetch=window.fetch;window.fixtureSupersededReady=false;window.fixtureSupersededAbort=false;window.fixtureSupersededDelivered=false;window.fetch=async(...args)=>{const url=typeof args[0]==="string"?args[0]:args[0].url;const query=new URL(url,location.href).pathname==="/v1/console/query";if(query){const signal=args[0]?.signal||args[1]?.signal;if(signal)signal.addEventListener("abort",()=>window.fixtureSupersededAbort=true,{once:true});}const response=await window.fixtureSupersededFetch(...args);if(query){window.fixtureSupersededReady=true;await new Promise(resolve=>setTimeout(resolve,1200));window.fixtureSupersededDelivered=true;}return response}')
        click('Run query')
        wait(lambda:js('return window.fixtureSupersededReady'),'actual response is held before superseding input')
        field('Body substring (case-sensitive)','superseded-cancel-fixture')
        wait(lambda:js('return window.fixtureSupersededAbort'),'superseded query aborts actual transport signal')
        check(js('return Array.from(document.querySelectorAll("button")).find(e=>e.textContent.trim()==="Run query").disabled'),'superseded request retains pending slot until transport completion')
        wait(lambda:js('return window.fixtureSupersededDelivered && !document.body.textContent.includes("Working…")'),'superseded transport reaches terminal completion')
        check(js('return !document.querySelector(".results tbody") && !document.querySelector(".results > details") && document.querySelector(".results")?.textContent.includes("No observations have been fetched")'),'superseded reply cannot restore discarded query results')
        js('window.fetch=window.fixtureSupersededFetch;delete window.fixtureSupersededFetch')
        field('Body substring (case-sensitive)','');click('Run query')
        wait(lambda:js('return document.querySelectorAll(".results tbody tr").length>0'),'new explicit query succeeds after cancelled transport completes')
        js('''window.fixtureOriginalFetch=window.fetch;window.fixtureDelayedReady=false;window.fixtureDelayedDelivered=false;
          window.fetch=async(...args)=>{const response=await window.fixtureOriginalFetch(...args);
          const url=typeof args[0]==="string"?args[0]:args[0].url;
          if(new URL(url,location.href).pathname==="/v1/console/query"){
            window.fixtureDelayedReady=true;await new Promise(resolve=>setTimeout(resolve,1200));window.fixtureDelayedDelivered=true;}return response;}''')
        click('Run query')
        wait(lambda:js('return window.fixtureDelayedReady'),'real authenticated response is held before disconnect')
        js('window.dispatchEvent(new Event("offline"))')
        check(js('return !document.querySelector(".results") && document.body.textContent.includes("Sign in with a passkey") && !document.body.textContent.includes("Browser fixture owner · human")'), 'offline clears protected in-memory views')
        wait(lambda:js('return window.fixtureDelayedDelivered && !document.body.textContent.includes("Working…")'),'delayed authenticated response completed after lock')
        check(js('return !document.querySelector(".results") && document.body.textContent.includes("Sign in to view your telemetry.")'),'delayed response cannot restore protected view after disconnect')
        js('window.fetch=window.fixtureOriginalFetch;delete window.fixtureOriginalFetch')
        click('Check session')
        wait(lambda: js('return document.body.textContent.includes("Browser fixture owner · human")'), 'online session explicitly revalidated')
        nav('Settings')
        code,inventory=api('/v1/console/nodes')
        enrollment=next(node['enrollment_id'] for node in inventory['nodes'] if node['name']=='browser-edge')
        grant={'actions':['telemetry_read','inventory_read'],'installation_wide':False,'enrollments':[enrollment],
               'signals':['logs'],'max_query_window_s':3600,'max_query_rows':200,'allowed_log_paths':[],
               'min_interval_s':1,'max_interval_s':3600,'enrollment_namespace':None,'max_enrollments':0}
        field('Operation','workload')
        field('Display name or immutable target ID','Browser fixture reader')
        field('Credential lifetime (seconds)','1800' if args.expired_cursor_probe else '600')
        field('Explicit scope (review every field before issuance)',json.dumps(grant))
        click('Submit reviewed scoped change')
        wait(lambda:js('return document.body.textContent.includes("credential_id") && document.querySelector("code")!==null'), 'real scoped workload credential issuance')
        credential=js('return Array.from(document.querySelectorAll("code")).map(e=>e.textContent.trim()).find(value=>/^[a-f0-9]{64}$/.test(value))')
        issuance=next(v for v in (json.loads(text) for text in js('return Array.from(document.querySelectorAll(".live-json")).map(e=>e.textContent)') if text.strip().startswith('{')) if isinstance(v,dict) and 'credential_id' in v)
        check(bool(credential),'issued workload secret displayed once')
        click('Clear displayed credential')
        check(js('return !Array.from(document.querySelectorAll("code")).some(e=>/^[a-f0-9]{64}$/.test(e.textContent.trim()))'),'one-time credential clear control removes displayed secret')
        # Exercise the issued workload over trusted native TLS without browser cookies.
        headers={'Authorization':'Bearer '+credential,'x-fabric-client-version':'1'}
        request=urllib.request.Request(origin+'/v1/console/nodes',headers=headers)
        with urllib.request.urlopen(request,context=context,timeout=10) as response:scoped_nodes=json.load(response)
        check(len(scoped_nodes['nodes'])==1 and all(node['enrollment_id']==enrollment for node in scoped_nodes['nodes']),'issued credential sees only its scoped immutable enrollment')
        request=urllib.request.Request(origin+'/v1/console/principals',headers=headers)
        try:
            urllib.request.urlopen(request,context=context,timeout=10)
            raise AssertionError('workload unexpectedly gained identity authority')
        except urllib.error.HTTPError as error:
            expect((error.code,json.load(error)),403,'access denied','workload identity administration denied')
        read={'kind':'logs','node':'browser-edge','from_ns':trace_origin-60_000_000_000,'to_ns':time.time_ns()+1_000_000_000,'contains':'browser-page-fixture','limit':200,'page':None}
        code,scoped_page=bearer_api(credential,'/v1/console/query','POST',read)
        check(code==200 and len(scoped_page['rows'])==200 and all(row['node']=='browser-edge' for row in scoped_page['rows']),'log-only bearer query returns only independently produced authorized-source rows')
        check(all(f'ordinal={i:03d}' in row['body'] for i,row in enumerate(scoped_page['rows'])),'scoped query matches producer ordinal constants')
        expect(bearer_api(credential,'/v1/console/query','POST',dict(read,contains='x'*4097)),400,'query filter exceeds budget','oversize query filter receives explicit bounded-input rejection')
        positive_code,positive_after_pressure=bearer_api(credential,'/v1/console/query','POST',read)
        check(positive_code==200 and positive_after_pressure['rows']==scoped_page['rows'],'query payload pressure preserves previously committed producer observations')
        code,scoped_status=bearer_api(credential,'/v1/console/status')
        check('browser-canary-b' not in json.dumps(scoped_status) and str(b_log) not in json.dumps(scoped_status),'scoped status excludes second source inventory paths and freshness')
        check(code==200 and 'committed_group' not in scoped_status,'scoped status hides global committed group')
        check(scoped_page['freshness'].get('browser-edge')!=receipt['second_canary_freshness_ns'],'independently collected canary fixtures have distinct freshness observations')
        all_logs_query=dict(read,contains='')
        all_logs_code,all_logs=bearer_api(credential,'/v1/console/query','POST',all_logs_query)
        all_log_rows=list(all_logs['rows'])
        if all_logs['next_page']:
            next_code,next_logs=bearer_api(credential,'/v1/console/query','POST',dict(all_logs_query,page=all_logs['next_page']))
            check(next_code==200 and next_logs['next_page'] is None,'bounded complete canary log witness spans at most two pages')
            all_log_rows.extend(next_logs['rows'])
        check(all_logs_code==200 and all_logs['freshness'].get('browser-edge')==max(row['observed_ns'] for row in all_log_rows),'log-only freshness reflects authorized log observations without later metric timestamp')
        check('browser-canary-b' not in json.dumps(scoped_page) and canary_b not in json.dumps(scoped_page),'scoped rows and evidence exclude independently witnessed second canary source')
        expect(bearer_api(credential,'/v1/console/query','POST',dict(canary_query)),403,'access denied','log grant cannot query second immutable canary source')
        denied_source=dict(read,node='fabric-server-self')
        expect(bearer_api(credential,'/v1/console/query','POST',denied_source), 403, 'access denied', 'log grant cannot query another immutable source')
        denied_signal={'kind':'metrics','node':'browser-edge','from_ns':read['from_ns'],'to_ns':read['to_ns'],'name':'system.cpu.time','limit':200,'page':None}
        expect(bearer_api(credential,'/v1/console/query','POST',denied_signal), 403, 'access denied', 'log-only grant cannot query metric observations')
        page=scoped_page['next_page']
        check(bool(page),'scoped continuation handle issued')
        expect(bearer_api(credential,'/v1/console/query','POST',dict(read,page=page+'x')), 410, 'page expired; run the query again', 'modified opaque page handle rejected')
        expect(bearer_api(credential,'/v1/console/query','POST',dict(read,page=page,contains='changed')), 403, 'access denied', 'page handle bound to original query')
        expect(api('/v1/console/query','POST',dict(read,page=page)), 403, 'access denied', 'scoped page handle cannot be reused by different principal')
        if args.expired_cursor_probe:
            cursor_wait_start=time.monotonic()
            while time.monotonic()-cursor_wait_start<902:
                if time.monotonic()>deadline or storage_violation.is_set():raise RuntimeError('expired cursor fixture bound exceeded')
                time.sleep(1)
            expect(bearer_api(credential,'/v1/console/query','POST',dict(read,page=page)),410,'page expired; run the query again','actual expired opaque cursor rejected before history execution')
            check(bearer_api(credential,'/v1/console/nodes')[0]==200,'cursor expiry rejection has adjacent live credential witness')
            receipt['cursor_expiry_wait_s']=time.monotonic()-cursor_wait_start
            click('Reverify with passkey')
            wait(lambda:not js('return document.body.textContent.includes("Working…")'),'fresh passkey step-up after expiry wait')
        field('Operation','rotate')
        field('Credential lifetime (seconds)','1800' if args.expired_cursor_probe else '600')
        field('Display name or immutable target ID',issuance['principal_id'])
        click('Submit reviewed scoped change')
        wait(lambda:js('return Array.from(document.querySelectorAll("code")).some(e=>/^[a-f0-9]{64}$/.test(e.textContent.trim()))'),'actual workload rotation one-time credential')
        rotated=js('return Array.from(document.querySelectorAll("code")).map(e=>e.textContent.trim()).find(value=>/^[a-f0-9]{64}$/.test(value))')
        click('Clear displayed credential')
        old_status,old_answer=bearer_api(credential,'/v1/console/nodes')
        receipt['rotation_previous_credential_status']=old_status
        check(old_status==200,'previous workload bearer remains during documented bounded rotation overlap')
        check(bearer_api(rotated,'/v1/console/nodes')[0]==200,'rotated workload bearer authorizes existing scope')
        field('Exact passkey, session, principal or credential ID',issuance['credential_id'])
        click('revoke credentials record')
        wd('/alert/accept',{})
        wait(lambda:bearer_api(credential,'/v1/console/nodes')[0]==401,'explicit revocation immediately retires old bearer')
        expect(bearer_api(credential,'/v1/console/nodes'), 401, 'sign in required', 'explicit revoked credential no longer authorizes')
        click('Check session')
        wait(lambda:js('return document.body.textContent.includes("Sign in to view your telemetry.")'),'policy invalidation after revocation locks browser')
        field('Principal ID',principal_id)
        click('Use passkey')
        wait(lambda:js('return document.body.textContent.includes("Browser fixture owner · human")'),'fresh owner login after credential revocation')
        nav('Settings')
        credential=rotated
        field('Operation','delegation')
        field('Display name or immutable target ID',issuance['principal_id'])
        field('Credential lifetime (seconds)','60')
        narrower=dict(grant,max_query_window_s=1800,max_query_rows=100)
        field('Explicit scope (review every field before issuance)',json.dumps(narrower))
        click('Submit reviewed scoped change')
        wait(lambda:js('return Array.from(document.querySelectorAll("code")).some(e=>/^[a-f0-9]{64}$/.test(e.textContent.trim()))'),'actual narrowed AI delegation issuance')
        delegated_credential=js('return Array.from(document.querySelectorAll("code")).map(e=>e.textContent.trim()).find(value=>/^[a-f0-9]{64}$/.test(value))')
        click('Clear displayed credential')
        check(bearer_api(delegated_credential,'/v1/console/query','POST',dict(read,limit=100))[0]==200,'delegated bearer authorizes narrowed producer query')
        expect(bearer_api(delegated_credential,'/v1/console/query','POST',read),400,'query exceeds authorized bounds','delegated bearer cannot exceed narrower row bound')
        credential=None
        check(api('/v1/console/session')[0] == 200, 'successful issuance preserves unchanged issuer session')
        first_handle=wd('/window',method='GET')
        second_handle=wd('/window/new',{'type':'tab'})['handle']
        wd('/window',{'handle':second_handle})
        wd('/url',{'url':origin+'/console/'})
        wait(lambda:js('return !!document.querySelector("[data-testid=connect-server]")'),'second tab shell')
        click('Connect to server')
        wait(lambda:js('return document.body.textContent.includes("Browser fixture owner · human")'),'second tab validated same session')
        nav('Explore')
        field('Signal','logs')
        field('Source name (blank = authorized scope)','browser-edge')
        field('Body substring (case-sensitive)','browser-page-fixture')
        # The deliberate 902-second expiry wait ages these original rows beyond
        # a fresh tab's default 15-minute preset. Preserve the independently
        # selected producer window for this positive logout witness.
        js('document.querySelector("details.exact-range").open=true')
        field('Start (Unix ns)',str(read['from_ns']));field('End, exclusive (Unix ns)',str(read['to_ns']))
        click('Run query')
        wait(lambda:js('return document.querySelectorAll(".results tbody tr").length===200'),'second tab has positive protected-history witness before logout')
        js('window.fixtureLogoutFetch=window.fetch;window.fixtureLogoutResponseReady=false;window.fixtureLogoutDelivered=false;window.fixtureLogoutLockedAt=null;window.addEventListener("fabric-session-logout",()=>window.fixtureLogoutLockedAt=performance.now(),{once:true});window.fetch=async(...args)=>{const response=await window.fixtureLogoutFetch(...args);const url=typeof args[0]==="string"?args[0]:args[0].url;if(new URL(url,location.href).pathname==="/v1/console/query"){window.fixtureLogoutResponseReady=true;await new Promise(resolve=>setTimeout(resolve,5000));window.fixtureLogoutDeliveredAt=performance.now();window.fixtureLogoutDelivered=true;}return response}')
        click('Run query')
        wait(lambda:js('return window.fixtureLogoutResponseReady'),'real authenticated response is held before cross-tab logout')
        wd('/window',{'handle':first_handle})
        nav('Explore');field('Signal','logs');field('Source name (blank = authorized scope)','browser-edge');field('Body substring (case-sensitive)','browser-page-fixture')
        js('document.querySelector("details.exact-range").open=true')
        field('Start (Unix ns)',str(read['from_ns']));field('End, exclusive (Unix ns)',str(read['to_ns']))
        js('window.fixtureBusyLogoutFetch=window.fetch;window.fixtureBusyLogoutReady=false;window.fixtureBusyLogoutAbort=false;window.fixtureBusyLogoutDelivered=false;window.fixtureBusyLogoutComplete=false;window.fixtureBusyLogoutOrder=[];window.fetch=async(...args)=>{const url=typeof args[0]==="string"?args[0]:args[0].url;const path=new URL(url,location.href).pathname;window.fixtureBusyLogoutOrder.push(path+":start");if(path==="/v1/console/query"){const signal=args[0]?.signal||args[1]?.signal;if(signal)signal.addEventListener("abort",()=>window.fixtureBusyLogoutAbort=true,{once:true});}const response=await window.fixtureBusyLogoutFetch(...args);if(path==="/v1/console/query"){window.fixtureBusyLogoutReady=true;await new Promise(resolve=>setTimeout(resolve,1200));window.fixtureBusyLogoutDelivered=true;}window.fixtureBusyLogoutOrder.push(path+":delivered");if(path==="/v1/console/logout")window.fixtureBusyLogoutComplete=true;return response}')
        click('Run query');wait(lambda:js('return window.fixtureBusyLogoutReady'),'first tab holds actual reply before busy signout')
        logout_cookie=next(item for item in wd('/cookie',method='GET') if item['name'].startswith('__Host-'))
        click('Sign out')
        check(js('return !document.querySelector(".results") && document.body.textContent.includes("Sign in with a passkey") && !document.body.textContent.includes("Browser fixture owner · human")'),'signout clears protected display immediately during pending read')
        wait(lambda:js('return window.fixtureBusyLogoutAbort'),'signout aborts pending real read transport')
        wait(lambda:js('return window.fixtureBusyLogoutDelivered && window.fixtureBusyLogoutComplete && !document.body.textContent.includes("Working…")'),'queued server logout completes after pending read transport')
        logout_order=js('return window.fixtureBusyLogoutOrder')
        receipt['busy_logout_fetch_order']=logout_order
        check(logout_order.count('/v1/console/logout:start')==1 and logout_order.index('/v1/console/query:delivered')<logout_order.index('/v1/console/logout:start'),'busy signout queues one CSRF logout without overlapping protected transports')
        js('window.fetch=window.fixtureBusyLogoutFetch;delete window.fixtureBusyLogoutFetch')
        wait(lambda: js('return document.body.textContent.includes("Signed out. In-memory telemetry cleared.")'), 'actual server logout')
        wd('/window',{'handle':second_handle})
        wait(lambda:js('return document.body.textContent.includes("Sign in to view your telemetry.")'),'cross-tab logout cleared protected views')
        check(js('return document.body.textContent.includes("another tab logged out")'),'broadcast carries logout to same-origin tab')
        wait(lambda:js('return window.fixtureLogoutDelivered && !document.body.textContent.includes("Working…")'),'held real reply completes after actual cross-tab logout')
        check(js('return window.fixtureLogoutLockedAt!==null && window.fixtureLogoutLockedAt<window.fixtureLogoutDeliveredAt && !document.querySelector(".results") && !document.body.textContent.includes("Browser fixture owner · human")'),'late authenticated reply cannot restore protected display after actual logout')
        wd('/window',method='DELETE')
        wd('/window',{'handle':first_handle})
        expect(api('/v1/console/session'), 401, 'sign in required', 'logged-out cookie no longer grants session')
        expect(bearer_api(delegated_credential,'/v1/console/nodes'), 401, 'sign in required', 'parent human logout immediately retires delegation')
        delegated_credential=None
        check(bearer_api(rotated,'/v1/console/nodes')[0]==200,'ordinary scoped workload remains independent of human logout')
        recovery_credential=rotated
        rotated=None
        request=urllib.request.Request(origin+'/v1/console/session',headers={
            'Cookie':logout_cookie['name']+'='+logout_cookie['value'],'x-fabric-client-version':'1'})
        try:
            urllib.request.urlopen(request,context=context,timeout=10)
            raise AssertionError('copied stale session cookie unexpectedly accepted')
        except urllib.error.HTTPError as error:
            expect((error.code,json.load(error)),401,'sign in required','replayed stale session cookie rejected by server')
        logout_cookie=None
        field('Principal ID', principal_id)
        click('Use passkey')
        wait(lambda: js('return document.body.textContent.includes("Browser fixture owner · human")'), 'actual WebAuthn assertion login')
        nav('Settings')
        current_cookie=next(item for item in wd('/cookie',method='GET') if item['name'].startswith('__Host-'))
        current_session_id=hashlib.sha256(current_cookie['value'].encode()).hexdigest()
        current_cookie=None
        code,sessions=api('/v1/console/sessions')
        check(code==200 and any(item['id']==current_session_id for item in sessions),'current session has immutable revocation target')
        field('Exact passkey, session, principal or credential ID',current_session_id)
        click('revoke sessions record');wd('/alert/accept',{})
        wait(lambda:api('/v1/console/session')[0]==401,'self session revocation retires current cookie')
        expect(api('/v1/console/session'),401,'sign in required','revoked session cannot authorize')
        click('Check session')
        wait(lambda:js('return document.body.textContent.includes("Sign in to view your telemetry.")'),'revoked session locks UI')
        field('Principal ID',principal_id);click('Use passkey')
        wait(lambda:js('return document.body.textContent.includes("Browser fixture owner · human")'),'fresh login after self-session revocation')
        nav('Settings')
        field('Operation','invite')
        field('Display name or immutable target ID','Browser fixture human')
        invite_grant=dict(grant,max_query_window_s=60,max_query_rows=100)
        field('Explicit scope (review every field before issuance)',json.dumps(invite_grant))
        click('Submit reviewed scoped change')
        wait(lambda:js('return Array.from(document.querySelectorAll("code")).some(e=>/^[a-f0-9]{64}$/.test(e.textContent.trim()))'),'actual local human invitation issuance')
        invitation=js('return Array.from(document.querySelectorAll("code")).map(e=>e.textContent.trim()).find(value=>/^[a-f0-9]{64}$/.test(value))')
        click('Clear displayed credential')
        click('Sign out')
        wait(lambda:js('return document.body.textContent.includes("Sign in to view your telemetry.")'),'owner sign out before account change')
        remove_authenticator(authenticator)
        authenticator=add_authenticator()
        field('One-time invitation',invitation)
        invitation=None
        click('Enroll invited passkey')
        wait(lambda:js('return document.body.textContent.includes("Browser fixture human · human")'),'actual invited human WebAuthn registration')
        check(js('return !document.querySelector(".results")'),'account change contains no previous telemetry display')
        code,invited_inventory=api('/v1/console/nodes')
        check(code==200 and len(invited_inventory['nodes'])==1 and all(n['enrollment_id']==enrollment for n in invited_inventory['nodes']),'invited human inventory obeys explicit immutable scope')
        expect(api('/v1/console/principals'), 403, 'access denied', 'invited reader cannot administer identities')
        check(edge.poll() is None,'native producer remains running after bounded expiry and polling')
        producer_offset=(out/'edge.log').stat().st_size
        with fixture_log.open('a') as source:source.write(''.join(f'browser-invited-scope-fixture ordinal={i:03d}\n' for i in range(200)))
        def invited_producer_acked():
            if edge.poll() is not None:
                raise AssertionError('native scoped-query producer stopped before fresh delivery')
            with (out/'edge.log').open() as source:
                source.seek(producer_offset);events=source.read()
            batches=re.findall(r'batch=(\d+) metrics=\d+ logs=(\d+)',events)
            committed=[int(value) for value in re.findall(r'status=ack committed_through=(\d+)',events)]
            return bool(committed) and sum(int(logs) for sequence,logs in batches if int(sequence)<=max(committed))>=200
        wait(invited_producer_acked,'independent producer commits all200 new scoped-query observations')
        nav('Explore');field('Signal','logs');field('Source name (blank = authorized scope)','browser-edge');field('Body substring (case-sensitive)','browser-invited-scope-fixture')
        def invited_query_ready():
            if js('return document.querySelectorAll(".results tbody tr").length===100'):return True
            if not js('return document.body.textContent.includes("Working…")'):
                pace_api();click('Last 60 seconds (grant cap)');click('Run query')
            return False
        wait(invited_query_ready,'actual scoped browser query has 100 producer rows within 60-second grant')
        invited_rows=[json.loads(value) for value in js('return Array.from(document.querySelectorAll(".results tbody .live-json")).map(e=>e.textContent)')]
        check(len(invited_rows)==100 and all(f'ordinal={i:03d}' in row['body'] for i,row in enumerate(invited_rows)),'scoped UI row cap matches independent new producer ordinals')
        applied_query=next(value for value in (json.loads(text) for text in js('return Array.from(document.querySelectorAll(".results > details .live-json")).map(e=>e.textContent)')) if isinstance(value,dict) and value.get('kind')=='logs')
        check(applied_query['limit']==100 and applied_query['to_ns']-applied_query['from_ns']<=60_000_000_000,'scope-limited UI sends exact 100-row and 60-second authorized query')
        js('window.fixtureScopeFetch=window.fetch;window.fixtureScopeFetchCount=0;window.fetch=(...args)=>{window.fixtureScopeFetchCount++;return window.fixtureScopeFetch(...args)};document.querySelector("details.exact-range").open=true')
        field('Start (Unix ns)',str(applied_query['to_ns']-900_000_000_000));field('End, exclusive (Unix ns)',str(applied_query['to_ns']))
        click('Run query')
        wait(lambda:js('return document.querySelector("[role=alert]").textContent.includes("60-second")'),'explicit over-grant query reports client validation error')
        check(js('return window.fixtureScopeFetchCount===0'),'invalid 900-second range rejected before fetch within 60-second grant')
        js('window.fetch=window.fixtureScopeFetch;delete window.fixtureScopeFetch')
        receipt['actual_api_headers']=js('return window.fixtureApiHeaders')
        check(bool(receipt['actual_api_headers']) and not js('return window.fixtureApiHeadersTruncated') and all(item['cache_control']=='no-store' and item['csp'] and item['coop']=='same-origin' and item['corp']=='same-origin' for item in receipt['actual_api_headers']),'actual authentication and API responses carry no-store CSP COOP and CORP')
        if args.browser=='chrome':
            cdp('Network.enable',{})
            cdp('Network.emulateNetworkConditions',{'offline':True,'latency':0,'downloadThroughput':0,'uploadThroughput':0})
            wd('/url',{'url':'about:blank'})
            wd('/url',{'url':origin+'/console/'})
            wait(lambda:js('return !!document.querySelector("[data-testid=locked-screen]")'),'real offline cached public shell reopened')
            check(js('return !document.querySelector(".results") && !document.body.textContent.includes("browser-page-fixture")'),'reopened offline shell contains no previous sensitive rows')
            check(async_js('const done=arguments[arguments.length-1];fetch("/v1/console/session",{headers:{"x-fabric-client-version":"1"}}).then(()=>done(false)).catch(()=>done(true))'),'offline API request fails rather than replaying cached identity')
            cdp('Network.emulateNetworkConditions',{'offline':False,'latency':0,'downloadThroughput':-1,'uploadThroughput':-1})
        check(js('return !document.body.textContent.includes("Partial fixture")'), 'live workspace contains no synthetic evidence badges')
        if args.recovery:
            os.killpg(server.pid,signal.SIGTERM);server.wait(timeout=15)
            recovery=subprocess.run([str(args.server.resolve()),'recover-access',str(config),principal_id],capture_output=True,text=True,timeout=20)
            (out/'owner-recovery.log').write_text(recovery.stdout+recovery.stderr)
            check(recovery.returncode==0,'actual offline owner recovery command succeeds after server stop')
            server=launch('server-recovered',[str(args.server.resolve()),'serve',str(config)])
            wait(healthy,'production server reopened after owner recovery')
            expect(bearer_api(recovery_credential,'/v1/console/nodes'),401,'sign in required','offline recovery invalidates previously issued workload credential')
            recovery_credential=None
            wd('/url',{'url':'about:blank'});wd('/url',{'url':origin+'/console/'})
            wait(lambda:js('return !!document.querySelector("[data-testid=connect-server]")'),'recovery public shell')
            click('Connect to server')
            wait(lambda:js('return document.body.textContent.includes("Sign in with a passkey") && !document.body.textContent.includes("Working…") && document.querySelector("[role=alert]")?.textContent.includes("HTTP 401: sign in required")'),'old identity session rejected after recovery epoch change')
            remove_authenticator(authenticator);authenticator=add_authenticator()
            replacement_secret=bootstrap_file.read_text().strip()
            js('Array.from(document.querySelectorAll("details")).find(e=>e.querySelector("button")?.textContent.trim()==="Create owner passkey").open=true')
            field('Display name','Browser fixture owner')
            field('One-time setup secret',replacement_secret);replacement_secret=None
            click('Create owner passkey')
            wait(lambda:js('return document.body.textContent.includes("Browser fixture owner · human")'),'real WebAuthn owner enrollment after offline recovery')
            code,recovered_session=api('/v1/console/session')
            check(code==200 and recovered_session['principal_id']==principal_id,'offline owner recovery preserves immutable owner principal')
            check(not bootstrap_file.exists(),'replacement recovery secret consumed once')
        if args.shell_lifecycle:
            check(args.browser=='chrome','shell lifecycle fixture uses Chrome')
            worker=work/'console/service-worker.js'
            original_worker=worker.read_text()
            original_cache=original_worker.split('const CACHE = ')[1].split(';')[0].strip('"')
            def restart_server(worker_text):
                nonlocal server
                os.killpg(server.pid,signal.SIGTERM);server.wait(timeout=15)
                worker.write_text(worker_text)
                manifest_path=work/'console/asset-manifest.json'
                asset_manifest=json.loads(manifest_path.read_text())
                content=worker.read_bytes()
                asset_manifest['service-worker.js']={'bytes':len(content),'sha256':hashlib.sha256(content).hexdigest()}
                manifest_path.write_text(json.dumps(asset_manifest)+'\n')
                server=launch('server-lifecycle-'+str(len(processes)),[str(args.server.resolve()),'serve',str(config)])
                wait(healthy,'restarted production TLS server')
            def worker_state():
                return async_js('const done=arguments[arguments.length-1];navigator.serviceWorker.getRegistration().then(async r=>done({active:r?.active?.state,waiting:r?.waiting?.state,caches:await caches.keys()}))')
            def request_update():
                return async_js('const done=arguments[arguments.length-1];navigator.serviceWorker.getRegistration().then(r=>r.update()).then(()=>done(true)).catch(error=>done({error:String(error)}))')
            def close_controlled_tab():
                old_handle=wd('/window',method='GET')
                blank_handle=wd('/window/new',{'type':'tab'})['handle']
                wd('/window',{'handle':old_handle});wd('/window',method='DELETE')
                wd('/window',{'handle':blank_handle})
                async_js('const done=arguments[arguments.length-1];setTimeout(()=>done(true),500)')
                wd('/url',{'url':origin+'/console/'})
            failed_cache=original_cache+'-interrupted'
            failed_worker=original_worker.replace(original_cache,failed_cache).replace('const ASSETS = [','const ASSETS = ["/console/fixture-missing-shell.js",')
            restart_server(failed_worker)
            failed_update=request_update()
            wait(lambda:failed_cache not in worker_state()['caches'],'interrupted candidate cache removed')
            check(not worker_state()['waiting'] and original_cache in worker_state()['caches'],'interrupted update retains coherent active public shell')
            receipt['interrupted_shell_update']=failed_update
            candidate_cache=original_cache+'-candidate'
            restart_server(original_worker.replace(original_cache,candidate_cache))
            request_update()
            wait(lambda:worker_state()['waiting']=='installed','new shell waits while current tab remains open')
            check(original_cache in worker_state()['caches'] and candidate_cache in worker_state()['caches'],'open tab retains old shell alongside coherent waiting candidate')
            close_controlled_tab()
            wait(lambda:js('return !!navigator.serviceWorker.controller') and worker_state()['caches']==[candidate_cache],'candidate activates after old controlled tab closes')
            check(js('return !!document.querySelector("[data-testid=locked-screen]")'),'updated shell reopens locked after server restart')
            restart_server(original_worker)
            request_update()
            wait(lambda:worker_state()['waiting']=='installed','rollback shell waits without forced takeover')
            close_controlled_tab()
            wait(lambda:js('return !!navigator.serviceWorker.controller') and worker_state()['caches']==[original_cache],'rollback returns one coherent original shell')
            receipt['shell_lifecycle_scope']='real worker install/activation/rollback using same application assets and staged cache identities; no application schema migration claim'
            manifest_id=origin+'/console/'
            try:
                cdp('PWA.install',{'manifestId':manifest_id,'installUrlOrBundleUrl':manifest_id})
                cdp('PWA.changeAppUserSettings',{'manifestId':manifest_id,'displayMode':'standalone'})
                ordinary_handle=wd('/window',method='GET')
                launched=cdp('PWA.launch',{'manifestId':manifest_id})
                receipt['installed_pwa']={'target_id':launched['targetId'],'user_display_preference':'standalone','os_state':cdp('PWA.getOsAppState',{'manifestId':manifest_id})}
                wd('/window',{'handle':launched['targetId']})
                wait(lambda:js('return !!document.querySelector("[data-testid=locked-screen]")'),'actual installed PWA renders locked public shell')
                check(js('return matchMedia("(display-mode: standalone)").matches'),'installed PWA uses standalone display mode')
                check(js('return !document.querySelector(".results") && !document.body.textContent.includes("browser-page-fixture")'),'installed PWA opens without prior sensitive display')
                wd('/window',{'handle':ordinary_handle})
                cdp('Target.closeTarget',{'targetId':launched['targetId']})
                cdp('PWA.uninstall',{'manifestId':manifest_id})
                check(bool(launched['targetId']),'isolated desktop PWA installs launches and uninstalls')
            except BaseException:
                try:cdp('PWA.uninstall',{'manifestId':manifest_id})
                except RuntimeError:pass
                raise
        receipt['native_input_hashes_after']={name:hashlib.sha256(Path(name).read_bytes()).hexdigest() for name in receipt['binaries']}
        receipt['native_snapshot_hashes_after']={role:hashlib.sha256(getattr(args,role).read_bytes()).hexdigest() for role in ('server','spindle')}
        check(receipt['native_input_hashes_after']==receipt['binaries'],'native input artifact identities remain unchanged throughout run')
        check(all(receipt['native_snapshot_hashes_after'][role]==identity['sha256'] for role,identity in receipt['immutable_binary_snapshots'].items()),'all fixture launches retain immutable candidate binary bytes')
        receipt['exit'] = 0
    except BaseException as error:
        receipt['error'] = str(error)
        if session:
            try:
                receipt['failure_browser_windows']=wd('/window/handles',method='GET')
                receipt['failure_worker_state']=async_js('const done=arguments[arguments.length-1];if(!navigator.serviceWorker){done(null);return;}navigator.serviceWorker.getRegistration().then(async r=>done({active:r?.active?.state,waiting:r?.waiting?.state,caches:await caches.keys()})).catch(error=>done({error:String(error)}))')
                receipt['ceremony_diagnostic']=js('return window.fixtureCeremonyDiagnostic||null')
                if args.browser=='firefox':
                    wd('/moz/context',{'context':'chrome'})
                    try:
                        receipt['firefox_authenticator_diagnostic']=js('''const auth=ChromeUtils.importESModule("chrome://remote/content/marionette/webauthn.sys.mjs").webauthn;
                          return {exists:auth.hasVirtualAuthenticator(arguments[0]),credential_count:auth.getCredentials(arguments[0]).length,
                          ctap2:Services.prefs.getBoolPref("security.webauthn.ctap2",false),
                          usb:Services.prefs.getBoolPref("security.webauth.webauthn_enable_usbtoken",false),
                          prompt:document.querySelector("#notification-popup")?.textContent||null};''',authenticator)
                    finally:
                        wd('/moz/context',{'context':'content'})
                js('document.querySelectorAll("code").forEach(e=>{if(/^[a-f0-9]{64}$/.test(e.textContent.trim()))e.textContent="[fixture secret redacted]"})')
                (out/'failure.png').write_bytes(base64.b64decode(wd('/screenshot', method='GET')))
                (out/'failure-text.txt').write_text(js('return document.body.innerText'))
            except BaseException:
                pass
        raise
    finally:
        if session:
            try:
                call('DELETE', '/session/' + session)
            except BaseException:
                pass
        for label, proc in reversed(processes):
            if proc.poll() is None:
                os.killpg(proc.pid, signal.SIGTERM)
                try:
                    proc.wait(timeout=15)
                except subprocess.TimeoutExpired:
                    os.killpg(proc.pid, signal.SIGKILL)
                    proc.wait(timeout=5)
            receipt.setdefault('process_exits', {})[label] = proc.returncode
        stop_samples.set()
        sampler.join(timeout=2)
        (out/'resource-samples.json').write_text(json.dumps(samples, indent=2) + '\n')
        for log in logs:
            log.close()
        # Do not archive private keys, bootstrap secrets, browser credentials or cookies.
        shutil.rmtree(work)
        receipt['cleanup'] = 'owned TLS/server/browser fixtures removed; process groups stopped; screenshots/logs retained'
        receipt['remaining_uncertainty'] = 'virtual authenticator only; no physical/synced passkeys, OS trust installation, mobile OS, general capacity or release qualification'
        (out/'browser.json').write_text(json.dumps(receipt, indent=2) + '\n')
    print(json.dumps({'exit':receipt['exit'],'checks':len(checks),'out':str(out)}))


if __name__ == '__main__':
    main()
