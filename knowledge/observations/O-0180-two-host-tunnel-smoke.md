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

## Physical workload and self-telemetry qualification on the integrated tree

The source used for the next release build was
`8eb1f2e31c8a16bb2d6a0d94db75a98a587ba591`. It has the same Git tree as
integrated `develop` revision `0d14f2c517ed1a0a49d2445be77cda9c9e3bf796`.
Rust 1.100.0 ran in the pinned builder image
`rust@sha256:ade39a710ef85f3506cffcfec3e8a24ba090badef1dff2fb798f71d878833fea`.

| Artifact | SHA-256 |
| --- | --- |
| Fabric CLI | `794c82a86800b58f3f13ad70afd2fde91ab25d87daf7b92e905f5fe25ee3d11a` |
| Spindle | `0e84da64bfb75421c264babe7639ee7af0a9c39f60c344941a5dcf61a1e452cc` |
| Fabric Server | `8e9722adbc6c6c39b1d082f28630516c4b1965fa55c4c30db304023efa6f8655` |
| Physical harness | `f9f15533839080d497a9be13f1132cb53c7cfcb77be564d607e433ec9b37bfb3` |
| Physical run log | `99454a0637ff732c070e7963c25ce6f82c859f93fa98e71307229805538df07d` |

The 4 minute 49 second trial ran the edge services natively on two DigitalOcean
Ubuntu hosts with glibc 2.39, Linux 6.8.0-142, and ext4. The receiver ran on
fedora-blade in the installed `fabrico11y.slice`. Both remote test slices
reported `MemoryHigh=256M`, `MemoryMax=512M`, `CPUQuota=15%`, `IOWeight=30`, and
`TasksMax=64`. Each host exporter delivered 16 selected metric names under a
distinct node ID and authenticated Spindle origin.

- During a single-origin tunnel outage, both origins accepted a durable
  receipt with source ID 18. The disconnected receipt was visible locally
  while its canonical frontier stayed at 17. The connected origin advanced
  to 18. After reconnection both exact rows appeared once at the explicit
  two-origin snapshot through ID 18. The node credential for one origin
  could not read the other's rows.
- A separately quota-limited telemetry origin, excluded from receiver workload
  accounting, delivered all four signal types from `spindle.self` and
  `fabric-server.self`. All 116 rows from the fixed local prefix matched
  canonical readback byte for byte. After the workload producers stopped and
  one reporting interval elapsed for drain, the telemetry-only prefix advanced
  from 127 to 133 over the next interval. Canonical delivery caught up and the
  interval contained zero receiver workload-operation traces.
- The receiver logged two blocked-or-dropped telemetry diagnostics. Its
  reported drop counter increased from 0 to 1 in the compared prefix. At the
  sampled points, Spindle telemetry health reported zero drops and no blocked
  delivery, and the forwarding error fields were null.
- Across three snapshots, the largest sampled RSS was 76,632,064 bytes on the
  receiver and the largest sampled descriptor count was 21 on the second edge
  collector. Spool and collector data grew by 33,547 bytes on the first edge
  and 18,061 bytes on the second over the selected interval. These point-in-time
  observations cover the bounded trial. Sustained load, loss rates, continuous
  resource peaks, and power-loss durability require separate measurements.

## Confined release regressions

Release tests built from product revision
`be949cc08b67636ab394871a043c7bafe5d636e9` passed as transient systemd units
in `fabrico11y.slice`. The `fabric-query` and `fabric-server` trees are identical
at that revision and at `0d14f2c517ed1a0a49d2445be77cda9c9e3bf796`.

| Test executable | SHA-256 | Test time | Unit memory peak |
| --- | --- | --- | --- |
| Query over 1,024 segments with `RLIMIT_NOFILE=64` | `55cb1f21036dd030a6f1e1476740c8e1e7480ff4d8d3cc9254dfd90670dec6e7` | 18.51 s | 75.7 MiB |
| Continuous ingest and source-prefix reads on shared SQLite custody | `cb2fafc6056361f3733b70036d4c6c612cbbe062005c499746765a39dbfccdaa` | 1.31 s | 9.5 MiB |

Both units reported zero swap. The regression log SHA-256 is
`b6cdd1fbb8a2dbafd7bcd8522c3175b2e868817eeead5b648a563ebcc29c7698`.

## Confined deployment rerun with the integrated release executables

The same three release executables used in the physical run passed the confined
same-host deployment fixture in 80.32 seconds. The fixture executable retained
SHA-256 `39d09156710e260d68524c3fef7fdd06728e0186d202c59a527e28a5096e8de9`
and source revision `4528400b3ac206153b2963d4edd994f7448a3d71`. The fixture
source is unchanged in the integrated tree. The receiver wire definitions are
also unchanged. Spindle health gained the backward-compatible optional
`connection_limit` field.

During the receiver outage, the offline receipt remained locally visible.
The receiver recovered the same authority and the replayed receipt became
canonically visible with one exact row. The dedicated telemetry lane supplied
all four signal types from both services through source ID 57. Canonical
readback matched the exact fixed local prefix, and the isolated local collector
returned both producers' signals. The receiver logged one diagnostic for blocked
or dropped telemetry delivery. The unit reported a combined memory peak of
27 MiB and zero swap. This fixture measures receipt recovery and exact signal
delivery through a fixed prefix. Throughput and exporter loss rates require
separate measurements.

The rerun log SHA-256 is
`2fbc52e78e7e1ece0583443cf19e281277c6a944be17b86ac33a75e35c80442c`.
