"""Lossless source-ledger columns with shared identity data; root owns execution."""
import gzip
import hashlib
import itertools
import json
from pathlib import Path
import shutil


def sha(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def store_gzip(raw, corpus):
    digest = sha(raw)
    destination = corpus / (digest + '.jsonl.gz')
    if not destination.exists():
        with raw.open('rb') as source, gzip.open(destination, 'wb', compresslevel=9) as out:
            shutil.copyfileobj(source, out)
    return destination


def restore_sources(record, destination):
    meta = json.loads((record/'source-layout.json').read_text())
    with gzip.open(record/meta['identity'], 'rt') as ids, gzip.open(record/'source-times.jsonl.gz', 'rt') as times, destination.open('wb') as out:
        count = 0
        for a, b in itertools.zip_longest(ids, times):
            if a is None or b is None:
                raise ValueError('source columns have different lengths')
            row = json.loads(a) + json.loads(b)
            out.write((json.dumps(row)+'\n').encode())
            count += 1
    if count != meta['rows'] or sha(destination) != meta['raw_sha256']:
        raise ValueError('source archive reconstruction differs')


def preserve_sources(source, destination, corpus, temporary):
    identity = temporary/'source-identity.jsonl'
    timing = destination/'source-times.jsonl.gz'
    original = hashlib.sha256()
    count = 0
    with gzip.open(source, 'rb') as src, identity.open('w') as ids, gzip.open(timing, 'wt', compresslevel=9) as times:
        for line in src:
            row = json.loads(line)
            if len(row) != 5:
                raise ValueError('unexpected source ledger schema')
            original.update(line)
            ids.write(json.dumps(row[:2])+'\n')
            times.write(json.dumps(row[2:])+'\n')
            count += 1
    shared = store_gzip(identity, corpus)
    relative = '../corpus/' + shared.name
    (destination/'source-layout.json').write_text(json.dumps({
        'encoding':'identity-and-time-columns-v1', 'identity':relative,
        'rows':count, 'raw_sha256':original.hexdigest(),
        'identity_raw_sha256':sha(identity)}, indent=2)+'\n')
    reconstructed = temporary/'source-reconstructed.jsonl'
    restore_sources(destination, reconstructed)
    reconstructed.unlink()
    identity.unlink()


def body_hashes(record):
    meta = json.loads((record/'source-layout.json').read_text())
    with gzip.open(record/meta['identity'], 'rt') as stream:
        hashes = dict(json.loads(line) for line in stream)
    seed = json.loads((record/'seed-layout.json').read_text())
    with gzip.open(record/seed['ledger'], 'rt') as stream:
        for line in stream:
            for tag, digest in json.loads(line)['source']:
                if tag in hashes:
                    raise ValueError('historical and live source names overlap')
                hashes[tag] = digest
    return hashes


def restore_clocks(record, destination):
    meta = json.loads((record/'clock-layout.json').read_text())
    hashes = body_hashes(record)
    count = 0
    with gzip.open(record/'data-clock-fields.jsonl.gz','rt') as src, destination.open('wb') as out:
        for line in src:
            row = json.loads(line)
            out.write((json.dumps(row+[hashes[row[0]]])+'\n').encode())
            count += 1
    if count != meta['rows'] or sha(destination) != meta['raw_sha256']:
        raise ValueError('clock archive reconstruction differs')


def preserve_clocks(source, destination, temporary):
    hashes = body_hashes(destination)
    original = hashlib.sha256()
    count = 0
    with gzip.open(source,'rb') as src,gzip.open(destination/'data-clock-fields.jsonl.gz','wt',compresslevel=9) as out:
        for line in src:
            row = json.loads(line)
            if len(row)!=8 or hashes.get(row[0])!=row[7]:
                raise ValueError('clock body differs from source; retain raw failure')
            original.update(line)
            out.write(json.dumps(row[:7])+'\n')
            count += 1
    (destination/'clock-layout.json').write_text(json.dumps({
        'encoding':'verified-body-hash-reference-v1','rows':count,
        'raw_sha256':original.hexdigest()},indent=2)+'\n')
    rebuilt = temporary/'clocks-reconstructed.jsonl'
    restore_clocks(destination,rebuilt)
    rebuilt.unlink()


def controls(temporary):
    lab = temporary/'archive-control'
    record = lab/'cell'
    corpus = lab/'corpus'
    record.mkdir(parents=True)
    corpus.mkdir()
    source = lab/'sources.jsonl.gz'
    with gzip.open(source,'wt') as out:
        for row in [['a','hash-a',123,'normal',0.5], ['b','hash-b',456,'burst',1.25]]:
            out.write(json.dumps(row)+'\n')
    preserve_sources(source,record,corpus,lab)
    seedraw = lab/'seed.jsonl'
    seedraw.write_text('')
    seed = store_gzip(seedraw,corpus)
    (record/'seed-layout.json').write_text(json.dumps({'ledger':'../corpus/'+seed.name}))
    clocks = lab/'clocks.gz'
    with gzip.open(clocks,'wt') as out:
        out.write(json.dumps(['a','node00',1,123,124,125,126,'hash-a'])+'\n')
    preserve_clocks(clocks,record,lab)
    with gzip.open(clocks,'wt') as out:
        out.write(json.dumps(['a','node00',1,123,124,125,126,'changed'])+'\n')
    try:
        preserve_clocks(clocks,record,lab)
    except ValueError:
        pass
    else:
        raise RuntimeError('changed body reference not rejected')
    meta = json.loads((record/'source-layout.json').read_text())
    meta['raw_sha256'] = '0'*64
    (record/'source-layout.json').write_text(json.dumps(meta))
    rejected = False
    try:
        restore_sources(record,lab/'altered.jsonl')
    except ValueError:
        rejected = True
    if not rejected:
        raise RuntimeError('changed-source hash not rejected')
    shutil.rmtree(lab)
    return {'lossless_source_clock_roundtrip':True, 'changed_archive_rejected':True,
            'changed_body_reference_rejected':True}
