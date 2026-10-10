# ADR-0025: Carry traces as a third signal

## Status

Accepted on 2026-10-03 at the repository owner's request ("include support for traces"), with the [product contract](../PRODUCT-CONTRACT.md) amended in its own commit. Implementation follows in separate commits, each with its tests.

## Context

Fabric carried host metrics and file logs. The contract excluded traces and trace intake. The owner asked for traces, for throughput this machine can sustain, and for any Debian-based distribution. The record model of [ADR-0023](ADR-0023-define-an-observation-record-with-a-canonical-encoding.md) already has a span signal with trace and span identities; the query walk of [ADR-0024](ADR-0024-answer-history-queries-by-a-walk-over-source-bounds.md) and its trigram filter already serve any time-ordered table.

## Decision

**Intake.** A Spindle in `run` mode may listen on a loopback address (`traces_listen`, for example `127.0.0.1:4318`) for OTLP/HTTP `POST /v1/traces` with `application/x-protobuf` bodies up to 1 MiB. It decodes the body as an `ExportTraceServiceRequest` to validate it, commits the exact bytes to the Spool as a Batch, and only then answers `200`; a full Spool answers `503` and a malformed or oversized body `400` or `413`. Listening on any non-loopback address is refused at configuration load. Requests waiting for one commit are concatenated into one Batch (concatenated protobuf encodings of a message are its merge), up to the envelope cap. The endpoint is served by one blocking thread; there is no async runtime in the Spindle.

**Envelope.** `Batch` gains field 9, `traces`: the exact encoded `ExportTraceServiceRequest`. The version stays 1: field 9 is additive, a version-1 decoder that predates it ignores it, and the Batch's identity is still its exact bytes. The 1 MiB cap applies to metrics, logs and traces together.

**Rows and storage.** One span row per OTLP `Span` in request order: `node`, `node_id`, `sequence`, `index` (position among the Batch's spans), `trace_id` (hex), `span_id` (hex), `parent_span_id` (hex, empty for a root), `name`, `kind` and `status` (the OTLP integers), `start_ns`, `end_ns`, and string attributes. A Segment gains `spans.parquet`, sorted by `start_ns`, node identity, sequence and index, with a trace-id filter per row group: the trigram filter of ADR-0024 part 2 over the hex trace IDs (`spans_filter.bin`), which rejects an absent 32-character ID with near certainty. Both files are listed in the version-1 manifest like the others; a Segment without them holds no spans.

**Query.** `{"kind":"spans","node":N?,"from_ns":A,"to_ns":B,"trace_id":T?,"name":S?,"limit":L,"page":P?}` returns span rows with `start_ns` in `[A, B)`, from node `N`, with trace ID `T` and name `S` exactly when given, in the order above, paginated like logs. Freshness counts span start times. Both query plans answer it; the walk orders tail entries, tail blocks and row groups by their lowest start time and skips row groups and blocks whose filter lacks the trace ID.

**Throughput.** The Spindle's per-pass collection limits rise (8,192 lines and 1 MiB per file, 768 KiB of bodies per Batch, encoding overhead counted against the cap) and a pass that leaves a backlog is followed by another once delivery catches up. The server seals journal files on several threads; Segment publication may complete out of order, while journal reclamation proceeds strictly oldest first.

Implementation clarification: the earlier wording, “committing Segments in journal order,” conflated publication with reclaim. At `5f0c52f`, each worker calls `segment::build` independently and the sealer later reclaims in label order. The [journal-reclaim investigation](../milestones/journal-reclaim-progress.md) changes when the contiguous published prefix is reclaimed, preserving the existing publication, checkpoint and deletion semantics.

**Distribution.** The package declares its dependencies from the binaries (`dpkg-shlibdeps`) and the oldest systemd whose features the units use, and a check fails if a binary needs glibc newer than 2.34.

## Consequences

Traces enter under the same custody, completeness, freshness and retention rules as logs. The independent query oracle learns spans before the server answers them (a trust-boundary change in its own commit). A trace lookup by ID costs the filter reads plus the groups that may hold the ID, not a scan.

## Alternatives considered

- **A general OTLP receiver on the server.** Rejected: it would bypass the Spindle's Spool and its custody rules and expose an unauthenticated ingest surface.
- **A gRPC endpoint.** Rejected for now: it needs HTTP/2 and in practice an async runtime; OTLP/HTTP protobuf is supported by every OpenTelemetry SDK.
- **A separate trace store with a trace-ID index.** Rejected: a per-row-group filter over hex IDs reaches the same floor (groups that may hold the ID) at one byte per row or so, reusing ADR-0024's mechanism.
