#!/usr/bin/env python3
"""One bounded RCA journal cell; no interpretation or qualification claim.

Must run through resource_group.py after protocol registration. Existing fixture
bytes and the independent query oracle remain unchanged. Controller-only inputs
are never exposed as investigator evidence.
"""
import argparse
import base64
import copy
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import ssl
import subprocess
import sys
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / 'tools'))
sys.path.insert(0, str(ROOT / 'tools/qualification'))
sys.path.insert(0, str(ROOT / 'tools/bench/labs/completion'))
import resource_group
import query_oracle
import cgroups
from delivery_faults import free_port

DATA = ROOT / 'docs/experiments/benchmarks/data/hammer-reference-01'
BUILD = DATA / 'memory/identity-build-01/build/build.json'
PACKET = DATA / 'query/rca-preparation-01/packet'
FIXTURE_MAX = 8 * 2**20
EVIDENCE_MAX = 32 * 2**20


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def dump(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2) + '\n')


def footprint(root):
    return sum(p.stat().st_size for p in root.rglob('*') if p.is_file())


def frozen(bins, manifest):
    build = json.loads(manifest.read_text())
    sources = {k: v for k, v in build['source_sha256'].items()
               if k.endswith('.rs') or Path(k).name in ('Cargo.toml', 'Cargo.lock')}
    actual = ({p for directory in (ROOT/'src', ROOT/'crates', ROOT/'examples')
               for p in directory.rglob('*.rs')} |
              {ROOT/'Cargo.toml', ROOT/'Cargo.lock'} | set((ROOT/'crates').glob('*/Cargo.toml')))
    if {str(p.relative_to(ROOT)) for p in actual} != set(sources):
        raise RuntimeError('retained Rust/manifest source set differs')
    if any(digest(ROOT/name) != value for name, value in sources.items()):
        raise RuntimeError('retained Rust/manifest source content differs')
    hashes = {name: digest(bins/name) for name in ('fabric-server', 'examples/server_dump')}
    if any(build['binaries'][name] != value for name, value in hashes.items()):
        raise RuntimeError('frozen binary content differs')
    return dict(source_sha256=sources, binary_sha256=hashes, build_sha256=digest(manifest))


