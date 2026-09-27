import json,pathlib,subprocess,struct,zlib,hashlib
R=pathlib.Path('/home/kmosoti/fabric-cli-review-big');R.mkdir(exist_ok=True); B='/tmp/fabric-cli-b/tools/storage-probe/target/release/fabric-research'
req=R/'big-request.json'; cfg=R/'config.json'; inp=R/'big-input.json'; one=R/'big-one.fol2'; big=R/'big-over64.fol2'; snap=R/'big-snap';trust=R/'big-trust.json'
import shutil
for q in [req,inp,one,big,trust]:
 if q.exists(): q.unlink()
if snap.exists(): shutil.rmtree(snap)
cfg.write_text('{"tenant":4,"source":5,"resource":6,"first_event_id":10}')
req.write_text(json.dumps({'resourceLogs':[{'scopeLogs':[{'logRecords':[{'body':{'stringValue':'X '*(4*1024*1024)}}]}]}]}))
for args in [('adapt-otlp',req,cfg,inp),('ingest',inp,one,1,1)]:
 p=subprocess.run([B,*map(str,args)],capture_output=True,text=True); print(args[0],p.returncode,p.stderr[:300]); assert p.returncode==0
frame=one.read_bytes(); size=struct.unpack('<I',frame[4:8])[0]; assert len(frame)==16+size+16
header_payload=frame[:16+size]
with big.open('wb') as f:
 for i in range(9):
  f.write(header_payload)
  data_end=(i+1)*len(frame)-16
  marker=b'FOC2'+struct.pack('<Q',data_end)
  f.write(marker+struct.pack('<I',zlib.crc32(marker)))
assert big.stat().st_size>64*1024*1024
p=subprocess.run([B,'publish',str(big),str(snap),str(trust),'1','999'],capture_output=True,text=True,timeout=120)
result={'argv':['publish',str(big),str(snap),str(trust),'1','999'],'log_bytes':big.stat().st_size,'input_bytes':inp.stat().st_size,'exit':p.returncode,'stdout':p.stdout[:500],'stderr':p.stderr[:500],'snapshot_exists':snap.exists(),'trusted_exists':trust.exists()}
print(json.dumps(result))
(pathlib.Path('/tmp/fabric-cli-review')/'oversize-log.json').write_text(json.dumps(result,indent=2))
