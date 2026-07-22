# Python Surface

Python is a public API and packaging surface over the Rust engine, not a second semantic
implementation. `fabric-py` owns coarse PyO3 conversion and `python/fabrico11y/` owns typed DTO
ergonomics. Reducers, ordering, correction application, replay, catalog, integrity, and recovery
remain in Rust.

Build and clean-environment parity are maintained by `python scripts/verify_python_sdk.py`.
