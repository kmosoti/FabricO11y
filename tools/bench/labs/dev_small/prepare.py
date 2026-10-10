"""Small executable fixture and checker controls, run by the coordinator."""
import base64
import hashlib
import json
import os
from pathlib import Path
import py_compile
import subprocess
import sys

import archive
import audit
import measurement
import native


def main():
    tmp = Path(os.environ['TMPDIR'])
    for source in Path(__file__).parent.glob('*.py'):
        py_compile.compile(str(source),cfile=str(tmp/(source.stem+'.pyc')),doraise=True)
    checks = {'native':native.controls(),'measurement':measurement.controls(),
              'archive':archive.controls(tmp),'audit':audit.controls()}
    bins = Path(os.environ['CARGO_TARGET_DIR'])/'release'
    summaries = []
    for i,condition in enumerate(('H0','H256','nearrotation')):
        root = tmp/condition
        root.mkdir()
        native.make_certs(root)
        (root/'admin').write_text('private-fixture-control\n')
        cfg = root/'server.conf'
        cfg.write_text(f'listen=127.0.0.1:7443\ntls_cert={root}/server.pem\ntls_key={root}/server.key\nadmin_token_file={root}/admin\nstate_dir={root}/state\njournal_bytes=1073741824\njournal_file_bytes=67108864\nretention_s=86400\nretention_bytes=1073741824\nseal_workers=1\n')
        ledger = root/'seed-ledger.jsonl'
        command = [str(bins/'examples/lab_history_seed'),str(cfg),condition,str(ledger)]
        # Use full H256 and nearrotation shapes here to check actual publication,
        # frame overhead and disk footprints before native timing.
        output = subprocess.check_output(command,text=True)
        summary = json.loads(output)
        expected = {}
        for line in ledger.read_text().splitlines():
            row = json.loads(line)
            expected[(row['label'],row['sequence'])] = row
        recovered = {}
        source_rows = 0
        with (root/'replay.jsonl').open('wb') as out:
            subprocess.run([str(bins/'examples/server_dump'),str(cfg),'--records'],stdout=out,check=True)
        with (root/'replay.jsonl').open() as replay:
            for line in replay:
                row=json.loads(line)
                raw=base64.b64decode(row['bytes'])
                batch=native.query_oracle.decode_batch(raw)
                key=(row['label'],batch['sequence'])
                if key in recovered or key not in expected:
                    raise ValueError('unexpected/duplicate seed replay')
                if hashlib.sha256(raw).hexdigest()!=expected[key]['sha256']:
                    raise ValueError('seed custody bytes differ')
                bodies=native.query_oracle.decode_logs_request(batch['logs_bytes'])
                actual=[[r['body'].split(' ')[0],hashlib.sha256(r['body'].encode()).hexdigest()] for r in bodies]
                if actual!=expected[key]['source']:
                    raise ValueError('seed body ledger differs from independent decode')
                source_rows += len(actual)
                recovered[key]=True
        if set(recovered)!=set(expected) or source_rows!=summary['rows']:
            raise ValueError('seed census differs')
        active=(root/'state/journal/batches.faj').stat().st_size
        if condition=='H256' and active!=0:
            raise ValueError('published history left active bytes')
        if condition=='nearrotation' and not 64*2**20-256*2**10<=active<64*2**20:
            raise ValueError('nearrotation crossed file boundary')
        summaries.append({'condition':condition,'rows':source_rows,'batches':len(expected),
            'encoded_bytes':summary['encoded_bytes'],'active_bytes':active,
            'prefix_sha256':summary['prefix_sha256'],'independent_decode_exact':True})
    print(json.dumps({'controls':checks,'fixtures':summaries},indent=2),flush=True)


if __name__=='__main__':
    main()
