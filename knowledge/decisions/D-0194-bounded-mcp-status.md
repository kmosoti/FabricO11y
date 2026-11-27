---
id: D-0194
title: Bounded MCP status scheduling
added_by: agent
human_ratified: false
date: 2026-11-27
---

The stdio adapter used one fail-fast work slot for all reads. An in-flight
capabilities call could therefore reject a pipelined progress call, and a query
could prevent clients from reading receiver status.

Capabilities reads receiver metadata, and progress reads the in-memory custody
frontiers. Put those calls behind a bounded FIFO with one active exchange.
The queue serves admitted requests in the order they start waiting for the
worker. A single deadline limits each request's time in the queue and canonical
transport. Keeping a separate cancellation slot allows cancellation while both
the analytical exchange and the status queue are occupied.

Planning reserves the receiver's query slot. Evidence retrieval can wait for
storage and return a full observation. Adding either to the status queue would
delay the calls clients use to inspect an active investigation. Both continue to
share the query slot. The adapter releases that slot when the blocking
exchange ends, even if the query was cancelled earlier.

Revisit scheduling if short status operations acquire storage dependencies or
measurements show that status timeouts routinely occur under ordinary polling.
See the [scheduling contract](../../docs/architecture.md#mcp-request-scheduling).
