# O7 archived pruning diagnostic

Status: registered before validator execution. No new builder, acceptance rule,
oracle or performance comparison. Source-grounded preparation is in
[operations preparation](coupled-operations-preparation.md).

Read the existing `builder-bigrows-64` first-pair reference/bounded readbacks and
pair receipt. Hash exact input reports. Require receipt/readback agreement for
input, byte counts, ordered ledgers, logical counts and window reports. Validate
group cardinalities and window types/order/widths/totals. Compare equal logical
matched rows and report differing hypothetical footer-admitted row counts.
Reject altered ordered ledgers, changed matched rows, malformed width/order;
a physical count difference must remain visible without changing the old gate.

H1: archived first-pair reports are internally consistent with changed grouping
and equal reported logical rows. H0: malformed reports or logical/input drift.
Actual pruning soundness remains inconclusive: no retained per-group identity
and min/max mapping or independent full-query chains exists in these reports.
Counts are hypothetical footer estimates, not measured IO. Rust's suppression
of counts in zero-match windows remains an explicit evidence limitation.

Run through existing serial admission with 4 MiB aggregate reserve:

```sh
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-coupled-o7-ledger-01 --lab recovery --stage recovery --seconds 60 -- python3 -B tools/bench/labs/catalog/coupled_admit.py --reserve-mib 4 -- python3 -B tools/bench/labs/catalog/coupled_pruning.py --reports docs/experiments/benchmarks/data/lab-completion-run-01/memory/builder-bigrows-64 --out docs/experiments/benchmarks/data/catalog-coupled-o7-ledger-01
```

Exit zero means the diagnostic and injected rejection controls completed, not
that historical C2 pruning-equivalence passed. Original metrics/failure stay
unchanged. Any further soundness ledger/checker requires separate registration.
