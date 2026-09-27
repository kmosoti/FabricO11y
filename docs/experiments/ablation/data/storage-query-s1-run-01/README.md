# Preserved S1 run 01

See the [result and limits](../../storage-query-s1-run-01.md) and [fixed protocol](../../storage-query-s1-protocol.md). The runner completed successfully on 2026-09-27 UTC.

- [queries.csv](queries.csv): all 15,360 timed executions, including logical work, timing, match counts and equality observations.
- [builds.csv](builds.csv): 12 logs/snapshots with append, replay/verification and summary-build wall times, raw bytes, logical summary bytes and cumulative process RSS.
- [summary.json](summary.json): validated grid, per-seed/per-case reductions and trial quantiles; [resources.json](resources.json): whole-process CPU and RSS.
- [environment.json](environment.json): commands, starting revision, toolchain/host/filesystem, build settings and exact measured source hashes; [protocol.md](protocol.md): preregistration snapshot.
- [selection.json](selection.json): both candidates' oracle outcomes and the injected lost-block defect.
- [csv-mutation.json](csv-mutation.json): rejected corruption of a copy of real query output.
- [review.json](review.json): cross-family review outcomes and execution limits.
- [SHA256SUMS.json](SHA256SUMS.json): original runner artifacts; [PRESERVED_SHA256SUMS.json](PRESERVED_SHA256SUMS.json): complete preserved artifacts excluding this README and the manifest itself.

The twelve generated `.log` files remain disposable local run artifacts. Regenerate them using the runner; this directory preserves the measured CSV evidence rather than another 5.7 MB of synthetic logs. No disk format or query winner is selected by these data.

Recompute the summary directly from this directory:

```sh
python3 -B tools/bench/summarize_storage_s1.py docs/experiments/ablation/data/storage-query-s1-run-01
```

Check the preserved hashes from the repository root:

```sh
python3 - <<'PY'
import hashlib, json
from pathlib import Path
p = Path('docs/experiments/ablation/data/storage-query-s1-run-01')
for name, expected in json.loads((p / 'PRESERVED_SHA256SUMS.json').read_text()).items():
    assert hashlib.sha256((p / name).read_bytes()).hexdigest() == expected, name
print('all preserved SHA256 hashes match')
PY
```
