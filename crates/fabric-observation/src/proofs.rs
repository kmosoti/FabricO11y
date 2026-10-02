//! Kani harnesses for the levels small enough to prove for every input.
//! They compile only under `cargo kani` (cfg(kani)); the registered check
//! runs the core's harnesses, not these, until this crate is added to it.

use crate::{bits, crc32, delta, varint, zigzag};
use alloc::vec::Vec;

#[kani::proof]
fn level0_packing_agrees_with_the_standard_library() {
    let v: u64 = kani::any();
    assert_eq!(bits::pack_le::<8>(v), v.to_le_bytes());
    assert_eq!(bits::unpack_le(&v.to_le_bytes()), v);
    let w: u32 = kani::any();
    assert_eq!(bits::pack_le::<4>(u64::from(w)), w.to_le_bytes());
    assert_eq!(bits::unpack_le(&w.to_le_bytes()), u64::from(w));
}

#[kani::proof]
fn level1_crc_table_is_the_definition_for_every_byte() {
    let b: u8 = kani::any();
    assert_eq!(crc32::hash(&[b]), crc32::bitwise(&[b]));
}

#[kani::proof]
fn level3_zigzag_is_a_bijection() {
    let v: i64 = kani::any();
    assert_eq!(zigzag::decode(zigzag::encode(v)), v);
    let u: u64 = kani::any();
    assert_eq!(zigzag::encode(zigzag::decode(u)), u);
}

#[kani::proof]
fn level4_step_inverts_for_every_base() {
    let a: u64 = kani::any();
    let b: u64 = kani::any();
    assert_eq!(delta::unstep(delta::step(a, b), b), a);
}

#[kani::proof]
#[kani::unwind(12)]
fn level2_varint_round_trips_every_u64() {
    let v: u64 = kani::any();
    let mut out = Vec::new();
    varint::put(&mut out, v);
    assert!(out.len() <= varint::MAX_LEN);
    assert_eq!(out.len(), varint::len(v));
    let mut cur = crate::bytes::Cursor::new(&out);
    assert_eq!(varint::get(&mut cur), Ok(v));
    assert_eq!(cur.remaining(), 0);
}
