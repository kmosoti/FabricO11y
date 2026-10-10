"""C5-only reversible hash-column compression; original JSON bytes stay exact."""
import base64
import gzip
import hashlib
import json
from pathlib import Path
import shutil

SEED = '2703163393'
MARKER = '"@"'
FILES = {'sources.jsonl.gz': 'sources', 'seed-ledger.jsonl': 'seed',
         'data-clocks.jsonl.gz': 'clocks'}

def body_hash(tag, seed=SEED):
    if tag.startswith('old-'):
        seq, index = map(int, tag[4:].split(':'))
        if tag != f'old-{seq:06}:{index:03}' or seq < 1 or index < 0:
            raise ValueError('invalid historical tag')
        text = tag+' '
        if index % 2 == 0:
            text += 'R'*(900-len(text))
        else:
            counter = 0
            while len(text) < 900:
                text += hashlib.sha256(f'{seed}:{seq}:{index}:{counter}'.encode()).hexdigest()
                counter += 1
        text = text[:900]
    else:
        node, tick, index = map(int, tag.split(':'))
        if tag != f'{node:02}:{tick:04}:{index:02}' or min(node,tick,index) < 0:
            raise ValueError('invalid live tag')
        padding = ('R'*900 if (tick+index)%2 == 0 else
                   base64.b85encode(hashlib.shake_256(f'{seed}:{tag}'.encode()).digest(720)).decode())
        text = ('load-'+tag+' '+padding)[:900]
    return hashlib.sha256(text.encode()).hexdigest()

def fields(kind, row):
    if kind == 'sources':
        if not isinstance(row,list) or len(row) != 5:
            raise ValueError('source schema')
        return [(row[0],row[1])]
    if kind == 'clocks':
        if not isinstance(row,list) or len(row) != 8:
            raise ValueError('clock schema')
        return [(row[0],row[7])]
    if kind == 'seed':
        if not isinstance(row,dict) or not row.get('source') or row.get('label') != 'oldhistory':
            raise ValueError('seed schema')
        raw_hash = row.get('sha256')
        if not isinstance(raw_hash,str) or len(raw_hash) != 64 or any(c not in '0123456789abcdef' for c in raw_hash):
            raise ValueError('missing actual Batch SHA')
        if any(not isinstance(v,list) or len(v)!=2 for v in row['source']):
            raise ValueError('seed source schema')
        return [(tag,digest) for tag,digest in row['source']]
    raise ValueError('unknown archive kind')

def transform(raw, kind, encode):
    text = raw.decode('utf-8')
    row = json.loads(text)
    pairs = fields(kind,row)
    for tag,digest in pairs:
        expected = body_hash(tag)
        if encode:
            if digest != expected:
                raise ValueError('body hash does not match deterministic producer')
            needle = json.dumps(expected)
            if text.count(needle) != 1:
                raise ValueError('ambiguous hash replacement')
            text = text.replace(needle,MARKER,1)
        else:
            if digest != '@' or MARKER not in text:
                raise ValueError('missing archive hash marker')
            # Hash fields occur in pair order; seed's actual Batch SHA stays intact.
            text = text.replace(MARKER,json.dumps(expected),1)
    return text.encode('utf-8'), [tag for tag,_ in pairs]

def opened(path):
    return gzip.open(path,'rb') if path.suffix == '.gz' else path.open('rb')

def digest(path, decoded=False):
    with (opened(path) if decoded else path.open('rb')) as stream:
        return hashlib.file_digest(stream,'sha256').hexdigest()

def restore(archive, kind, target, expected_sha256):
    hasher = hashlib.sha256()
    with gzip.open(archive,'rb') as source,target.open('wb') as output:
        for line in source:
            raw,_ = transform(line,kind,False)
            hasher.update(raw); output.write(raw)
    if hasher.hexdigest() != expected_sha256:
        raise ValueError('archive reconstruction differs: missing/order/timestamp/content')

def preserve(root, destination, tmp):
    """Persist exact originals via reversible markers; return receipt, then remove.

    All four inputs must be closed. No original is removed until all archives,
    readbacks, cross-file identity coverage and persistent receipts succeed.
    Call this before a generic copier, excluding these original input filenames.
    """
    root,destination,tmp = map(Path,(root,destination,tmp))
    destination.mkdir(parents=True,exist_ok=True);tmp.mkdir(parents=True,exist_ok=True)
    paths = [root/name for name in FILES]+[root/'recovered-hashes.jsonl.gz']
    if any(not p.is_file() for p in paths):
        raise ValueError('missing closed C5 archive input')
    receipt = {}; tags_by_kind = {}; seed_sequences = []
    for name,kind in FILES.items():
        source=root/name;archive=destination/(kind+'.c5.jsonl.gz')
        if archive.exists():raise ValueError('archive destination already exists')
        hasher=hashlib.sha256();count=0;tags=[]
        with opened(source) as inp,gzip.open(archive,'wb',compresslevel=9) as out:
            for line in inp:
                packed,line_tags=transform(line,kind,True)
                if kind=='seed':seed_sequences.append(json.loads(line)['sequence'])
                hasher.update(line);out.write(packed);count+=1;tags.extend(line_tags)
        if len(set(tags))!=len(tags):raise ValueError('duplicate producer/clock tag')
        tags_by_kind[kind]=set(tags)
        entry={'kind':kind,'archive':archive.name,'original_name':name,'original_sha256':hasher.hexdigest(),
               'original_file_sha256':digest(source),'archive_sha256':digest(archive),'lines':count,'tags':len(tags)}
        restored=tmp/(kind+'.c5-readback.jsonl')
        restore(archive,kind,restored,entry['original_sha256'])
        entry['original_decoded_bytes']=restored.stat().st_size
        restored.unlink();receipt[name]=entry
    if seed_sequences != list(range(1,len(seed_sequences)+1)):
        raise ValueError('missing/duplicate/reordered seed sequence')
    if tags_by_kind['seed'] & tags_by_kind['sources'] or tags_by_kind['clocks'] != tags_by_kind['seed'] | tags_by_kind['sources']:
        raise ValueError('seed/live/clock partition differs')
    source=root/'recovered-hashes.jsonl.gz';archive=destination/source.name
    if archive.exists():raise ValueError('recovered hash destination already exists')
    shutil.copyfile(source,archive)
    if digest(source)!=digest(archive) or digest(source,True)!=digest(archive,True):
        raise ValueError('actual Batch hash gzip readback differs')
    receipt[source.name]={'archive':archive.name,'file_sha256':digest(source),'decoded_sha256':digest(source,True),
                          'scope':'actual Batch SHA preserved unchanged; no regeneration'}
    receipt_path=destination/'c5-archive.json'
    if receipt_path.exists():raise ValueError('archive receipt already exists')
    receipt_path.write_text(json.dumps({'format':'c5-hash-markers-v1','seed':SEED,'files':receipt,'originals_removed':False},indent=2)+'\n')
    for path in paths:path.unlink()
    receipt_path.write_text(json.dumps({'format':'c5-hash-markers-v1','seed':SEED,'files':receipt,'originals_removed':True},indent=2)+'\n')
    return receipt

