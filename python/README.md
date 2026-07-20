# Python Surface

Python is a public API and packaging surface over the Rust engine, not a second semantic
implementation.

FO05 will add:

- `crates/fabric-py/` for coarse PyO3 conversion and exposure;
- `python/fabrico11y/` for typed Python-facing APIs and errors;
- parity tests that execute the same operations through Rust and Python;
- maturin packaging configuration.

Reducers, ordering, correction application, replay semantics, and answer construction remain in
Rust. The Python layer may validate ergonomic input shape, but normative validation belongs to the
shared contract boundary.
