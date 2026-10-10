# Remote diagnostic reducer name-shadow counterexample

Origin: `remote-reduce-two-01` and `remote-reduce-eight-01` on 2026-10-09 both
exited 1 at `remote_reduce.py:214` with `TypeError: 'str' object is not callable`.
The original coordinator logs/receipts are retained. Both had read the retained
simulator events and recovery hashes before failing to write their final report.
This is a Python diagnostic-tool defect, not a new Fabric delivery failure.

The recovery row unpack used `label, sequence, digest, size, _received_ns`.
Assigning that local string shadows the module's `digest(path)` function for
the whole `main` scope. The later input-hash comprehension then calls a string.
Minimal reproducer:

```python
def digest(path):
    return path

def report():
    for digest in ["a retained SHA-256 string"]:
        pass
    return digest("input")  # TypeError
```

Rename the unused unpacked hash to `_record_digest`; retain every count,
timestamp, archive and oracle assertion. Retry the same two retained inputs
with fresh `remote-reduce-two-02` / `remote-reduce-eight-02` IDs. These full
replays discriminate the original failure and exercise final report generation;
do not relabel the failed attempts or infer success before the retries finish.

Both retries subsequently exited 0 under the same contained ledger. The full
input replays reached report generation and retained matching exact-delivery
counts. See the [two-worker report](data/hammer-reference-01/query/remote-reduce-two-02/remote-reduction.json)
and [eight-worker report](data/hammer-reference-01/query/remote-reduce-eight-02/remote-reduction.json).
