"""Build the bounded console into owned data-drive storage; no deployment."""
import argparse
import base64
import gzip
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
DATA = Path('/run/media/kmosoti/data/FabricO11y')
sys.path.insert(0, str(ROOT / 'tools'))
from resource_group import require_limits


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    require_limits()
    if not DATA.parent.is_mount() or not args.out.is_absolute():
        raise SystemExit('mounted data drive and absolute output required')
    out = args.out.resolve()
    if not out.is_relative_to(DATA / 'results') or out.exists():
        raise SystemExit('output must be a fresh directory below data-drive results')
    out.mkdir(parents=True)
    env = dict(os.environ, CARGO_HOME=str(DATA / 'toolchain-cache/cargo'),
               RUSTUP_HOME=str(DATA / 'toolchain-cache/rustup'),
               RUSTUP_TOOLCHAIN='1.99.0', CARGO_BUILD_JOBS='2',
               XDG_CACHE_HOME=str(DATA / 'cache/ui'),
               CARGO_PROFILE_RELEASE_OPT_LEVEL='s',
               CARGO_PROFILE_RELEASE_LTO='thin',
               CARGO_PROFILE_RELEASE_CODEGEN_UNITS='1')
    command = [str(DATA / 'tools/ui/trunk'), 'build', '--release', '--locked',
               '--dist', str(out / 'dist'), '--public-url', '/console/']
    receipt = {'command': command, 'source': str(ROOT / 'crates/fabric-ui'),
               'out': str(out), 'exit': None, 'artifact_role': 'console foundation; not authenticated release'}
    inputs = sorted(p for p in (ROOT / 'crates/fabric-ui').rglob('*') if p.is_file())
    inputs += [ROOT / 'Cargo.lock', Path(__file__).resolve()]
    receipt['source_sha256'] = {str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest() for p in inputs}
    receipt['tools'] = {name: json.loads((DATA / ('tools/ui/' + name + '.json')).read_text()) for name in ['trunk']}
    try:
        with (out / 'build.log').open('w') as log:
            result = subprocess.run(command, cwd=ROOT / 'crates/fabric-ui', env=env,
                                    stdout=log, stderr=subprocess.STDOUT)
        receipt['build_exit'] = result.returncode
        if result.returncode:
            receipt['exit'] = result.returncode
            print((out / 'build.log').read_text()[-10000:])
            raise SystemExit(result.returncode)
        dist = out / 'dist'
        for size in (192, 512):
            subprocess.run(['magick', '-limit', 'memory', '64MiB', '-limit', 'map', '64MiB',
                            str(dist / 'icon.svg'), '-resize', f'{size}x{size}',
                            str(dist / f'icon-{size}.png')], check=True)
        files = sorted(p for p in dist.iterdir() if p.is_file())
        manifest = {p.name: {'sha256': hashlib.sha256(p.read_bytes()).hexdigest(),
                             'bytes': p.stat().st_size,
                             'gzip_bytes': len(gzip.compress(p.read_bytes(), mtime=0))}
                    for p in files}
        if sum(v['bytes'] for v in manifest.values()) > 8 * 1024 * 1024:
            receipt['exit'] = 2
            raise SystemExit('public shell exceeds the 8 MiB build budget')
        version = hashlib.sha256(json.dumps(manifest, sort_keys=True).encode()).hexdigest()[:20]
        allowed = ['/console/' + p.name for p in files]
        worker = '''// Generated allowlist: public shell only. Never handles API/auth data.
const CACHE = "fabric-shell-__VERSION__";
const ASSETS = __ASSETS__;
self.addEventListener("install", event => {
  event.waitUntil((async () => {
    const previous = (await caches.keys()).filter(key => key.startsWith("fabric-shell-"));
    try {
      await (await caches.open(CACHE)).addAll(ASSETS);
    } catch (error) {
      if (!previous.includes(CACHE)) await caches.delete(CACHE);
      throw error;
    }
    // Keep the oldest active shell plus this candidate; retire replaced waiting
    // candidates while tabs keep the active worker alive. No forced takeover.
    await Promise.all(previous.filter(key => key !== previous[0] && key !== CACHE)
      .map(key => caches.delete(key)));
  })());
});
self.addEventListener("activate", event => {
  event.waitUntil(caches.keys().then(keys => Promise.all(keys.filter(key =>
    key.startsWith("fabric-shell-") && key !== CACHE).map(key => caches.delete(key)))));
});
self.addEventListener("fetch", event => {
  const request = event.request;
  const url = new URL(request.url);
  if (request.method !== "GET" || url.origin !== self.location.origin ||
      url.search || request.headers.has("Authorization")) return;
  const path = url.pathname === "/console/" ? "/console/index.html" : url.pathname;
  if (!ASSETS.includes(path)) return;
  event.respondWith(caches.open(CACHE).then(async cache =>
    (await cache.match(path)) || fetch(request)));
});
'''.replace('__VERSION__', version).replace('__ASSETS__', json.dumps(allowed))
        (dist / 'service-worker.js').write_text(worker)
        html = (dist / 'index.html').read_text()
        hashes = ["'sha256-" + base64.b64encode(hashlib.sha256(script.encode()).digest()).decode() + "'"
                  for script in re.findall(r'<script[^>]*>(.*?)</script>', html, re.S) if script.strip()]
        receipt['headers'] = {
            'Content-Security-Policy': "default-src 'self'; script-src 'self' 'wasm-unsafe-eval' " + ' '.join(hashes) + "; style-src 'self'; img-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'",
            'X-Content-Type-Options': 'nosniff', 'Referrer-Policy': 'no-referrer',
        }
        manifest['service-worker.js'] = {'sha256': hashlib.sha256(worker.encode()).hexdigest(), 'bytes': len(worker.encode())}
        receipt.update(assets=manifest, cache_version=version,
                       total_bytes=sum(v['bytes'] for v in manifest.values()))
        if receipt['total_bytes'] > 8 * 1024 * 1024:
            receipt['exit'] = 2
            raise SystemExit('public shell including worker exceeds the 8 MiB build budget')
        if any(hashlib.sha256(p.read_bytes()).hexdigest() != receipt['source_sha256'][str(p.relative_to(ROOT))] for p in inputs):
            receipt['exit'] = 2
            raise SystemExit('source changed during build; artifact is not frozen')
        receipt['exit'] = 0
        print(json.dumps({'out': str(out), 'bytes': receipt['total_bytes'], 'assets': len(manifest)}))
    finally:
        (out / 'build.json').write_text(json.dumps(receipt, indent=2) + '\n')


if __name__ == '__main__':
    main()
