# E2R: one-sync group seals with an external error witness

Status: selected model repaired and approved by cross-family review. This is an executable
Python byte/sector model, not a new application log format. The
[frozen contract](../../../tools/seal-probe/CONTRACT.md) instantiates the prior
mad-scientist E2R registration with exact framing, fixtures and allowed outcomes.
The application keeps its FOL2 two-sync append path.

## Contract and experiment

A descriptor and one to six opaque payload frames precede a dedicated seal sector.
The seal digest covers descriptor, frame headers, body bytes and padding; the
scanner also checks seal fields and padding. A successful logical append performs
one sync after all sectors and then acknowledges the whole group. Any modeled
write/sync error poisons the handle, withholds ACK and records an external error.

The independently authored oracle fixes sector sizes 512/4096, seeds 11/12/13,
payload lengths 57, 127, 509, 513, 1023 and 4096, and earlier acknowledged data.
It explores **309,456 persisted-sector subsets**: all subsets for 33 fixtures of
at most 16 sectors, and 100,000 sampled masks for each of three larger fixtures.
It also checks **294 write/sync error boundaries**, **6,000 corruption cases**,
clean encodings, and identical visible bytes after successful versus failed sync.
The latter must scan identically but lead to different supervisor decisions.

Before implementation, the reference-only command exited 0. The full oracle exited
1 because the candidate module did not exist. The [freeze record](data/seal-e2-run-01/oracle-freeze.json)
pins those test/fixture sources. Two independent candidates were then generated in
separate scratch directories without changing the frozen oracle.

## Results and selection

| Candidate | Frozen oracle | Additional 8,448 descriptor/seal bit flips | Outcome |
| --- | --- | --- | --- |
| A | Exit 0, all seven tests | Exit 1 | Rejected: can discard an acknowledged group |
| B | Exit 0, all seven tests | Exit 0 | Selected, then repaired after review |

The extra [selection probe](../../../tools/seal-probe/test_selection.py) was written
by the integrator after inspecting candidate A, so it is explicitly separate from
the independent frozen oracle. It flips every bit in descriptor and seal metadata
for single acknowledged groups of sizes 1–6 at both sector sizes.

A minimal counterexample changes byte 17 bit 1 of a one-frame, 512-byte-sector
acknowledged group. The descriptor's 57-byte length becomes 569; the original
complete seal is still present. Candidate A follows the corrupted extent, treats
the expected seal beyond end-of-image as absent, and returns an empty committed
prefix. The probe fails with a real assertion and exit 1. Candidate B validates
frame extent/header agreement before accepting any missing seal, so it fails closed.
The [candidate and counterexample records](data/seal-e2-run-01/selection.json) retain
both implementations and their actual results.

Four source mutations were also executed with paired clean controls. Always-error
fails the clean control; truncate-on-mismatch fails acknowledged-data corruption;
always-truncate fails the seal-only subset; ignoring the external EIO flag fails
the indistinguishable-history control. All clean controls exited 0 and all mutated
variants exited 1 at the intended assertion. The [raw mutation record](data/seal-e2-run-01/mutations.json)
contains commands, source additions, hashes and outputs.

## Independent review and repair

A GPT verifier found that `append_logical` could ACK a duplicate or skipped group
id, or append after uncommitted prior bytes, even though recovery rejected the
result. The [regression before repair](data/seal-e2-run-01/pre-repair-regression.json)
failed with exit 1. The selected model now requires a fully committed previous
prefix and its exact next group id before modeling writes or ACK. This does not
remove the caller's obligation to honor a known external I/O-error witness.

Both reviewers reran the relevant checks on the repaired source. The
[GPT repair review](data/seal-e2-run-01/gpt-repair-review.json) and
[Claude repair review](data/seal-e2-run-01/claude-repair-stdout.json) approve it after
executing independent probes. Frozen oracle and selection/regression checks exit 0.
Claude's arbitrary `python -c` helper was denied; allowed test/probe commands
completed. The source hash and each selection/repair step are in `selection.json`.
The original contract remains archived; the live contract adds the input-validation
clarification without changing the frozen fixture/test corpus.

## Reproduction

```sh
python3 -B -m unittest discover -s tools/seal-probe -p test_oracle.py -v
python3 -B tools/seal-probe/test_selection.py tools/seal-probe/candidate.py
```

These are correctness checks, not timed performance comparisons. Candidate B's
source hash is recorded in the selection file. The separate
[cost comparison](../benchmarks/group-seal-cost-run-01.md) completed all 96 trials: the
512-byte group-1 cell failed its CPU gate; the 4096-byte cell passed both gates.
The original [protocol](../benchmarks/group-seal-cost-protocol.md) is retained.

## Interpretation and limits

Under the model's sector-atomic writes, single writer, preserved earlier ACKed
sectors, honored successful syncs, durable namespace and collision-resistant hash,
only validated whole groups are exposed. An unsealed tail can fail closed
conservatively. Returning an error preserves the entire input byte string.
This trades some recoverability of ambiguous tails for avoiding silent loss.

The file scanner cannot infer an earlier failed sync from valid-looking bytes.
The external error witness is mandatory, and its durable implementation is future
work. The model does not cover concurrent writers, torn sectors, malicious hash
recomputation, physical power loss or arbitrarily erased acknowledged seals.
Completely erasing an ACKed seal can be indistinguishable from never writing it;
no unconditional corruption-recovery claim is made. The payloads are opaque bytes,
not complete Fabric Events. Finite checks do not prove arbitrary implementation
behavior or select a production storage format.

The discarded candidate shows why the random fault corpus alone was insufficient.
Cost, physical durability and the end-to-end prototype remain separate gates in
[the completion contract](end-to-end-prototype.md).
