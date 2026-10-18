# Receipt hash-comparison negative control

Register before the isolated mutant build. Starting from the measured native
receipt verifier, create a separate, clean DATA worktree. Change only the final
ArtifactStore digest comparison to accept a non-JSON artifact even when its
expected digest differs. Keep full reads, path admission, metadata comparisons,
budgets and JSON checks intact. The defective branch is an experiment artifact,
never a production candidate.

Build both candidate and differential driver at that same mutant revision under
the required resource launcher. Archive their hashes before execution. Run the
existing 25-case differential with fresh DATA output. Expected observations:
the unchanged Python reference and defective native verifier accept the positive
fixture; the modified-log fixture is rejected by Python but accepted by the
mutant; all other cases retain their registered outcomes, including the four
intentional strict-representation differences. The driver must exit 1, record
exactly the modified-log unexpected mismatch, and defer benchmarks. A source,
path or startup failure is not evidence that the hash defect was detected.

Preserve the mutant commit, binaries, logs, report, actual launcher exit and
whole-tree stop. Do not interpret this synthetic receipt fixture as execution of
the registered product checks. This control establishes one concrete defect
detection, not completeness against arbitrary verifier defects.
