# S0 run-01 evidence

See [the result and limits](../../append-attribution-s0.md). Raw write/verify CSV
uses `.stdout`; build output and errors use `.stdout`/`.stderr`. `commands.jsonl`
records actual subprocess exits and resources. `summary.json` contains per-trial
metrics. `artifacts.json` from the runner also hashes reproducible `.fol2` scratch
logs left under `target/append-attribution-s0/run-01`; those binaries are not checked
in. `protocol-at-run.md.txt` pins the protocol before results were added. Reviewer
counts are their reports; the underlying recorded command output is authoritative.
