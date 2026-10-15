"""Finite exact-package legacy transition; caller owns every process and cleanup."""
import hashlib
import json
import shutil
import ssl
import subprocess
import urllib.request
from migration_contract import CANARY, grade, require


def manifest(root):
    result = {}
    for path in sorted(root.rglob('*')):
        require(not path.is_symlink(), 'migration state symlink')
        if path.is_file():
            info = path.stat()
            require(info.st_nlink == 1, 'migration aliased state')
            result[str(path.relative_to(root))] = dict(bytes=info.st_size,
                sha256=hashlib.sha256(path.read_bytes()).hexdigest(), device=info.st_dev, inode=info.st_ino)
    return result


def prepare(args, work, out, origin, config, admin, launch, stop, wait, receipt):
    state = work/'state'
    require(not state.exists(), 'migration needs fresh original state')
    record = receipt['migration'] = {'commands': [], 'source': CANARY}
    query = dict(kind='logs', node='migration-edge', from_ns=0, to_ns=2**64-1, contains=CANARY, limit=2)
    admin_config = work/'migration-admin.conf'
    admin_config.write_text(f'server_url={origin}\nserver_ca={work}/ca.pem\nadmin_token_file={admin}\n')
    admin_config.chmod(0o600)
    legacy_config = work/'migration-legacy.conf'
    legacy_config.write_text('\n'.join(line for line in config.read_text().splitlines()
        if not line.startswith(('access_origin=', 'access_rp_id=', 'console_dir=')))+'\n')
    context = ssl.create_default_context(cafile=str(work/'ca.pem'))
    def healthy():
        if server.poll() is not None: raise RuntimeError('migration legacy server exited')
        try:
            with urllib.request.urlopen(origin+'/v1/health', context=context, timeout=1) as response:
                return response.status == 200
        except OSError: return False
    def cli(mode, cfg, *words, secret=False):
        command = [str(args.fabricctl), mode, str(cfg), *words]
        result = subprocess.run(command, capture_output=True, text=True, timeout=10)
        record['commands'].append({'argv': command, 'exit': result.returncode})
        require(result.returncode == 0, 'migration packaged CLI failed')
        value = json.loads(result.stdout)
        if not secret: record.setdefault('cli_answers', []).append(value)
        return value
    server = launch('migration-legacy-server', [str(args.server), 'serve-legacy', str(legacy_config)])
    wait(healthy, 'legacy native TLS health')
    source = work/'migration-source.log'; source.write_text(CANARY+'\n')
    enrolled = cli('admin', admin_config, 'node', 'add', 'migration-edge', '--log', str(source), '--interval', '60', secret=True)
    token = work/'migration-edge-token'; token.write_text(enrolled['token']+'\n'); token.chmod(0o600); enrolled = None
    node_config = work/'migration-edge.conf'
    node_config.write_text(f'spool_dir={work}/migration-spool\nlog={source}\nmetric_interval_s=60\nspool_bytes=33554432\nserver_url={origin}\nserver_ca={work}/ca.pem\ntoken_file={token}\n')
    launch('migration-legacy-edge', [str(args.spindle), 'run', str(node_config)])
    def observed():
        answer = cli('admin', admin_config, 'query', json.dumps(query))
        if answer.get('rows'):
            record['legacy'] = answer
            return True
        return False
    wait(observed, 'independent legacy canary committed')
    require(len(record['legacy']['rows']) == 1 and record['legacy']['rows'][0]['body'] == CANARY
        and record['legacy']['next_page'] is None, 'legacy complete literal canary')
    stop('migration-legacy')
    record['legacy_stopped'] = True
    record['before'] = manifest(state)
    info = state.stat(); record['state_identity'] = [info.st_dev, info.st_ino]
    raw = out/'migration-original'; raw.mkdir(mode=0o700)
    selected = {name: row for name,row in record['before'].items()
        if name == 'control.json' or name.startswith(('journal/', 'segments/'))}
    require('control.json' in selected and any(n.startswith(('journal/', 'segments/')) for n in selected),
        'original control and telemetry evidence missing')
    for name in selected:
        target = raw/name; target.parent.mkdir(parents=True,exist_ok=True); shutil.copyfile(state/name,target)
    record['original'] = selected
    record['stopped_unchanged'] = manifest(state) == record['before']
    require(record['stopped_unchanged'], 'migration stopped state changed before ordinary serve')
    return record, query, cli


def finish(args, work, out, config, admin, prepared, api, bearer, launch, stop, wait):
    record, query, cli = prepared
    state = work/'state'; info = state.stat()
    require([info.st_dev, info.st_ino] == record['state_identity'], 'migration substituted state directory')
    status, answer = api('/v1/console/query','POST',query)
    require(status == 200, 'migration owner query unavailable'); record['owner'] = answer
    status, answer = bearer(admin.read_text().strip(), '/v1/console/query', 'POST', query)
    record['master'] = (status, answer.get('error') if isinstance(answer,dict) else None)
    record['admin_status'] = bearer(admin.read_text().strip(), '/v1/admin/nodes')[0]
    status, reader = api('/v1/console/readers','POST',{'name':'Migration finite reader','ttl_s':600})
    require(status == 200, 'migration reader issuance failed')
    credential = reader['token']
    status, answer = bearer(credential, '/v1/console/query','POST',query)
    require(status == 200, 'migration reader query unavailable');record['reader'] = answer
    protected = work/'migration-reader';protected.write_text(credential+'\n');protected.chmod(0o600);credential=reader=None
    cfg = work/'migration-access.conf'
    cfg.write_text(f'server_url={args._migration_origin}\nserver_ca={work}/ca.pem\nreader_token_file={protected}\n');cfg.chmod(0o600)
    record['cli'] = cli('access',cfg,'query',json.dumps(query))
    record['prefixes_preserved'] = all((state/name).is_file()
        and hashlib.sha256((state/name).read_bytes()[:row['bytes']]).hexdigest() == row['sha256']
        for name,row in record['original'].items())
    require(record['prefixes_preserved'], 'migration original control/telemetry prefix changed')
    # Separate stopped development-format fixture; never edit admitted live state.
    fresh = work/'migration-format1'; (fresh/'access').mkdir(parents=True,mode=0o700)
    (fresh/'access/access.json').write_text('{"version":1,"principals":{}}')
    (fresh/'telemetry-sentinel').write_text(CANARY+'\n')
    before = manifest(fresh)
    cfg = work/'migration-format1.conf';cfg.write_text(config.read_text().replace(f'state_dir={state}',f'state_dir={fresh}'))
    proc = launch('migration-format1', [str(args.server),'serve',str(cfg)])
    wait(lambda:proc.poll() is not None, 'ordinary production format1 startup refusal')
    record['format_exit'] = proc.returncode
    stop('migration-format1')
    record['format_stderr'] = (out/'migration-format1.log').read_text()
    record['format_unchanged'] = all((fresh/name).is_file()
        and hashlib.sha256((fresh/name).read_bytes()).hexdigest()==row['sha256'] for name,row in before.items())
    grade(record)
    return record