def controls(tmp):
    """Deterministic controls; root executes under the resource launcher."""
    tmp=Path(tmp);work=tmp/'c5-archive-controls';work.mkdir(exist_ok=False)
    live='00:0001:00';old='old-000001:001'
    source=(json.dumps([live,body_hash(live),10,'normal',0])+'\n').encode()
    packed,_=transform(source,'sources',True)
    if transform(packed,'sources',False)[0]!=source:raise RuntimeError('framing roundtrip')
    if body_hash(live,'wrong')==body_hash(live) or body_hash(old,'wrong')==body_hash(old):raise RuntimeError('wrong seed control ineffective')
    for wrong in ['0'*64,body_hash(live,'wrong')]:
        altered=json.loads(source);altered[1]=wrong
        try:transform((json.dumps(altered)+'\n').encode(),'sources',True)
        except ValueError:pass
        else:raise RuntimeError('altered hash/wrong seed accepted')
    seed_raw=(json.dumps({'label':'oldhistory','sequence':1,'sha256':'a'*64,'bytes':100,'source':[[old,body_hash(old)]]},separators=(',',':'))+'\n').encode()
    clock_raw=(json.dumps([old,'oldhistory',1,None,3,4,None,body_hash(old)])+'\n').encode()
    for kind,raw in [('seed',seed_raw),('clocks',clock_raw)]:
        packed_other,_=transform(raw,kind,True)
        if transform(packed_other,kind,False)[0]!=raw:raise RuntimeError('seed/clock framing')
    archive=work/'control.gz';target=work/'readback';expected=hashlib.sha256(source).hexdigest()
    for defect,raw in [('missing',b''),('duplicate',packed+packed),('timestamp',packed.replace(b'10',b'11',1))]:
        with gzip.open(archive,'wb') as stream:stream.write(raw)
        try:restore(archive,'sources',target,expected)
        except ValueError:pass
        else:raise RuntimeError('archive defect accepted: '+defect)
    other=(json.dumps(['00:0002:00',body_hash('00:0002:00'),11,'normal',0])+'\n').encode()
    packed_other,_=transform(other,'sources',True)
    with gzip.open(archive,'wb') as stream:stream.write(packed_other+packed)
    try:restore(archive,'sources',target,hashlib.sha256(source+other).hexdigest())
    except ValueError:pass
    else:raise RuntimeError('swapped order accepted')
    with gzip.open(archive,'wb') as stream:stream.write(packed)
    restore(archive,'sources',target,expected)
    if target.read_bytes()!=source:raise RuntimeError('compressor readback')
    inputs=work/'inputs';output=work/'output';scratch=work/'scratch';inputs.mkdir()
    live_clock=(json.dumps([live,'node00',1,10,11,12,13,body_hash(live)])+'\n').encode()
    originals={'sources.jsonl.gz':source,'seed-ledger.jsonl':seed_raw,'data-clocks.jsonl.gz':clock_raw+live_clock}
    for name,raw in originals.items():
        if name.endswith('.gz'):
            with gzip.open(inputs/name,'wb') as stream:stream.write(raw)
        else:(inputs/name).write_bytes(raw)
    recovered=(json.dumps(['oldhistory',1,'a'*64,100,4])+'\n').encode()
    with gzip.open(inputs/'recovered-hashes.jsonl.gz','wb') as stream:stream.write(recovered)
    saved=preserve(inputs,output,scratch)
    if list(inputs.iterdir()):raise RuntimeError('verified originals not removed')
    for name,raw in originals.items():
        entry=saved[name];restored=scratch/'final-readback'
        restore(output/entry['archive'],entry['kind'],restored,entry['original_sha256'])
        if restored.read_bytes()!=raw:raise RuntimeError('preserve/restore integration')
    with gzip.open(output/'recovered-hashes.jsonl.gz','rb') as stream:
        if stream.read()!=recovered:raise RuntimeError('actual Batch hash retention')
    shutil.rmtree(work)
    return {'exact_framing':True,'wrong_seed':True,'altered_hash':True,'missing_duplicate_order_timestamp':True,'gzip_readback':True,'four_input_preserve_restore':True}
