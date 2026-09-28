//! Application use cases. Each calls pure decisions in `fabric-core` and
//! performs effects only through `fabric-ports`; none depends on a concrete
//! adapter (checked by `cargo xtask check-layers`).

pub mod delivery;
