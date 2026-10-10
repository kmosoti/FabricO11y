"""Contained Firefox checks of the real compiled console, using WebDriver."""
import argparse
import base64
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parents[2]
DATA = Path('/run/media/kmosoti/data/FabricO11y')
sys.path.insert(0, str(ROOT / 'tools'))
from resource_group import require_limits


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--build', required=True, type=Path)
    parser.add_argument('--out', required=True, type=Path)
    parser.add_argument('--inject-api-cache-defect', action='store_true')
    args = parser.parse_args()
    require_limits()
    build = args.build.resolve()
    out = args.out.resolve()
    if not build.is_relative_to(DATA / 'results') or not out.is_relative_to(DATA / 'results') or out.exists():
        raise SystemExit('build/output must be owned data-drive results; output fresh')
    out.mkdir()
    build_receipt = json.loads((build / 'build.json').read_text())
    dist = build / 'dist'
    sentinel = 'sensitive-browser-fixture-do-not-cache'
    requests = []
    class Handler(SimpleHTTPRequestHandler):
        def translate_path(self, path):
            # No arbitrary repository serving, even on this loopback test listener.
            name = path.split('?', 1)[0].removeprefix('/console/') or 'index.html'
            return str(dist / name) if '/' not in name and name not in ('.', '..') else str(dist / '__missing__')
        def do_GET(self):
            requests.append(self.path)
            if args.inject_api_cache_defect and self.path == '/console/service-worker.js':
                body = (dist / 'service-worker.js').read_bytes() + b'''
self.addEventListener("fetch", event => {
  if (new URL(event.request.url).pathname.startsWith("/v1/")) {
    event.respondWith(fetch(event.request).then(async response => {
      await (await caches.open("fabric-shell-defect")).put(event.request, response.clone());
      return response;
    }));
  }
});
'''
                self.send_response(200)
                self.send_header('Content-Type', 'text/javascript')
                self.end_headers()
                self.wfile.write(body)
            elif self.path.startswith('/v1/'):
                body = json.dumps({'fixture': sentinel}).encode()
                self.send_response(200)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Cache-Control', 'no-store')
                self.end_headers()
                self.wfile.write(body)
            elif self.path.startswith('/console/'):
                super().do_GET()
            else:
                self.send_error(404)
        def end_headers(self):
            for k, v in build_receipt['headers'].items():
                self.send_header(k, v)
            self.send_header('Cache-Control', 'no-store')
            super().end_headers()
        def log_message(self, *args):
            pass
    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        port = sock.getsockname()[1]
    endpoint = f'http://127.0.0.1:{port}'
    profile = Path(tempfile.mkdtemp(prefix='ui-browser-', dir=os.environ['FABRIC_SCRATCH_ROOT']))
    log = (out / 'webdriver.log').open('w')
    driver = subprocess.Popen([str(DATA / 'tools/ui/geckodriver'), '--host', '127.0.0.1', '--port', str(port)],
                              stdout=log, stderr=subprocess.STDOUT)
    session = None
    checks = []
    receipt = {'build': str(build), 'checks': checks, 'exit': 1,
               'injected_api_cache_defect': args.inject_api_cache_defect}
    def call(method, path, body=None):
        data = None if body is None else json.dumps(body).encode()
        request = urllib.request.Request(endpoint + path, data=data, method=method,
                                         headers={'Content-Type': 'application/json'})
        try:
            with urllib.request.urlopen(request, timeout=45) as r:
                answer = json.load(r)
        except urllib.error.HTTPError as e:
            raise RuntimeError(e.read().decode()) from e
        return answer.get('value')
    def wd(path, body=None, method='POST'):
        return call(method, f'/session/{session}' + path, body)
    def js(script, *values):
        return wd('/execute/sync', {'script': script, 'args': values})
    def wait(predicate, label):
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            if predicate(): return
            time.sleep(.1)
        raise AssertionError('timeout: ' + label)
    def check(condition, label):
        if not condition: raise AssertionError(label)
        checks.append(label)
    def click(selector):
        element = wd('/element', {'using': 'css selector', 'value': selector})
        key = element['element-6066-11e4-a52e-4f735466cecf']
        wd('/element/' + key + '/click', {})
    def nav(title):
        js('Array.from(document.querySelectorAll(".sidebar .nav-item")).find(e=>e.textContent.includes(arguments[0])).click()', title)
    def screenshot(name):
        # Capture settled UI rather than an intermediate theme-transition frame.
        wd('/execute/async', {'script': '''const done=arguments[arguments.length-1];
          Promise.all(document.getAnimations().filter(a =>
            a.effect.getTiming().iterations !== Infinity).map(a =>
              a.finished.catch(() => {}))).then(() => done(true));''', 'args': []})
        (out / name).write_bytes(base64.b64decode(wd('/screenshot', method='GET')))
    try:
        wait(lambda: driver.poll() is None and _ready(endpoint), 'geckodriver')
        info = call('POST', '/session', {'capabilities': {'alwaysMatch': {
            'browserName': 'firefox', 'moz:firefoxOptions': {'binary': '/usr/bin/firefox',
            'args': ['-headless', '-profile', str(profile)],
            'prefs': {'browser.shell.checkDefaultBrowser': False, 'browser.startup.page': 0}}
        }}})
        session = info['sessionId']
        receipt['browser'] = info['capabilities']['browserVersion']
        wd('/window/rect', {'width': 1440, 'height': 1100})
        origin = f'http://127.0.0.1:{server.server_port}'
        wd('/url', {'url': origin + '/console/'})
        wait(lambda: js('return !!document.querySelector("[data-testid=enter-demo]")'), 'WASM mount')
        check(js('return !!document.querySelector("[data-testid=locked-screen]")'), 'default is locked')
        click('[data-testid=enter-demo]')
        wait(lambda: js('return document.body.textContent.includes("Memory observations")'), 'demo overview')
        check(js('return document.body.textContent.includes("DEMONSTRATION")'), 'fixture labeled')
        screenshot('overview-light.png')
        click('.theme-button')
        check(js('return document.querySelector(".console").classList.contains("dark")'), 'theme toggles')
        screenshot('overview-dark.png')
        nav('Explore')
        check(js('return document.querySelectorAll("tbody tr").length === 6'), 'six initial log rows')
        check(js('return !window.__fixture_xss && Array.from(document.querySelectorAll("td")).some(e=>e.textContent.includes("<script>"))'), 'untrusted log rendered as text')
        js('const e=document.querySelector("input");e.value="😃".repeat(1025);e.dispatchEvent(new Event("input",{bubbles:true}))')
        click('[data-testid=run-query]')
        check(js('return document.querySelector("input").value === "" && document.querySelector("input").getAttribute("aria-invalid") === "true" && document.querySelectorAll("tbody tr").length === 6'), 'oversized UTF-8 filter rejected without changing results')
        js('const e=document.querySelector("input");e.value="NO-MATCH";e.dispatchEvent(new Event("input",{bubbles:true}))')
        click('[data-testid=run-query]')
        check(js('return document.body.textContent.includes("No fixture log")'), 'empty filter is explicit')
        nav('Trace detail')
        check(js('return document.querySelectorAll(".span-row").length === 4'), 'four trace rows')
        check(js('return document.querySelectorAll(".span-track rect").length === 4'), 'CSP-safe trace geometry')
        click('[data-testid=related-logs]')
        check(js('return document.querySelectorAll("tbody tr").length === 2'), 'related logs exactly two')
        nav('Live Tail')
        for _ in range(10): click('[data-testid=tail-advance]')
        check(js('return document.querySelectorAll(".tail-line").length === 6'), 'tail remains six rows')
        click('[data-testid=tail-pause]')
        check(js('return document.querySelector("[data-testid=tail-advance]").disabled'), 'pause prevents advance')
        nav('Overview')
        wd('/window/rect', {'width': 390, 'height': 844})
        check(js('return document.documentElement.scrollWidth <= window.innerWidth'), 'mobile no page overflow')
        check(js('return getComputedStyle(document.querySelector(".mobile-nav")).display !== "none"'), 'mobile navigation present')
        screenshot('overview-mobile.png')
        receipt['viewport'] = js('return {width:innerWidth,height:innerHeight,dpr:devicePixelRatio}')
        # Firefox WebDriver refresh can bypass workers; exercise an ordinary new
        # navigation, the same path as reopening an installed app (no takeover).
        receipt['registration'] = wd('/execute/async', {'script': 'const done=arguments[arguments.length-1];navigator.serviceWorker.ready.then(r=>done({scope:r.scope,state:r.active.state,url:r.active.scriptURL}))', 'args': []})
        wd('/url', {'url': 'about:blank'})
        wd('/url', {'url': origin + '/console/'})
        wait(lambda: js('return !!navigator.serviceWorker.controller'), 'worker controls new navigation')
        wait(lambda: js('return !!document.querySelector("[data-testid=locked-screen]")'), 'locked reopen')
        check(js('return !!document.querySelector("[data-testid=locked-screen]")'), 'reload clears demonstration memory')
        cache = wd('/execute/async', {'script': '''const done=arguments[arguments.length-1];
          fetch('/v1/browser-fixture').then(r=>r.json()).then(async()=>{
            const keys=await caches.keys(); const paths=[];
            for(const key of keys) for(const r of await (await caches.open(key)).keys()) paths.push(r.url);
            done({keys,paths,local:localStorage.length,session:sessionStorage.length});
          }).catch(e=>done({error:String(e)}));''', 'args': []})
        check(bool(cache.get('keys')), 'shell cache exists')
        check(all('/console/' in p and '/v1/' not in p for p in cache['paths']), 'API bypasses shell cache')
        check(cache['local'] == 0 and cache['session'] == 0, 'no browser key-value telemetry')
        receipt['cache'] = cache
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
        wd('/url', {'url': 'about:blank'})
        wd('/url', {'url': origin + '/console/'})
        wait(lambda: js('return !!document.querySelector("[data-testid=locked-screen]")'), 'offline shell mount')
        check(js('return !document.body.textContent.includes("request started trace=")'), 'offline shell has no telemetry')
        receipt['requests'] = requests
        receipt['exit'] = 0
    except BaseException as e:
        receipt['failure'] = str(e)
        if session:
            try: screenshot('failure.png')
            except Exception: pass
        raise
    finally:
        if session:
            try: wd('', method='DELETE')
            except Exception: pass
        driver.terminate()
        try: driver.wait(timeout=10)
        except subprocess.TimeoutExpired:
            driver.kill(); driver.wait()
        server.shutdown(); server.server_close(); thread.join(timeout=5)
        log.close()
        shutil.rmtree(profile)
        receipt['profile_removed'] = not profile.exists()
        receipt['processes_stopped'] = driver.poll() is not None and not thread.is_alive()
        receipt['requests'] = requests
        (out / 'browser.json').write_text(json.dumps(receipt, indent=2) + '\n')
        print(json.dumps({'checks': len(checks), 'exit': receipt['exit'], 'out': str(out)}))


def _ready(endpoint):
    try:
        with urllib.request.urlopen(endpoint + '/status', timeout=1) as r:
            return r.status == 200
    except OSError:
        return False


if __name__ == '__main__':
    main()
