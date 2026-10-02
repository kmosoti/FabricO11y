//! Properties of each level of the observation codec's tower, stated from
//! the level's own contract. The block-level properties live in
//! `observation.rs`; these are the rungs beneath them.

use fabric_observation::bytes::Cursor;
use fabric_observation::delta::{SecondOrderDecoder, SecondOrderEncoder, step, unstep};
use fabric_observation::dictionary::{Intern, Lookup};
use fabric_observation::{varint, zigzag};
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(3_000))]

    /// Level 1: one byte string per value, the shortest, and the decoder reads exactly it.
    #[test]
    fn varint_round_trips_in_its_declared_length(v in any::<u64>()) {
        let mut out = Vec::new();
        varint::put(&mut out, v);
        prop_assert_eq!(out.len(), varint::len(v));
        prop_assert!(out.len() <= varint::MAX_LEN);
        let mut cur = Cursor::new(&out);
        prop_assert_eq!(varint::get(&mut cur), Ok(v));
        prop_assert_eq!(cur.remaining(), 0);
    }

    /// Level 1, canonicality: whatever the decoder accepts from arbitrary
    /// bytes is the shortest form of its value, and nothing panics.
    #[test]
    fn varint_accepts_only_the_shortest_form(bytes in proptest::collection::vec(any::<u8>(), 0..12)) {
        let mut cur = Cursor::new(&bytes);
        if let Ok(v) = varint::get(&mut cur) {
            let used = cur.position();
            let mut again = Vec::new();
            varint::put(&mut again, v);
            prop_assert_eq!(&bytes[..used], &again[..]);
        }
    }

    /// Level 2: a bijection on both sides.
    #[test]
    fn zigzag_is_a_bijection(v in any::<i64>(), u in any::<u64>()) {
        prop_assert_eq!(zigzag::decode(zigzag::encode(v)), v);
        prop_assert_eq!(zigzag::encode(zigzag::decode(u)), u);
    }

    /// Level 3: steps invert for every pair, and second-order sequences
    /// round trip whatever the values do, including wraps.
    #[test]
    fn delta_sequences_round_trip(a in any::<u64>(), b in any::<u64>(), series in proptest::collection::vec(any::<u64>(), 1..64)) {
        prop_assert_eq!(unstep(step(a, b), b), a);
        let mut enc = SecondOrderEncoder::new();
        let mut dec = SecondOrderDecoder::new();
        for v in &series {
            prop_assert_eq!(dec.value(enc.code(*v)), *v);
        }
    }

    /// Level 3: a regular series codes to zeros after its second element,
    /// whatever its start and interval.
    #[test]
    fn regular_series_cost_nothing_after_two(start in any::<u64>(), interval in any::<u64>(), n in 3_usize..40) {
        let mut enc = SecondOrderEncoder::new();
        let mut value = start;
        for i in 0..n {
            let code = enc.code(value);
            if i >= 2 {
                prop_assert_eq!(code, 0);
            }
            value = value.wrapping_add(interval);
        }
    }

    /// Level 4: interning any sequence gives a table of distinct entries in
    /// first-use order, and replaying the ids through a lookup accepts them
    /// all, uses every entry, and yields the sequence.
    #[test]
    fn dictionary_round_trips_in_first_use_order(values in proptest::collection::vec(0_u8..6, 1..50)) {
        let mut intern = Intern::new();
        let ids: Vec<u64> = values.iter().map(|v| intern.id(v)).collect();
        let table = intern.table().to_vec();
        let mut sorted = table.clone();
        sorted.sort_unstable();
        sorted.dedup();
        prop_assert_eq!(sorted.len(), table.len(), "entries are distinct");
        let data = [0_u8; 1];
        let mut cur = Cursor::new(&data);
        let mut lookup = Lookup::new(&mut cur, table, &[], "dup").unwrap();
        let back: Vec<u8> = ids.iter().map(|id| lookup.get(&mut cur, 0, *id).unwrap()).collect();
        prop_assert_eq!(back, values);
        prop_assert!(lookup.all_used());
    }

    /// Level 4, canonicality: a permutation of the ids that references an
    /// entry early is refused; a table with a repeated entry is refused.
    #[test]
    fn dictionary_refuses_early_references_and_repeats(n in 2_usize..8) {
        let table: Vec<u8> = (0..n as u8).collect();
        let data = [0_u8; 1];
        let mut cur = Cursor::new(&data);
        let mut lookup = Lookup::new(&mut cur, table.clone(), &[], "dup").unwrap();
        prop_assert!(lookup.get(&mut cur, 0, 1).is_err(), "id 1 before id 0");
        let mut repeated = table;
        repeated.push(0);
        prop_assert!(Lookup::new(&mut cur, repeated, &[], "dup").is_err());
    }
}

mod floor {
    //! Levels 0 and 1: the shifts agree with the standard library for every
    //! value tested, and the table-driven CRC is the bit-serial definition.
    use fabric_observation::{bits, crc32};
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(3_000))]

        #[test]
        fn packing_agrees_with_the_standard_library(v in any::<u64>(), w in any::<u32>()) {
            prop_assert_eq!(bits::pack_le::<8>(v), v.to_le_bytes());
            prop_assert_eq!(bits::unpack_le(&v.to_le_bytes()), v);
            prop_assert_eq!(bits::pack_le::<4>(u64::from(w)), w.to_le_bytes());
            prop_assert_eq!(bits::unpack_le(&w.to_le_bytes()), u64::from(w));
            prop_assert_eq!(u64::from(bits::lo7(v)), v & 0x7f);
            prop_assert_eq!(bits::shr7(v), v >> 7);
        }

        #[test]
        fn crc_table_equals_the_definition(data in proptest::collection::vec(any::<u8>(), 0..300)) {
            prop_assert_eq!(crc32::hash(&data), crc32::bitwise(&data));
        }

        #[test]
        fn crc_detects_any_single_bit_flip(data in proptest::collection::vec(any::<u8>(), 1..64), at in any::<usize>(), bit in 0_u8..8) {
            let mut m = data.clone();
            let i = at % m.len();
            m[i] ^= 1 << bit;
            prop_assert_ne!(crc32::hash(&m), crc32::hash(&data));
        }
    }
}
