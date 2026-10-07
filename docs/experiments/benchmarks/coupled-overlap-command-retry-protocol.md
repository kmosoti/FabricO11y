# O8 server command retry

Pilot pair1-01 failed before the service started: its driver omitted the required
`serve` subcommand. No timing result exists. Preserve the failed receipt and owned
scratch. Correct only the server invocation to `fabric-server serve CONFIG` and
record the executing driver hash alongside the original frozen-source reference.
Frozen native binaries and workload remain unchanged.

Run `catalog-overlap-pair1-02` with the registered pair command, corresponding
fresh output directory, the same freeze01,180s deadline and8MiB reserve. First
archive pair1-01 using the registered catalog cleanup helper in
`catalog-coupled-failure-cleanup-03`,120s verification stage. Historical failure
remains visible; no oracle or metric expectation changes.
