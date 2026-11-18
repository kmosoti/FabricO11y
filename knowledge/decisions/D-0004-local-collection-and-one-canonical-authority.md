---
id: D-0004
title: Local Collection and One Canonical Authority
type: decision
status: accepted
date: 2026-11-18
added_by: agent
human_ratified: false
related:
  - D-0003
---

# D-0004 · Local Collection and One Canonical Authority

Retain independent local custody and projection on each host. A multi-host
deployment sends selected observations from multiple Spindle origins to one
canonical authority via authenticated loopback tunnels. Receivers authenticate
each origin separately and readers request explicit per-origin coverage.

Authority, origin, and source ID together identify a forwarded record across
deployments. The origin's local custody frontier, the receiver's custody
frontier, and either projection's visibility answer different questions.
Source-fenced queries report the coverage they actually observed. They cannot
establish which additional records an unreachable host accepted locally.

A single authority allows fleet investigations to reuse existing origin fences,
receiver credentials, and canonical recovery. Federation would also need
explicit cross-authority coverage, query fan-out limits, and a cursor contract.
Native cross-host listeners would add a transport security boundary. The
existing loopback tunnel deployment supplies that boundary independently.

Shadow forwarding preserves local payloads while operators qualify exact
readback. Reclamation increases dependence on the receiver because reclaimed
records require canonical access for replay. Keep reclamation disabled until the
deployment is qualified and budgets allow the intended outage window.

See [single-host and distributed deployment](../../docs/architecture.md#4-single-host-and-distributed-deployment)
and [canonical custody](../../docs/canonical-custody.md).
