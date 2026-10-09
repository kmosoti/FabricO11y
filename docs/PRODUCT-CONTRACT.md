# Product contract

This is the highest-priority source of truth for what FabricO11y promises (see the [source-of-truth hierarchy](README.md#source-of-truth)). It states intended behavior. Whether a promise is implemented, tested, measured or qualified is recorded in the [qualification ledger](QUALIFICATION.md#capability-ledger) and the [verification matrix](formal/verification-matrix.md); nothing here is a release claim. The text below migrated from the release-stage contract at base `9b3a2b4` (`docs/ALPHA.md`); numeric targets, defaults and gate requirements are unchanged.

## Thesis

FabricO11y answers: *what did reality tell us, what happened to that evidence, and how much of it can we truthfully claim to know?* It collects observations, preserves their meaning and custody across failures, retains them under explicit policy, and answers queries without hiding missing coverage or uncertainty.

```text
Linux reality -> Spindle observes -> Batch on a Strand -> Spool retains custody
  -> Delivery transfers custody -> Server commits -> Journal / Segment retain evidence
  -> Query interprets retained evidence -> completeness and freshness stay explicit
```

Every promise below belongs to one of five correctness dimensions:

| Dimension | Contract question |
| --- | --- |
| Fidelity | Were identities, values, units, timestamps, attributes and signal semantics preserved? |
| Custody | Who is responsible for telemetry before and after durable acknowledgment? |
| Completeness | What evidence was examined, excluded, unavailable, expired or missing? |
| Freshness | How current is the queryable evidence, and how was that measured? |
| Boundedness | What bounds memory, queues, disk, retries, runtime and evidence output? |

## Product boundary

The first target profile is one operator-controlled x86_64 Linux installation of a Debian-family distribution with glibc 2.34 or newer and systemd 249 or newer (Debian 12 and 13, Ubuntu 22.04 and 24.04, and distributions derived from them, natively or under WSL2): outbound Spindles (the `fabric-node` executable) collecting CPU, memory, filesystem, disk, network and configured newline-delimited logs, and receiving OpenTelemetry traces that local applications export to the Spindle's loopback OTLP/HTTP endpoint ([ADR-0025](decisions/ADR-0025-carry-traces-as-a-third-signal.md)); one Fabric Server for authenticated delivery, durable storage, query and control; `fabricctl` and an HTTP API. Spindles retain the last valid configuration during disconnection. There is no external Collector requirement, general OTLP receiver (the Spindle's trace endpoint accepts only OTLP/HTTP protobuf trace exports on loopback), arbitrary remote compute, UI, or scale claim beyond the [qualification gates](QUALIFICATION.md). Real-server qualification follows local qualification. The release that first targets this profile is tagged, not named in the architecture ([ADR-0019](decisions/ADR-0019-keep-release-maturity-in-tags.md)).

## Fidelity: the envelope and telemetry semantics

The Fabric delivery envelope is versioned and identifies `(Spindle identity, generation, batch sequence)` — the Strand and a position on it ([ADR-0017](decisions/ADR-0017-name-the-spindle-and-the-strand.md)) — plus exact encoded OpenTelemetry metrics, logs and traces payload bytes. Preserve units, resource attributes, event and observation times when known, cumulative counter start/reset semantics, span identities (trace, span and parent span IDs), span start and end times, kind and status, and source cursor. A trace export is acknowledged to the exporting application only after its bytes are committed to the Spool, under the same custody rules as a collected Batch. The local `FAB1` Spool pins `opentelemetry-proto = 0.33.0` and `prost = 0.14.4` in [Cargo.toml](../Cargo.toml); the envelope lives in [crates/fabric-frame/src/envelope.rs](../crates/fabric-frame/src/envelope.rs). This does not qualify a general OTLP receiver. The [OTLP specification](https://opentelemetry.io/docs/specs/otlp/), [metric data model](https://opentelemetry.io/docs/specs/otel/metrics/data-model/), [log data model](https://opentelemetry.io/docs/specs/otel/logs/data-model/) and [trace API](https://opentelemetry.io/docs/specs/otel/trace/api/) define telemetry semantics. A homegrown JSON document is not OTLP.

Compatibility guardrails: `FOL2` and `FAB1` framing, magic values, protobuf field numbers, Batch byte encoding, Strand identity, sequence, ACK, deduplication and retry semantics, journal sync ordering, checkpoint format, query ordering, rate, pagination and retention semantics do not change without an explicit contract change, an ADR if architectural, a migration plan and new verification. A type move is not permission to change bytes.

## Custody and safety contract

An unacknowledged Batch remains in the Spindle's Spool. Identity and encoded bytes must be durable before first send; retries reuse both. The server syncs Batch data and deduplication state before ACK. On one Strand, equal sequence and equal bytes is one logical commit; equal sequence and different bytes is an error; equal content at another sequence is a distinct Batch ([ADR-0013](decisions/ADR-0013-deliver-batches-in-order-with-bounded-dedup.md)). Journal Batch commit uses data sync followed by marker sync, with FOL2 readable unchanged. A write or sync error quarantines that writer and records recovery-required; readable bytes alone cannot clear a reported error. Recovery from a known failed journal requires an independent retained source. A process that dies during an append without a reported error leaves an interrupted-append marker; reopen keeps only frames with a valid commit marker ([ADR-0011](decisions/ADR-0011-separate-interrupted-append-from-known-failure.md)). An ACK is conditional on successful sync calls and documented filesystem assumptions; physical power loss is untested.

Log cursor movement and collected lines commit together. Rename, truncation, incomplete final line, oversized line, source failure, Spool full and process restart have explicit outcomes and tests.

## Completeness contract

A full Spool retains unacknowledged records, refuses new durable collection and exposes a collection gap or unknown interval; missed metrics before collection are never represented as delivered. A server publishes immutable compressed Segments before reclaiming journal coverage. Missing or corrupt optional indexes trigger an exact scan; missing or corrupt raw Segments mark the query incomplete. Query responses distinguish scan completeness, Spindle freshness, collection gaps and retained window. Pagination fixes a snapshot. The query contract and envelope are specified in [retained history and query](architecture/retained-history.md).

## Freshness contract

Every query answer reports, per Spindle with any retained record, the newest observation, point or span start time retained, so a caller can tell a quiet Spindle from a stale one. The registered freshness target (observation to queryable, p99) is in the [decision rule](QUALIFICATION.md#registered-workload-and-decision-rule).

## Boundedness contract

All active queues are byte bounded. Defaults: 15 s metrics, 256 MiB Spindle Spool, server retention of at most 24 h and at most 20 GiB of sealed Segments with the actual retained boundary reported. Test overrides must stay inside the aggregate test budget. Collection limits (log paths, lines, bytes per pass, devices, gap counts) are listed in the [Spindle view](architecture/spindle.md).

## Control and security contract

TLS protects Spindle and admin traffic. Spindle credentials are revocable and distinct from admin credentials. A configuration has desired and applied revisions, validates before activation, obeys local permissions and resource ceilings, and can be paused ([ADR-0014](decisions/ADR-0014-manage-nodes-through-server-control-state.md)).

## Linux installation contract

Every production `fabric-server serve` invocation owns one dedicated local
Spindle process with its own persistent identity, node credential and bounded
Spool. Its destination is that server's HTTPS listener; TLS verification remains
enabled. Server and Spindle operational diagnostics are bounded local files,
automatically collected through the ordinary Spool/ACK path. Emit periodic
process samples and state transitions, not a new self-log per collected or
delivered Batch. Diagnostics are best effort before Spool commit: rotation or
unavailable collection must not be presented as complete evidence. A Spindle
collects its own diagnostics; it does not spawn another Spindle. Edge Spindles
keep their configured destination.

The production CLI supervises and reaps its companion and stops serving if that
process exits unexpectedly. The library serving primitive remains available for
embedded composition and isolated tests; it does not implicitly launch processes.
The companion inherits its server service's cgroup; existing numeric service and
aggregate limits below are not increased. The server unit's bound covers the
combined server and companion. Operational logs retain at most two 256 KiB files
per process; a managed companion uses a 64 MiB Spool and 64 KiB/s output cap.
Its local diagnostics and the server diagnostic file consume two of the existing
sixteen log-source slots. Automatic self-observation is a capability to verify,
not a new qualification claim.

Status: packaging exists and passes static checks; the running-installation acceptance ran in a Debian 13 systemd container and is inconclusive, because `MemoryHigh` enforcement needs a unified cgroup hierarchy ([ledger](QUALIFICATION.md#capability-ledger)).

Ship only `fabrico11y-node.service`, `fabrico11y-server.service`, `fabricctl`, and one `system-fabrico11y.slice` for their aggregate resource bound. The slice name deliberately places it beneath `system.slice`; both services set `Slice=system-fabrico11y.slice`. The package installs `/usr/lib/sysusers.d/fabrico11y.conf` with `g fabricolly -` followed by `u! fabricolly -:fabricolly "Fabric O11y service" - /usr/sbin/nologin`. The static system user **and primary group** are exactly `fabricolly`. Both units explicitly set `User=fabricolly` and `Group=fabricolly`. Package installation must inspect an existing account/group for the expected non-login service identity and primary group, refusing a collision instead of silently commandeering it. Vendor files live under `/usr/lib`; administrator overrides live under `/etc`. The Rust daemons never create accounts, chown installation paths, or manage cgroups. [Debian's systemd 257 sysusers documentation](https://manpages.debian.org/trixie/systemd/sysusers.d.5.en.html) specifies these entries and the vendor/administrator precedence.

The units use `Type=exec` until the code actually sends `sd_notify(READY=1)`. The Spindle unit sets `StateDirectory=fabrico11y/node`, `RuntimeDirectory=fabrico11y/node`; the server uses `fabrico11y/server` for both. Each sets `StateDirectoryMode=0700`, `RuntimeDirectoryMode=0700`, `UMask=0077`. Install binaries, unit files and base configuration as root owned; provide protected credential files only as needed. Separate paths organize Spindle/server state, but the shared Unix principal does not isolate them from one another. Local log access is granted by explicit narrow ACLs; unreadable logs produce visible collection gaps, never root fallback or blanket `adm`/`systemd-journal` membership. OS service and limit changes remain operator systemd/package actions; Fabric remote configuration stays behind the admin HTTP credential. [systemd.exec](https://manpages.debian.org/trixie/systemd/systemd.exec.5.en.html) defines identity, managed directories and execution sandboxing; [systemd.service](https://manpages.debian.org/trixie/systemd/systemd.service.5.en.html) defines readiness semantics.

The services run without capabilities (`CapabilityBoundingSet=` and `AmbientCapabilities=` empty), with `NoNewPrivileges=yes`, `ProtectSystem=strict`, `ProtectHome=yes`, `PrivateTmp=yes`, `ProtectKernelModules=yes`, `ProtectKernelTunables=yes`, `ProtectControlGroups=yes`, `Delegate=no`, and `RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6`. Keep read visibility of host `/proc`, `/sys` and `/sys/fs/cgroup` required for host metrics. Do not hide those sources with `ProcSubset=pid`, `ProtectProc=invisible`, or `PrivateNetwork=yes`. systemd owns the unified cgroup hierarchy; daemons do not write cgroup files. No privileged helper, polkit broker, D-Bus broker, extra service, dynamic identity, cgroup manager or mandatory self-cgroup collector is part of this profile. [systemd.exec](https://manpages.debian.org/trixie/systemd/systemd.exec.5.en.html) documents these controls.

Frozen installed defaults: Spindle `MemoryHigh=128M`, `MemoryMax=256M`, `TasksMax=128`; server `MemoryHigh=2560M`, `MemoryMax=3072M`, `TasksMax=512`; aggregate slice `MemoryHigh=2816M`, `MemoryMax=3328M`, `TasksMax=640`. Each sets `CPUWeight=100`, `IOWeight=100`, `MemoryAccounting=yes`, `TasksAccounting=yes`, `IOAccounting=yes`; CPU accounting is supplied by the unified hierarchy. These weights are relative shares, not a CPU quota. The 64 MiB native-Spindle and 2 GiB central **RSS** qualification gates remain separate from cgroup memory, which includes page cache and kernel charges. Report actual controller availability, cgroup placement, `EffectiveMemoryHigh`/`EffectiveMemoryMax`, tasks and accounting; an unavailable or ineffective I/O controller on WSL is reported, never treated as measured enforcement. [systemd resource control](https://manpages.debian.org/trixie/systemd/systemd.resource-control.5.en.html) defines hierarchical effective limits, relative weights and the high/max distinction. If these numeric defaults fail the registered workload, changing them requires a recorded contract revision and a fresh qualification run.

Installation acceptance must exercise sysusers dry run and temporary root, repeated install and existing-account collision refusal, `systemd-analyze verify`, actual non-root UID/primary GID, managed directory ownership/modes, cgroup placement/effective limits/accounting, metric reads and one explicitly authorized log inside the sandbox, visible gaps for denied logs, TLS, graceful shutdown/restart with retained Spool, memory/task pressure containment and uninstall without deleting user data unless explicitly purged. Use disposable test services/hosts and avoid unrelated host accounts or units. Static unit parsing alone cannot pass installation acceptance; if a running install test is unavailable, record it unrun. [ADR-0010](decisions/ADR-0010-use-static-systemd-services-for-alpha.md) records the service boundary.

## Non-goals of the first profile

A general OTLP receiver, a UI, SQLite, `rcgen`, an async runtime in the Spindle, inotify watchers, plugin boundaries, dynamic users, polkit helpers, and any index beyond Parquet row-group statistics and the per-row-group trigram filter of [ADR-0024](decisions/ADR-0024-answer-history-queries-by-a-walk-over-source-bounds.md) (part 2), unless a registered gate fails. The trigram filter is an optional index under the rule above: a missing or corrupt filter triggers an exact scan and never changes an answer. Every feature or configuration value must have a current consumer and a test.

## Changing this contract

A change to this page, to an [independent oracle](QUALIFICATION.md#tooling-classes), to a negative control's expected outcome, to a formal invariant or to verification policy is a trust-boundary change: it is made in its own commit, says why, and is not mixed with ordinary implementation work ([ADR-0018](decisions/ADR-0018-accept-work-on-executable-evidence.md)).
