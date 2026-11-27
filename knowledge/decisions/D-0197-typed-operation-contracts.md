# Typed operation contracts

`fabric-ops` uses a bounded checked static inventory and typed operation
bindings. The inventory validates names, versions and duplicate identities;
negotiation checks the protocol and operation versions separately and verifies
the binding's entire descriptor, including lifecycle shape. A successful token
retains concrete request and response types. Adapters authenticate callers and
choose dispatch and wire formats; registry metadata is not authority.

The query inventory maps the seven `fabric::query::Operation` names and its
existing `QueryRequest` / `QueryResponse` API version 1. The inventory's
operation version is local contract metadata: current query wire requests have
only the top-level version. Canonical `Request` / `Response` and Spindle
`Request` / `Reply` are separate command families without a per-operation
version field; `fabric.cli.v1` is a versioned output envelope rather than a
request protocol. Adapters may bind those concrete types when their version
metadata is defined, without changing the existing wire formats.

The four normalized outcome categories carry no strings, payloads or
identities. Retry is conditional on the operation's replay contract;
reconciliation requires checking authoritative state; expiry names an elapsed
budget or lease and says nothing about external effects; unknown outcome does
not establish success or failure. This
seam does not map current transport error strings or execute retries.

A compile-time-only list would not detect duplicate identities supplied by an
adapter. Runtime dynamic registration would require ownership, allocation and
mutation semantics beyond this contract. A checked borrowed static inventory
bounds validation and keeps runtime/erasure decisions for the adapter.
