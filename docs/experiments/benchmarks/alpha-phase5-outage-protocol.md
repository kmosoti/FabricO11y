# Registered phase-5 measurement: outage buffering and drain

Status: registered on 2026-09-28 before any registered outage run. Results go in a separate run record.

## Question

Does one real `fabric-node`, offered the registered source, buffer a 30-minute server outage and drain within 10 minutes after the server returns, with exact delivery?

## Method

[`outage_drain.py`](../../../tools/alpha/outage_drain.py) runs one `fabric-server` and one enrolled `fabric-node run` whose log path comes from central control. A writer offers 2 lines/s of 512-byte bodies, alternating `R` × 512 and the phase-0 `entropy_body(seed, 0, tick)`, for the whole run; host metrics are sampled every 15 s. The server runs 60 s, is stopped with SIGTERM for 1,800 s, then restarted. Drain time runs from the restart until the node's acknowledged sequence reaches the last sequence it had committed at the restart, while the writer keeps offering. The run continues 60 s after the drain. Seeds `0xA11FA001` to `0xA11FA003`, one trial each, sequential, under the frozen [runner](../../../tools/alpha/runner.py) with a 3,000 s duration limit, 1 GiB live-data limit and 1 MiB evidence limit.

## Decision rule

A trial passes when drain time is at most 600 s, every offered line is committed on the node, the [delivery oracle](../../../tools/alpha/DELIVERY_ORACLE.md) passes over the node spool, the node's delivery lines and `server_dump`, node VmHWM stays at most 64 MiB, and both processes exit 0. Peak spool bytes during the outage are reported. A budget stop or unrun trial is a failure.

## Limits

One host and loopback TLS; the outage is a stopped server process, not a network partition.
