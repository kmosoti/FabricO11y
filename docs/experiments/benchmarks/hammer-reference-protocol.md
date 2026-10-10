# Service pressure, native references and remote edge trials

Prospective registration, 2026-10-09. The owner requested heavier experiments,
then replaced the 10 GB server allowance with **4,000,000,000 bytes for the
Fabric server only** and a **100,000,000,000-byte storage ceiling**. The local
laboratory remains inside its existing 16/20 GiB, zero-swap, 30-minute service.
This is diagnostic research, not deployment qualification. No product defaults,
wire formats, durability order or independent oracles change.

## Questions and decision rules

H1: the server drains a rising offered workload while retaining exact custody
and answering complete queries within 4 GB. H0: some combination of collection,
admission, sealing, query work or resource pressure prevents that. Record actual
offered/accepted/ACKed rates, producer lag, backlog, process CPU/RSS, child cgroup
anon/file/kernel pressure, disk and IO, query latency and observed visibility.
Latency misses are results, not reasons to suppress a run. Any semantic failure
stops that track for investigation. Distinguish producer saturation from server
saturation; a finite plateau does not establish a long soak.

Local cells use twenty real Spindles, TLS, 900-byte deterministic alternating
compressible/high-entropy bodies and 60-second phases at 1,000, 4,000 and 16,000
aggregate logs/s (1,260,000 logs), followed by 180 seconds without new source
logs while metrics and queries continue. Seed 2704101 runs Scan then Walk;
seed 2704102 runs Walk then Scan, if initial cells preserve semantics and budget
remains. Same source generation, four server CPU equivalents, two collector
CPU equivalents, one sealer, normal 16 MiB sort cap, 64 MiB journal files,
4 GiB journal/retention caps, 24-hour retention. Server memory high/max is
3/4 decimal GB; collector aggregate high/max 768/1024 MiB, no swap, task caps.
Every case has a 1,400-second absolute deadline, including replay, independent
grading and preservation; enclosing job at most 1,450 seconds. Full independent
source/ACK hashes and final query chains are retained as separate correctness
boundaries. Old 1-second ACK/2-GiB RSS thresholds remain labeled diagnostics,
not acceptance criteria for this larger workload. Required correctness includes
exact recovered source, no unexplained gaps/duplicates, ACK custody, contiguous
sequences, complete exact query chains, clean exits, drained backlog and no OOM.
Quiet-window residency uses the previous first/final 60-second discriminator
only as a finite observation. No query default is selected from these trials.

H2: representation and pushdown have different cost regimes on matched telemetry.
Three fresh native Arrow/DuckDB/Vortex processes use seeds 2704001–2704003,
500,000 rows, 512-byte alternating bodies and 1,000 node identities. Each writes
Parquet (Zstd 3, 8,192-row groups) and Vortex, including conversion/write costs.
Five query shapes cover time, node/time, rare literal, absent literal and full
count/body-length aggregation. Three repetitions rotate engine order, returning
exact ordered Top50 IDs/bodies plus counts, or exact aggregates. Python source
formulas provide expectations and missing/extra/changed/order/count defects must
be rejected. Record CPU/wall, file bytes, cumulative process HWM and process
samples. Arrow full materialization, DuckDB SQL projection and Vortex predicates
are deliberately different mechanisms, not equivalent execution strategies.
Pinned wheels are execution artifacts; their versions differ from the reviewed
source revisions. No cold-cache or engine-wide winner claim follows.

H3: an actual small remote edge can preserve custody while forwarding through
the established private path. Read-only inspection found both droplets have
one CPU and about 458 MiB RAM. `digitalocean-01` runs Caddy and receives no load.
Use only `digitalocean-02`, disk-backed owned `/var/tmp` scratch, a 128 MiB max /
96 MiB high cgroup, 50% CPU, zero swap, 128 tasks and at most 600 seconds per
remote service. Keep remote fixtures below 512 MiB and at least 2 GiB disk free.
Use a loopback-only SSH reverse tunnel over Tailscale to the local TLS server;
change no frozen SSH, tailnet or firewall configuration.

Run eight real Spindles at 80/320/1,280 aggregate 900-byte logs/s for twenty
seconds per phase, then allow sixty seconds to drain. Run a separate native
simulator with 100 identities, two workers, 60 seconds, five 900-byte logs per
half-second per identity (1,000 logs/s) using its existing fixed source and
delivery transcript. Use current rebuilt binaries with identities recorded.
Validate real source bytes and ACK hashes against local durable replay; grade
simulated delivery with the unchanged independent delivery oracle and a dropped
recovery negative control. Query visibility and timing on different clocks are
reported only with their measurement boundary; do not assume synchronized
hosts. Record both ends' resource counters, offered/accepted rates, retries,
final backlog, process exits and cleanup. An incompatible binary or unavailable
private path is a recorded environment failure, not a performance result.

## Admission and evidence

This owner-scoped pressure round allocates at most 7,200 additional seconds of
serialized local jobs. Preserve and charge all earlier ledgers: the shared
frontier grows by this explicit allocation, while the 86,400-second campaign
ceiling remains. Each outer job stays below 1,800 seconds. Retained evidence
ceilings are 2 GiB service, 512 MiB reference, 128 MiB coordinator; owned scratch
is at most 8 GiB per active job, with a 16 GiB data-drive free reserve. These
stricter operational bounds sit below the owner's storage ceiling. Do not
fill the disk merely because capacity is permitted. Failed raw trees remain
on the data drive until exact preservation; successful raw fixtures may be
removed after exact grading and compact evidence verification. Remote failures
are copied back and authenticated before remote deletion.

Freeze command, protocol, harness, binaries and relevant source identity before
measurement. Heavy jobs are serial; preparation agents do not run workloads.
Keep original failures and separate retries. Finish with results, model updates,
remaining uncertainty, relevant checks, receipts and owned process/file cleanup.
The old soak and full acceptance matrix remain independent and unclaimed.
