//! Kani harnesses for the arithmetic the codec's canonicality rests on. They
//! compile only under `cargo kani` (cfg(kani)); the registered check runs the
//! core's harnesses, not these, until this crate is added to it.

use super::{delta, undelta, unzigzag, zigzag};

#[kani::proof]
fn zigzag_is_a_bijection() {
    let v: i64 = kani::any();
    assert_eq!(unzigzag(zigzag(v)), v);
    let u: u64 = kani::any();
    assert_eq!(zigzag(unzigzag(u)), u);
}

#[kani::proof]
fn delta_inverts_for_every_base() {
    let a: u64 = kani::any();
    let b: u64 = kani::any();
    assert_eq!(undelta(delta(a, b), b), a);
}
