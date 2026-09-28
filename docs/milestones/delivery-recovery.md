# Milestone: delivery and recovery qualification

Status: complete and merged into `main` (`2d67cc4`); CI green on `e89c965` (Rust, Documentation, Extended verification). Base: the history-qualification milestone, merged at `d54db38`.

This milestone runs the registered outage protocol, registers and runs [stress revision 2](../experiments/benchmarks/stress-protocol-r2.md), and registers and runs a [soak](../experiments/benchmarks/soak-protocol.md). Every run is on a four-CPU host, so no result is a target-profile qualification. The soak **failed** one gate, and that result is recorded as it is. The milestone changes no product code, wire format, persisted format or oracle.

## Acceptance criteria

| ID | Criterion | Deciding evidence |
| --- | --- | --- |
| DR-1 | The protocols are registered before the runs, each in its own commit. Harness changes keep revision 1's defaults, and a new harness has negative controls | stress revision 2 (`01989cc`); the soak protocol and `soak_tier.py` (`1c67ff3`); `test_soak_tier.py`, registered in the fast profile (`2b5c939`) |
| DR-2 | The binaries and harnesses are frozen with hashes and are unchanged after the runs | `hashes.txt` files under [data/delivery-recovery](../experiments/benchmarks/data/delivery-recovery/hashes-p5r2.txt); `sha256sum -c` after the runs |
| DR-3 | The registered outage protocol runs to a result for all three seeds | [outage run 01](../experiments/benchmarks/outage-run-01.md) |
| DR-4 | Stress revision 2 runs to a result for all three seeds | [stress run 01](../experiments/benchmarks/stress-run-01.md) |
| DR-5 | The soak runs to a result under its registered decision rule | [soak run 01](../experiments/benchmarks/soak-run-01.md) |
| DR-6 | Failed gates, failed starts, deviations and other host load are recorded, not hidden | the three run records |
| DR-7 | The records use the evidence states exactly | the capability ledger, verification matrix, experiments index, roadmap and current state; docs check exit 0 |

The milestone does not require the gates to pass. It requires every registered run to produce a recorded, honestly stated result.

Out of scope, stated so it is not implied:

- **Revision 1 on the 12-CPU target host.**
- **A sealer change.** It is an implementation change with its own milestone scope, followed by soak run 02.
- **Burst-window latency.** The registered stress harness does not compute it, and adding it would be a new revision.

## Definition of done

DR-1 to DR-7 are met, with the commands and exits recorded below. The PR's CI is green (Rust, Documentation, Extended verification), and the PR is merged into `main`.

## Results

Host: 4 logical CPUs, 15 GiB RAM, Ubuntu 24.04 (Firecracker VM), ext4, Python 3.11.15. The binaries in every run are byte-identical builds of the Rust sources at `63bbeac`.

| Criterion | Command | Exit | Result |
| --- | --- | --- | --- |
| DR-1 | `python3 -B tools/qualification/test_soak_tier.py` | 0 | Clean trial passes; one injected defect per gate fails exactly that gate. Three gate weakenings (ACK limit, RSS factor, sealing condition) each make the test exit 1 |
| DR-1 | `cargo xtask checks --profile fast` | 0 | 17 of 17, including `soak-decision-rule` |
| DR-2 | `sha256sum -c hashes.txt`, for both frozen sets, after the runs | 0 | 26 and 28 files, 0 mismatches |
| DR-3 | outage smoke test (20 s outage) | 1 | The frozen set lacked `spool_dump`; nothing was measured. The set was rebuilt with every example the harnesses use |
| DR-3 | outage smoke test, rebuilt set | 0 | Passed; not a trial |
| DR-3 | the three registered outage invocations | 0 (each) | Every gate passed. Drain 99.0, 97.9 and 97.9 s (limit 600). Every line committed; oracle exact; node VmHWM at most 5.2 MiB |
| DR-4 | the three stress revision 2 invocations | 0 (each) | Every gate passed. Oracle exact over 180,000 batches. 600 rejections answered 401 or 400, with none committed. Backlog at most 8. Management 265–268 rounds with none failing. Server VmHWM at most 648 MiB |
| DR-5 | the soak invocation | 1 | **Failed** `no_rss_growth`: last-window median RSS 535.7 MiB against a threshold of 343.1 MiB. The other nine gates passed: 546,000 batches oracle-exact, ACK p99 at most 63.6 ms in every window, backlog 0, 546 complete queries (p99 302 ms), 91 management calls, 10 Segments, VmHWM 623 MiB |
| DR-6 | — | — | Recorded in the run records: the outage smoke failure; the host load during outage trial 1's outage phase and trial 3's first minute; the stress harness's missing burst-window latency; the soak counterexample and next action |
| DR-7 | `bun tools/docs/check.mjs` | 0 | Documentation checks passed |

State after this milestone:

- **Outage and drain:** Measured and passing.
- **Burst, rejection and concurrent management:** Measured and passing under revision 2.
- **Soak:** Failed.
- **Target profile:** none of these is qualified.