class Pilot:
    def __init__(self, work, bins, group, cpus, started, plan='walk'):
        self.work, self.bins, self.group, self.cpus = work, bins, group, cpus
        self.deadline = started + 160
        self.port, self.admin = free_port(), os.urandom(32).hex()
        self.server = None
        self.samples, self.requests, self.acks, self.chains = [], [], [], []
        self.counts, self.tokens = {}, {}
        self.phase = 'setup'
        self.response_bytes = 0
        self.http_attempts = 0
        self.conf = work/'server.conf'
        self.generation = 0
        self.exits = []
        self.plan = plan

    def remaining(self, cap=10, reserve=20):
        result = min(cap, self.deadline-time.monotonic()-reserve)
        if result <= 0:
            raise TimeoutError('pilot absolute deadline cleanup reserve reached')
        return result

    def sample(self):
        row = dict(mono_ns=time.monotonic_ns(), wall_ns=time.time_ns(),
                   disk_bytes=footprint(self.work), phase=self.phase,
                   generation=self.generation)
        if row['disk_bytes'] > FIXTURE_MAX:
            raise RuntimeError('8 MiB live fixture ceiling reached')
        row['cgroup'] = {k: (self.group/k).read_text().strip() for k in
                         ('memory.current', 'memory.peak', 'memory.events',
                          'memory.swap.current', 'cpu.stat', 'io.stat', 'pids.current')}
        if self.server and self.server.poll() is None:
            stat = Path(f'/proc/{self.server.pid}/stat').read_text().rpartition(') ')[2].split()
            status = dict(line.split(':', 1) for line in Path(f'/proc/{self.server.pid}/status').read_text().splitlines() if ':' in line)
            row['server'] = dict(cpu_s=(int(stat[11])+int(stat[12]))/os.sysconf('SC_CLK_TCK'),
                                 rss_kib=int(status['VmRSS'].split()[0]),
                                 hwm_kib=int(status['VmHWM'].split()[0]))
        self.samples.append(row)

    def setup(self):
        # Every subprocess gets an absolute-deadline-derived timeout.
        commands = [
            ['req','-x509','-newkey','ec','-pkeyopt','ec_paramgen_curve:P-256','-nodes','-keyout','ca.key','-out','ca.pem','-days','2','-subj','/CN=fabric test CA','-addext','basicConstraints=critical,CA:TRUE','-addext','keyUsage=critical,keyCertSign,cRLSign'],
            ['req','-newkey','ec','-pkeyopt','ec_paramgen_curve:P-256','-nodes','-keyout','server.key','-out','server.csr','-subj','/CN=127.0.0.1'],
            ['x509','-req','-in','server.csr','-CA','ca.pem','-CAkey','ca.key','-CAcreateserial','-out','server.pem','-days','2','-extfile','san.ext']]
        (self.work/'san.ext').write_text('subjectAltName=IP:127.0.0.1\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\n')
        for command in commands:
            subprocess.run(['openssl', *command], cwd=self.work, check=True,
                           capture_output=True, timeout=self.remaining(8))
        (self.work/'admin-token').write_text(self.admin+'\n')
        self.conf.write_text(f'listen=127.0.0.1:{self.port}\ntls_cert={self.work}/server.pem\ntls_key={self.work}/server.key\nstate_dir={self.work}/state\nadmin_token_file={self.work}/admin-token\njournal_bytes=8388608\njournal_file_bytes=1048576\nretention_s=86400\nretention_bytes=8388608\nseal_workers=1\nquery_plan={self.plan}\n')
        self.ctx = ssl.create_default_context(cafile=str(self.work/'ca.pem'))

    def http(self, endpoint, body=None, token=None, raw=False):
        self.remaining()
        self.http_attempts += 1
        if self.http_attempts > 256:
            raise RuntimeError('256 total HTTP request ceiling reached')
        query = endpoint == '/v1/admin/query'
        if query:
            count = self.counts.get(self.phase, 0)+1
            if count > 32:
                raise RuntimeError('32 HTTP query requests per phase exceeded')
            self.counts[self.phase] = count
        data = body if raw else json.dumps(body).encode() if body is not None else None
        request = urllib.request.Request(f'https://127.0.0.1:{self.port}'+endpoint,
            data=data, headers={'authorization':'Bearer '+(token or self.admin),
                               'content-type':'application/x-protobuf' if raw else 'application/json'})
        begin = time.monotonic_ns()
        event = dict(phase=self.phase, endpoint=endpoint, start_mono_ns=begin,
                     query=body if query else None)
        try:
            with urllib.request.urlopen(request, context=self.ctx, timeout=self.remaining(5)) as response:
                encoded = response.read(1024*1024+1)
                if len(encoded) > 1024*1024:
                    raise RuntimeError('HTTP response exceeded 1 MiB')
                self.response_bytes += len(encoded)
                event['response_bytes'] = len(encoded)
                if self.response_bytes > 8*2**20:
                    raise RuntimeError('8 MiB cumulative HTTP response ceiling reached')
                answer = json.loads(encoded)
            if query:
                event['answer'] = answer
            return answer
        except BaseException as exc:
            event['error'] = repr(exc)
            raise
        finally:
            ended = time.monotonic_ns()
            event.update(end_mono_ns=ended, end_wall_ns=time.time_ns(), elapsed_ms=(ended-begin)/1e6)
            self.requests.append(event)
            self.sample()

    def start(self):
        self.generation += 1
        with (self.work/f'server-{self.generation}.out').open('wb') as out, (self.work/f'server-{self.generation}.err').open('wb') as err:
            self.server = subprocess.Popen([sys.executable, str(Path(__file__).resolve()), '--enter', str(self.group),
                'taskset', '-c', ','.join(map(str, self.cpus)), str(self.bins/'fabric-server'), 'serve', str(self.conf)], stdout=out, stderr=err)
        until = time.monotonic()+self.remaining(12)
        while time.monotonic() < until:
            if self.server.poll() is not None:
                raise RuntimeError('server exited on startup')
            try:
                self.http('/v1/admin/nodes')
                return
            except OSError:
                time.sleep(.05)
        raise TimeoutError('TLS startup deadline')

    def stop(self):
        if self.server:
            if self.server.poll() is not None:
                raise RuntimeError('server exited before graceful stop')
            self.sample()
            self.server.send_signal(signal.SIGTERM)
            code = self.server.wait(timeout=self.remaining(10, 10))
            self.exits.append(dict(generation=self.generation, returncode=code, graceful=True))
            if code != 0:
                raise RuntimeError('nonzero graceful server exit')

    def send(self, packet, replay=False):
        raw = base64.b64decode(packet['bytes'], validate=True)
        begin, wall = time.monotonic_ns(), time.time_ns()
        answer = self.http('/v1/batches', raw, self.tokens[packet['label']], True)
        end = self.requests[-1]['end_mono_ns']
        self.acks.append(dict(label=packet['label'], sequence=packet['sequence'],
            sha256=packet['sha256'], replay=replay, source_inject_mono_ns=begin,
            source_inject_wall_ns=wall, ack_mono_ns=end, ack_wall_ns=self.requests[-1]['end_wall_ns'],
            source_inject_to_ack_ms=(end-begin)/1e6, answer=answer))
        if answer.get('status') != 'ack' or answer.get('committed_through') != packet['sequence']:
            raise RuntimeError(f'unexpected batch ACK: {answer}')

    def recovery(self, name):
        with (self.work/name).open('wb') as recovered, (self.work/(name+'.err')).open('wb') as err:
            subprocess.run([str(self.bins/'examples/server_dump'), str(self.conf), '--records'],
                           check=True, stdout=recovered, stderr=err, timeout=self.remaining(12))
        self.sample()
        return [json.loads(line) for line in (self.work/name).read_text().splitlines()]

    def chain(self, recipe, pages=None, held=False):
        query = recipe['query']
        pages = copy.deepcopy(pages) if pages else [self.http('/v1/admin/query', query)]
        while pages[-1]['next_page'] is not None:
            pages.append(self.http('/v1/admin/query', dict(query, page=pages[-1]['next_page'])))
        result = dict(phase='initial' if held else self.phase, executed_phase=self.phase,
                      key='held_trace' if held else recipe['key'], query=query, pages=pages)
        self.chains.append(result)
        return result


