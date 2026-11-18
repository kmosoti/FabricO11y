---
id: O-0180
date: 2026-11-18
added_by: agent
human_ratified: false
---

# Release deployment and physical two-host qualification

The release executables were built with Rust 1.100.0 from
`df3db73aa6e0e707385275a2751b314d7e9727c6`.
The fixture source is at `4528400b3ac206153b2963d4edd994f7448a3d71`.
The intervening commit updates the test fixture only.

| Artifact | SHA-256 |
| --- | --- |
| Fabric CLI | `0ba174d69faad16358ea583b0a780baebfefa28e2a6f561bfee813c0ffea4c9e` |
| Spindle | `2c60350751ac7b56c7b3f84782f765444f1ffa868b8a536519a3fc672d3f5f0f` |
| Fabric Server | `b66be40c57aebd87a0e349aba59a1970169f8f45141ee6ed7f31dd0066f86634` |
| Confined test executable | `39d09156710e260d68524c3fef7fdd06728e0186d202c59a527e28a5096e8de9` |

## Confined release fixture

The [deployment fixture](../../docs/deployment-verification.md#confined-executable-fixture)
passed in 83.81 seconds. Its transient systemd unit reported a 152.3 MiB managed
memory peak across the test and all its processes, with zero swap. The sandbox
used the configured `fabrico11y.slice` quotas. It used three independent Spindle
stores and a SQLite/Parquet receiver on the same Linux host.

- Local projection readiness moved from pending to visible for the same
  receipt. While the receiver was stopped, another durable receipt remained
  visible locally. It became canonically visible after restart and replay, and
  the fixed canonical prefix contained exactly one copy.
- The dedicated telemetry lane acknowledged a fixed prefix through source ID
  55. Both services supplied metrics, events, logs, and traces. Each raw row
  in that prefix matched the canonical response byte for byte. The isolated
  local projection returned both producers in its metrics, logs, and traces.
- The receiver emitted one local diagnostic about blocked or dropped internal
  telemetry. The fixture checks delivery of each signal type and the
  exact acknowledged prefix. It does not measure exporter loss rates.

## Physical forwarding run

The same receiver and a debug-stripped copy of the release Spindle were used
with two independent DigitalOcean Linux hosts. Each origin ran in its own
scoped transient systemd service over a separate authenticated SSH reverse
tunnel. The receiver ran on fedora-blade. The edge spool directories were on
persistent ext4. The transferred Spindle had SHA-256
`84d3a5e445f3058fba1ccba0a94973b831088b106b84df0c7f832dddba4989e2`.

Both origins durably accepted source ID 1 while the receiver was down. After
reconnection, the receiver reported two separate frontiers through ID 1. An
explicit typed query returned both exact rows, and the credential for one
origin could not read the other. The test stopped its temporary services and
closed its tunnels. This physical run exercised workload forwarding,
authorization, and outage catch-up. MELT delivery was exercised in the confined
release fixture.
