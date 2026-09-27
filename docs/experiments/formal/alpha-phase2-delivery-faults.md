# Phase-2 delivery under process faults, run 01

Status: all eight fault runs passed the frozen [delivery oracle](../../../tools/alpha/DELIVERY_ORACLE.md) on 2026-09-27, and both negative controls failed as intended. This covers three nodes and one server on one host. It is not the ten-process, latency or commit-mode gate of phase 2.

## Method

[`delivery_faults.py`](../../../tools/alpha/delivery_faults.py) starts one `fabric-server` over TLS with throwaway `openssl` certificates and three `fabric-node run` processes. Each node collects metrics every second and a writer appends ten 83-byte log lines per second. Runs last 20 s, then writers stop, nodes get 3 s to drain and are stopped with SIGTERM, and the server is stopped with SIGTERM. Scenarios:

- `clean`: no fault.
- `server-kill`: SIGKILL the server at three random times and restart it after 0.5 to 2 s.
- `node-kill`: SIGKILL a random node six times and restart it.
- `outage`: SIGKILL the server at 3 s and restart it 10 s later.

The transcript is built only from observations the oracle's adapter contract allows: spool dumps taken while a node is stopped, the node's per-attempt delivery lines (SHA-256 of the bytes sent, written before the ACK cursor is persisted), and `server_dump` from a fresh process after the server stopped. A run passes only if the oracle passes and every node and the server exit 0.

```sh
python3 -B tools/alpha/delivery_faults.py --scenario <clean|server-kill|node-kill|outage> --seed <1|2> --seconds 20
python3 -B tools/alpha/delivery_faults.py --scenario clean --seed 3 --seconds 8 --mutate drop-recovered
```

Binary and script hashes are in [fault-run-01-hashes.txt](../benchmarks/data/alpha-phase2/fault-run-01-hashes.txt); per-run summaries are in [fault-run-01.jsonl](../benchmarks/data/alpha-phase2/fault-run-01.jsonl) and exit codes in [fault-run-01-exits.txt](../benchmarks/data/alpha-phase2/fault-run-01-exits.txt).

## Results

| Scenario | Seed | Sources | Attempts | ACKs | Recovered | Oracle |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| clean | 1 | 71 | 70 | 70 | 70 | pass |
| clean | 2 | 71 | 71 | 71 | 71 | pass |
| server-kill | 1 | 71 | 95 | 71 | 71 | pass |
| server-kill | 2 | 71 | 92 | 71 | 71 | pass |
| node-kill | 1 | 75 | 75 | 75 | 75 | pass |
| node-kill | 2 | 75 | 75 | 75 | 75 | pass |
| outage | 1 | 69 | 105 | 69 | 69 | pass |
| outage | 2 | 72 | 106 | 69 | 69 | pass |

Attempts above ACKs are retries after a killed or absent server. Sources above recovered are batches committed on the node after its last successful send; they remain in its spool, which the oracle checks.

## Negative controls

- `--mutate drop-recovered` removes one recovered record before grading. The run failed with `ACKED-DURABLE`, exit 1.
- A node mutant that persisted its ACK cursor before sending failed `node-kill` seeds 1 and 2 with `NODE-RETAINS-UNACKED`. That mutant was built and run once and then reverted; it is not in the tree.
- The end-to-end Rust tests in [delivery.rs](../../../crates/fabric-server/tests/delivery.rs) fail on a server that acknowledges a same-sequence retry regardless of bytes, and on a server that acknowledges without appending.

## Limits

One host, loopback TLS, three nodes, 20 s runs. Faults are process kills and a server outage; I/O errors on the server journal are covered by the shared frame log's unit tests, not by this harness. The node spool stayed below the 8 MiB rotation size, so reclaim was not exercised here; it has unit tests. Physical power loss is untested.
