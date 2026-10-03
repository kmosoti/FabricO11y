import json, subprocess, shutil, statistics, glob, os, sys
B="target/release/examples/sealbench"; W="/dev/shm/sb"
OUT="."
algs=["base","chunk","watermark:16384","watermark:65536","partition:32768","merge:32768","hybrid:16384","hybrid:65536"]
reps=int(sys.argv[1]) if len(sys.argv)>1 else 3
res=[]
for wl in ["steady","outage","adversarial","bigrows"]:
    inp=sorted(glob.glob(f"{W}/{wl}/sealed-*.faj"))
    for a in algs:
        runs=[]
        for r in range(reps):
            od=f"{W}/out/{wl}-{a.replace(':','_')}"
            shutil.rmtree(od,ignore_errors=True)
            runs.append(json.loads(subprocess.check_output([B,"run",a,od]+inp)))
        segs=glob.glob(f"{W}/out/{wl}-{a.replace(':','_')}/seg-*")+glob.glob(f"{W}/out/{wl}-{a.replace(':','_')}/segments/seg-*")
        base=glob.glob(f"{W}/out/{wl}-base/segments/seg-*")[0]
        v=json.loads(subprocess.check_output([B,"verify",base,segs[0]]))
        leftovers=[p for p in os.listdir(os.path.dirname(segs[0])) if p.startswith('.')]
        row={"workload":wl,"alg":a,"runs":runs,"verify":v,"leftover_temp_files":leftovers,
             "wall_median":statistics.median(x["wall_s"] for x in runs),
             "cpu_median":statistics.median(x["cpu_user_s"]+x["cpu_sys_s"] for x in runs),
             "peak_heap_max":max(x["peak_heap_bytes"] for x in runs),
             "peak_heap_min":min(x["peak_heap_bytes"] for x in runs)}
        res.append(row)
        print(wl,a,"heap MiB %.1f"%(row["peak_heap_max"]/2**20),"wall %.2fs"%row["wall_median"],"cpu %.2fs"%row["cpu_median"],
              "late",runs[0]["late_logs"],"spill MiB %.1f"%(runs[0]["spill_bytes"]/2**20),
              "same_rows",v["logs"]["same_rows"],v["metrics"]["same_rows"],"order",v["logs"]["same_order"],v["metrics"]["same_order"],
              "amp60 %.2f/%.2f"%(v["logs"]["read_amp_60s"],v["metrics"]["read_amp_60s"]),"manifest",v["manifest_equal"],"batches",v["batches_same"],"gaps",v["gaps_same"],flush=True)
json.dump(res,open(f"{OUT}/matrix.json","w"),indent=1)