def run(args):
    resource_group.require_limits()
    started = time.monotonic()
    scratch = Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve(strict=True)
    scratch.relative_to((resource_group.STORAGE/'scratch').resolve(strict=True))
    out = args.out.resolve()
    out.relative_to((DATA/'query').resolve(strict=True))
    if out.exists() or out == (DATA/'query').resolve():
        raise RuntimeError('fresh evidence child required')
    if not args.id.replace('-', '').replace('_', '').isalnum():
        raise ValueError('invalid scratch identifier')
    work = scratch/('rca-native-'+args.id)
    if work.exists():
        raise RuntimeError('fresh owned scratch required')
    packet = args.packet.resolve(strict=True)
    if footprint(packet) > FIXTURE_MAX:
        raise RuntimeError('input packet exceeds 8 MiB')
    manifest = json.loads((packet/'controller/manifest.json').read_text())
    for name, expected in manifest['files'].items():
        member = (packet/name).resolve(strict=True)
        member.relative_to(packet)
        if digest(member) != expected:
            raise RuntimeError('prepared packet hash mismatch')
    if manifest['sources']['tools/qualification/query_oracle.py'] != digest(Path(query_oracle.__file__)):
        raise RuntimeError('independent oracle differs from preparation binding')
    producer = [json.loads(line) for line in (packet/'controller/producer.jsonl').read_text().splitlines()]
    producer = [p for p in producer if p['case'] == args.case]
    initial = [p for p in producer if p['stage'] == 'initial']
    late = [p for p in producer if p['stage'] == 'late']
    recipes = next(p for p in json.loads((packet/'investigator/playbooks.json').read_text()) if p['case']==args.case)['queries']
    expected_late = 1 if args.case=='rca-05' else 0
    initial_spans = 2 if args.case in ('rca-04', 'rca-05') else 3
    fresh_spans = 2 if args.case=='rca-04' else 3
    delivery_phase = 'after_late' if expected_late else 'after_delivery'
    if len(initial)!=2 or len(late)!=expected_late or len(recipes)!=8:
        raise RuntimeError(f'frozen {args.case} producer/playbook shape differs')
    for p in producer:
        raw = base64.b64decode(p['bytes'], validate=True)
        b = query_oracle.decode_batch(raw)
        if hashlib.sha256(raw).hexdigest()!=p['sha256'] or b['sequence']!=p['sequence'] or b['generation']!=p['generation'] or b['node_id'].hex()!=p['node_id']:
            raise RuntimeError('producer bytes/identity mismatch')
    bins = args.bin_dir.resolve(strict=True)
    identity = frozen(bins, args.build_manifest)
    cpus = sorted(os.sched_getaffinity(0))
    if len(cpus)<2:
        raise RuntimeError('two affinity CPUs required')
    parent = cgroups.delegate()
    page = os.sysconf('SC_PAGE_SIZE')
    maximum = 4_000_000_000//page*page
    group, limits = cgroups.subgroup(parent, 'rca-server-'+args.id, maximum, 3_000_000_000//page*page, 256, 2)
    work.mkdir(mode=0o700)
    out.mkdir(mode=0o700, parents=True)
    pilot = Pilot(work, bins, group, cpus[:2], started, args.plan)
    status, verdicts, controls = 'failed', [], []
    helper_sources = (Path(__file__), Path(query_oracle.__file__), Path(cgroups.__file__),
                      Path(resource_group.__file__), ROOT/'tools/qualification/delivery_faults.py')
    dump(out/'environment.json', dict(command=sys.argv, **identity, limits=limits, cpus=cpus[:2],
        parent=str(parent), scratch=str(work), internal_deadline_s=160, cleanup_reserve_s=20,
        fixture_ceiling_bytes=FIXTURE_MAX, evidence_ceiling_bytes=EVIDENCE_MAX,
        packet_manifest_sha256=digest(packet/'controller/manifest.json'), plan=args.plan, case=args.case,
        late_batches=expected_late, initial_span_count=initial_spans, fresh_span_count=fresh_spans,
        helper_sha256={str(p.relative_to(ROOT)):digest(p) for p in helper_sources},
        boundary='journal only; no seal/publication, source clock or interpretation claim', uname=list(os.uname())))
    try:
        pilot.setup(); pilot.start()
        for p in initial:
            pilot.tokens[p['label']] = pilot.http('/v1/admin/nodes', dict(name=p['label']))['token']
            pilot.send(p)
        pilot.send(initial[0], replay=True)
        pilot.phase = 'initial'
        for recipe in recipes:
            pilot.chain(recipe)
        trace = next(r for r in recipes if r['key']=='trace')
        held = [pilot.http('/v1/admin/query', trace['query'])]
        if held[0]['next_page'] is None:
            raise RuntimeError('held trace needs another page')
        pilot.phase = delivery_phase
        for packet_to_send in late:
            pilot.send(packet_to_send)
        old = pilot.chain(trace, held, held=True)
        for recipe in recipes:
            pilot.chain(recipe)
        if sum(len(p['rows']) for p in old['pages']) != initial_spans:
            raise RuntimeError(f'old snapshot span count differs from {initial_spans}')
        fresh = next(c for c in pilot.chains if c['phase']==delivery_phase and c['key']=='trace')
        if sum(len(p['rows']) for p in fresh['pages']) != fresh_spans:
            raise RuntimeError(f'fresh snapshot span count differs from {fresh_spans}')
        pilot.stop()
        before_restart = pilot.recovery('before-restart.jsonl')
        pilot.phase = 'after_restart'; pilot.start()
        for recipe in recipes:
            pilot.chain(recipe)
        pilot.stop()
        records = pilot.recovery('recovered.jsonl')
        if sorted(json.dumps(r, sort_keys=True) for r in records) != sorted(json.dumps(r, sort_keys=True) for r in before_restart):
            raise RuntimeError('durable recovery bytes/receive observations changed across restart')
        observed = {}
        for r in records:
            raw = base64.b64decode(r['bytes'], validate=True)
            b = query_oracle.decode_batch(raw)
            key = (r['label'], b['node_id'].hex(), b['generation'], b['sequence'])
            if key in observed or not isinstance(r['received_ns'], int) or r['received_ns']<=0:
                raise RuntimeError('duplicate recovery or invalid measured receive timestamp')
            observed[key] = hashlib.sha256(raw).hexdigest()
        expected = {(p['label'],p['node_id'],p['generation'],p['sequence']):p['sha256'] for p in producer}
        if observed != expected:
            raise RuntimeError('producer-to-recovery exact identity/byte mismatch')
        initial_records = [r for r in records if query_oracle.decode_batch(base64.b64decode(r['bytes']))['sequence']==1]
        for chain in pilot.chains:
            source = initial_records if chain['phase']=='initial' else records
            verdict = query_oracle.check(source, chain['query'], chain['pages'])
            verdicts.append(dict(phase=chain['phase'], key=chain['key'], verdict=verdict))
            if not verdict['passed']:
                raise RuntimeError('independent exact query rejection: '+repr(verdicts[-1]))
        cases = [('omitted_matching_row', next(c for c in pilot.chains if c['phase']=='initial' and c['key']=='logs'), 'ROW-DROPPED'),
                 ('altered_metric_value', next(c for c in pilot.chains if c['phase']=='initial' and c['key']=='pool_use'), 'ROW-CONTENT')]
        if expected_late:
            cases.append(('late_span_in_held_old_chain', old, 'ROW-DUPLICATED'))
        for defect, chain, rule in cases:
            changed = copy.deepcopy(chain['pages'])
            if defect=='omitted_matching_row':
                changed[0]['rows'].pop(0)
            elif defect=='altered_metric_value':
                changed[0]['rows'][0]['value'] += 1
            else:
                late_row = next(r for p in fresh['pages'] for r in p['rows'] if r['sequence']==2)
                changed[-1]['rows'].append(copy.deepcopy(late_row))
            verdict = query_oracle.check(initial_records, chain['query'], changed)
            # Unexpected identity is also reported by ROW-DUPLICATED by this
            # unchanged oracle; record the actual rule rather than a new oracle.
            controls.append(dict(defect=defect, query=chain['query'], pages=changed, verdict=verdict))
            if verdict['passed'] or rule not in {v['rule'] for v in verdict['violations']}:
                raise RuntimeError('negative control not semantically rejected')
        if frozen(bins, args.build_manifest) != identity:
            raise RuntimeError('frozen source/binary binding changed during pilot')
        events = dict(line.split() for line in (group/'memory.events').read_text().splitlines())
        if int(events['oom']) or int(events['oom_kill']) or (group/'memory.swap.current').read_text().strip()!='0':
            raise RuntimeError('child resource invariant violated')
        if int((group/'memory.peak').read_text())>maximum or (group/'memory.max').read_text().strip()!=str(maximum):
            raise RuntimeError('server 4 GB page-floor memory ceiling violated')
        if list((work/'state/segments').glob('seg-*')) or list((work/'state/journal').glob('sealed-*.faj')):
            raise RuntimeError('journal-only pilot unexpectedly published/sealed')
        status = 'passed'
    except BaseException as exc:
        dump(out/'failure.json', dict(error=repr(exc)))
    finally:
        if pilot.server and pilot.server.poll() is None:
            pilot.server.kill(); pilot.server.wait(timeout=5)
            pilot.exits.append(dict(generation=pilot.generation, returncode=pilot.server.returncode, graceful=False))
        dump(out/'queries.json', pilot.chains)
        dump(out/'http-observations.json', pilot.requests)
        dump(out/'acks.json', pilot.acks)
        visibility = []
        for ack in pilot.acks:
            observed = []
            for request in pilot.requests:
                if request['start_mono_ns'] < ack['ack_mono_ns'] or 'answer' not in request:
                    continue
                rows = request['answer']['rows']
                if any(row.get('node')==ack['label'] and row.get('sequence')==ack['sequence'] for row in rows):
                    observed.append(request['end_mono_ns'])
            visibility.append(dict(label=ack['label'], sequence=ack['sequence'], replay=ack['replay'],
                ack_to_first_observed_query_upper_bound_ms=(min(observed)-ack['ack_mono_ns'])/1e6 if observed else None,
                boundary='first completed matching-row query after ACK; no exact visibility instant'))
        dump(out/'visibility.json', visibility)
        dump(out/'resources.json', pilot.samples)
        dump(out/'oracle-verdicts.json', verdicts)
        dump(out/'controls.json', controls)
        (out/'controller').mkdir(exist_ok=True)
        (out/'controller/producer.jsonl').write_text(''.join(json.dumps(p)+'\n' for p in producer))
        shutil.copyfile(args.build_manifest, out/'controller/retained-build.json')
        shutil.copyfile(packet/'controller/manifest.json', out/'controller/packet-manifest.json')
        dump(out/'playbook.json', dict(case=args.case, queries=recipes))
        for source in helper_sources:
            shutil.copyfile(source, out/'controller'/str(source.relative_to(ROOT)).replace('/', '__'))
        cleanup = dict(removed=False, original=str(work))
        try:
            # On failure preserve the whole tiny owned fixture, including state,
            # before cleanup. Credentials remain controller-only laboratory data.
            paths = [p for p in work.rglob('*') if p.is_file()] if status!='passed' else [p for p in work.iterdir() if p.is_file() and p.name not in ('admin-token','ca.key','server.key','server.conf')]
            if footprint(out)+sum(p.stat().st_size for p in paths)>EVIDENCE_MAX-1024*1024:
                raise RuntimeError('32 MiB evidence ceiling; retain owned scratch')
            for source in paths:
                target = out/'controller/recovery'/source.relative_to(work)
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(source, target)
                if digest(source)!=digest(target):
                    raise RuntimeError('evidence copy hash mismatch')
            shutil.rmtree(work)
            cleanup['removed'] = True
        except BaseException as exc:
            status = 'failed'; cleanup['error'] = repr(exc)
        dump(out/'cgroup-final.json', cgroups.snapshot(parent))
        if not (group/'cgroup.procs').read_text().strip():
            group.rmdir(); cleanup['server_subgroup_removed'] = True
        dump(out/'summary.json', dict(status=status, elapsed_s=time.monotonic()-started,
            query_http_requests=pilot.counts, checked_chains=len(verdicts), negative_controls=len(controls),
            total_http_requests=pilot.http_attempts, total_http_response_bytes=pilot.response_bytes,
            server_exits=pilot.exits,
            server_cpu_seconds_observed=sum(max((r.get('server',{}).get('cpu_s',0) for r in pilot.samples if r['generation']==generation), default=0) for generation in range(1,pilot.generation+1)),
            server_peak_hwm_kib=max((r.get('server',{}).get('hwm_kib',0) for r in pilot.samples), default=0),
            peak_sampled_fixture_bytes=max((r['disk_bytes'] for r in pilot.samples), default=0),
            recovered_batches=len(records) if 'records' in locals() else None,
            phase='journal', publication_exercised=False, interpretation_exercised=False,
            case=args.case, plan=args.plan, late_batches=expected_late,
            timing_boundary='source injection starts at HTTP send; synthetic telemetry clocks are not source latency clocks',
            limitations=[f'one synthetic case ({args.case}) and one plan ({args.plan}) per cell', 'no seal/publication', 'no blind investigator or causality scoring',
                         'resource samples at request/phase boundaries; cgroup peak is continuous']))
        dump(out/'cleanup.json', cleanup)
        dump(out/'manifest.json', {str(p.relative_to(out)):dict(bytes=p.stat().st_size,sha256=digest(p))
            for p in sorted(out.rglob('*')) if p.is_file() and p.name!='manifest.json'})
    return 0 if status=='passed' else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--id', default='pilot')
    parser.add_argument('--case', choices=[f'rca-{number:02}' for number in range(1,8)], default='rca-05')
    parser.add_argument('--plan', choices=['scan','walk'], default='walk')
    parser.add_argument('--packet', type=Path, default=PACKET)
    parser.add_argument('--build-manifest', type=Path, default=BUILD)
    parser.add_argument('--bin-dir', type=Path, default=Path('/run/media/kmosoti/data/FabricO11y/cargo/release'))
    return run(parser.parse_args())


if __name__=='__main__':
    if len(sys.argv)>2 and sys.argv[1]=='--enter':
        cgroups.enter(Path(sys.argv[2])); os.execvp(sys.argv[3], sys.argv[3:])
    raise SystemExit(main())
