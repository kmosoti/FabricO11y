# Continuation documentation closeout retry

Status: prospective retry02, registered after closeout01 exited1. Its1241
transformation readbacks and changed-hash control succeeded; documentation failed
on two relative links in the byte-exact bootstrap protocol copies. Checker probes
and hook tests passed. Original records and source copies remain.

Preserve closed bootstrap copies at their original paths. Add exact context
copies under each archive directory so their links resolve: reclaim01 gets
`data/catalog-overlap-freeze-01/freeze.json`; reclaim02 gets the initial protocol
and the same nested freeze receipt. Only these three declared files are added.
Verify source/destination bytes and SHA256, reject existing conflicting content,
and save provenance in the new closeout receipt. No documentation rule is waived,
no checker exclusion is added, and no historical source copy is rewritten.

Archive/remove only closeout01's failed launcher scratch with the existing
contained catalog cleanup command, fresh cleanup02 report,60-second deadline,
4MiB evidence reservation. Preserve its original manual check receipts before
the retry changes current verification receipts. Then use closeout02 with the
same180-second deadline,4MiB reservation, full retained readbacks, syntax,
manual documentation checks, verified cleanup archives and inventory. The same
frontier, stage, cgroup and disk ceilings apply; no new native cell or Rust change.

```sh
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-coupled-continuation-cleanup-02 --lab coordinator --stage verification --seconds 60 -- python3 -B tools/bench/labs/catalog/coupled_admit.py --reserve-mib 4 -- python3 -B tools/bench/labs/catalog/coupled_cleanup.py --id catalog-coupled-continuation-cleanup-02 --only-job catalog-coupled-continuation-closeout-01
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-coupled-continuation-closeout-02 --lab coordinator --stage verification --seconds 180 -- python3 -B tools/bench/labs/catalog/coupled_admit.py --reserve-mib 4 -- python3 -B tools/bench/labs/catalog/coupled_continuation_closeout.py --id catalog-coupled-continuation-closeout-02
```
