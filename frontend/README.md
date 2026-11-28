# Portable application state

This separate Cargo workspace contains `fabric-app-core`, a Sans-I/O reducer
for the existing `ListRuns` and `RunSummary` query contracts. The same reducer
can open a typed run detail at the list snapshot. It has no renderer or transport.
The adapter authenticates the backend, supplies the expected authority, and
bounds incoming bytes before constructing a response event. An expected
identity or returned coverage field is not an authorization grant.

The reducer accepts at most 256 retained run rows, 32 pages, 256 KiB per
response, and one million request generations per state instance. Each
transition emits at most one effect and a bounded change signal. Errors leave
state unchanged. The reducer checks response bytes before JSON parsing,
validates returned authority, scope and fixed snapshot, and decodes typed
`RunSummary` rows before changing state. A page retains its own returned
coverage, including gaps and completeness. An adapter must also enforce the
wire byte limit before allocating the event.

A request can be cancelled. Disconnection while a read is in flight reports
an unknown outcome; a reconnect starts a fresh explicit read. Responses for an
inactive or older generation are ignored. These states do not imply custody,
visibility, successful cancellation, or any external mutation.

Run native checks with `cargo test --locked --manifest-path frontend/Cargo.toml`.
The `parity` package is test-only and not a default member. Its recorded
fixture can be executed natively and as a `wasm32-unknown-unknown` cdylib via
`parity/wasm-fixture.cjs`; compare the two exact outputs using Node. This
harness is not a browser or frontend package qualification.
