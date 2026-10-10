#!/usr/bin/env python3
"""Self-test the read-only manifest checker against an owned synthetic Git repo."""
from pathlib import Path
import importlib.util
import json
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('verify_base', ROOT / 'verify_base.py')
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


def command(root, *args):
    return subprocess.run(['git', '-C', str(root), *args], check=True,
                          capture_output=True, text=True, timeout=10).stdout.strip()


def main():
    checks = []
    with tempfile.TemporaryDirectory(prefix='manifest-selftest-', dir=Path(__file__).parent) as name:
        root = Path(name)
        command(root, 'init', '-q')
        command(root, 'config', 'user.name', 'Specification Fixture')
        command(root, 'config', 'user.email', 'fixture@example.invalid')
        data = b'first\nfn anchor() {}\nlast\n'
        (root / 'demo.rs').write_bytes(data)
        command(root, 'add', 'demo.rs')
        command(root, '-c', 'core.hooksPath=/dev/null', 'commit', '-qm', 'Synthetic fixture')
        base = command(root, 'rev-parse', 'HEAD')
        manifest = {'base_commit':base,
                    'locked_sources':[{'path':'demo.rs','blob_sha':module.blob_hash(data)}],
                    'edits':[{'id':'S1','path':'demo.rs','original_start_line':2,
                              'original_end_line':2,'symbol_or_text_anchor':'fn anchor'}]}
        code, result = module.verify(root, manifest)
        assert code == 0
        checks.append('matching source and anchor: exit0')
        (root / 'demo.rs').write_text('changed\n')
        code, result = module.verify(root, manifest)
        assert code == 3 and result['files_requiring_reanchor'] == ['demo.rs']
        checks.append('working tree drift: exit3')
        (root / 'demo.rs').unlink()
        (root / 'target.rs').write_bytes(data)
        (root / 'demo.rs').symlink_to(root / 'target.rs')
        assert module.verify(root, manifest)[0] == 3
        checks.append('symlink working file: exit3')
        (root / 'demo.rs').unlink()
        (root / 'demo.rs').write_bytes(data)
        for mutation in ('blob','anchor','range'):
            candidate = json.loads(json.dumps(manifest))
            if mutation == 'blob':
                candidate['locked_sources'][0]['blob_sha'] = '0' * 40
            elif mutation == 'anchor':
                candidate['edits'][0]['symbol_or_text_anchor'] = 'does not exist'
            else:
                candidate['edits'][0]['original_end_line'] = 100
            try:
                module.verify(root, candidate)
            except ValueError:
                checks.append(mutation + ' defect rejected')
            else:
                raise AssertionError(mutation + ' defect not rejected')
        for unsafe in ('../outside', '/absolute'):
            try:
                module.safe_path(unsafe)
            except ValueError:
                checks.append('unsafe manifest path rejected: ' + unsafe)
            else:
                raise AssertionError('unsafe path accepted')
    result = {'status':'passed','exit':0,'checks':checks,
              'owned_fixture_cleanup_complete':not root.exists(),
              'scope':'bundle manifest checker only; not FabricO11y code',
              'fabric_checkout_verified':False}
    (ROOT / 'manifest-checker-selftest.json').write_text(json.dumps(result,indent=2)+'\n')
    print(json.dumps(result,indent=2))

if __name__ == '__main__':
    main()
