//! Level 6: typed cells.
//!
//! The small values a record is made of, each with one byte form: a number
//! (integer or finite double), an attribute value, a sorted attribute list,
//! and the enumerations of the signal payloads. Doubles travel as their
//! bits, compared as bits, so that a cell is a value with one encoding;
//! NaN is refused because it has many bit patterns and no order. Attribute
//! lists are strictly sorted by key, which gives a set of attributes one
//! list and makes duplicate keys impossible.

use crate::bits::pack_le;
use crate::bytes::{Cursor, DecodeError};
use crate::dictionary::Lookup;
use crate::{varint, zigzag};
use alloc::borrow::ToOwned;
use alloc::string::String;
use alloc::vec::Vec;

/// An `f64` held by its bits, so that cells are `Eq` and the encoding is one-to-one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bits(pub u64);

impl Bits {
    pub fn from_f64(v: f64) -> Self {
        Bits(v.to_bits())
    }
    pub fn to_f64(self) -> f64 {
        f64::from_bits(self.0)
    }
    pub fn is_nan(self) -> bool {
        self.to_f64().is_nan()
    }
}

/// A number that compares by its bits.
#[derive(Clone, Copy, Debug)]
pub enum Number {
    Int(i64),
    Double(f64),
}

impl PartialEq for Number {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Number::Int(a), Number::Int(b)) => a == b,
            (Number::Double(a), Number::Double(b)) => a.to_bits() == b.to_bits(),
            _ => false,
        }
    }
}
impl Eq for Number {}

/// An attribute value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Str(String),
    Int(i64),
    Double(Bits),
    Bool(bool),
    Bytes(Vec<u8>),
}

/// An attribute value borrowed from a block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueRef<'a> {
    Str(&'a str),
    Int(i64),
    Double(Bits),
    Bool(bool),
    Bytes(&'a [u8]),
}

impl ValueRef<'_> {
    pub fn to_owned(self) -> Value {
        match self {
            ValueRef::Str(s) => Value::Str(s.to_owned()),
            ValueRef::Int(i) => Value::Int(i),
            ValueRef::Double(b) => Value::Double(b),
            ValueRef::Bool(b) => Value::Bool(b),
            ValueRef::Bytes(b) => Value::Bytes(b.to_vec()),
        }
    }
}

const NUMBER_INT: u8 = 0;
const NUMBER_DOUBLE: u8 = 1;
const VALUE_STR: u8 = 0;
const VALUE_INT: u8 = 1;
const VALUE_DOUBLE: u8 = 2;
const VALUE_FALSE: u8 = 3;
const VALUE_TRUE: u8 = 4;
const VALUE_BYTES: u8 = 5;

/// A finite double's bits, or the reason it has no cell.
pub fn check_double(bits: Bits) -> Result<(), &'static str> {
    if bits.is_nan() {
        Err("NaN is not a value")
    } else {
        Ok(())
    }
}

/// Strictly sorted by key: a set of attributes has one list.
pub fn check_attributes(attributes: &[(String, Value)]) -> Result<(), &'static str> {
    for pair in attributes.windows(2) {
        if let [(a, _), (b, _)] = pair
            && a >= b
        {
            return Err("attributes must be strictly sorted by key");
        }
    }
    for (_, value) in attributes {
        if let Value::Double(bits) = value {
            check_double(*bits)?;
        }
    }
    Ok(())
}

pub fn put_number(out: &mut Vec<u8>, n: Number) {
    match n {
        Number::Int(i) => {
            out.push(NUMBER_INT);
            varint::put(out, zigzag::encode(i));
        }
        Number::Double(d) => {
            out.push(NUMBER_DOUBLE);
            out.extend_from_slice(&pack_le::<8>(d.to_bits()));
        }
    }
}

pub fn get_number(cur: &mut Cursor<'_>) -> Result<Number, DecodeError> {
    let at = cur.position();
    match cur.byte()? {
        NUMBER_INT => Ok(Number::Int(zigzag::decode(varint::get(cur)?))),
        NUMBER_DOUBLE => {
            let bits = Bits(cur.u64_le()?);
            match check_double(bits) {
                Ok(()) => Ok(Number::Double(bits.to_f64())),
                Err(reason) => cur.fail_at(at.saturating_add(1), reason),
            }
        }
        _ => cur.fail_at(at, "unknown number tag"),
    }
}

/// Writes one attribute list; strings go through the dictionary.
pub fn put_attributes(
    out: &mut Vec<u8>,
    attributes: &[(String, Value)],
    strings: &mut crate::dictionary::Intern<String>,
) {
    varint::put(out, attributes.len() as u64);
    for (key, value) in attributes {
        varint::put(out, strings.id(key));
        match value {
            Value::Str(s) => {
                out.push(VALUE_STR);
                varint::put(out, strings.id(s));
            }
            Value::Int(i) => {
                out.push(VALUE_INT);
                varint::put(out, zigzag::encode(*i));
            }
            Value::Double(b) => {
                out.push(VALUE_DOUBLE);
                out.extend_from_slice(&pack_le::<8>(b.0));
            }
            Value::Bool(false) => out.push(VALUE_FALSE),
            Value::Bool(true) => out.push(VALUE_TRUE),
            Value::Bytes(b) => {
                out.push(VALUE_BYTES);
                varint::put_bytes(out, b);
            }
        }
    }
}

/// Reads one attribute list with keys and string values borrowed from the
/// block's dictionary (itself borrowed from the input).
pub fn get_attributes<'a>(
    cur: &mut Cursor<'a>,
    strings: &mut Lookup<&'a str>,
) -> Result<Vec<(&'a str, ValueRef<'a>)>, DecodeError> {
    let count = varint::count(cur, 2)?;
    let mut list: Vec<(&'a str, ValueRef<'a>)> = Vec::with_capacity(count);
    for _ in 0..count {
        let key_at = cur.position();
        let id = varint::get(cur)?;
        let key = strings.get(cur, key_at, id)?;
        if list.last().is_some_and(|(k, _)| *k >= key) {
            return cur.fail_at(key_at, "attributes must be strictly sorted by key");
        }
        let tag_at = cur.position();
        let value = match cur.byte()? {
            VALUE_STR => {
                let at = cur.position();
                let id = varint::get(cur)?;
                ValueRef::Str(strings.get(cur, at, id)?)
            }
            VALUE_INT => ValueRef::Int(zigzag::decode(varint::get(cur)?)),
            VALUE_DOUBLE => {
                let at = cur.position();
                let bits = Bits(cur.u64_le()?);
                if let Err(reason) = check_double(bits) {
                    return cur.fail_at(at, reason);
                }
                ValueRef::Double(bits)
            }
            VALUE_FALSE => ValueRef::Bool(false),
            VALUE_TRUE => ValueRef::Bool(true),
            VALUE_BYTES => ValueRef::Bytes(varint::get_bytes(cur)?),
            _ => return cur.fail_at(tag_at, "unknown value tag"),
        };
        list.push((key, value));
    }
    Ok(list)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionary::Intern;
    use alloc::string::ToString;

    #[test]
    fn numbers_round_trip_and_nan_is_refused() {
        for n in [
            Number::Int(-5),
            Number::Int(i64::MAX),
            Number::Double(-0.0),
            Number::Double(f64::INFINITY),
        ] {
            let mut out = Vec::new();
            put_number(&mut out, n);
            assert_eq!(get_number(&mut Cursor::new(&out)).unwrap(), n);
        }
        let mut out = Vec::new();
        put_number(&mut out, Number::Double(f64::NAN));
        assert_eq!(
            get_number(&mut Cursor::new(&out)).unwrap_err().reason,
            "NaN is not a value"
        );
        assert_eq!(
            get_number(&mut Cursor::new(&[9])).unwrap_err().reason,
            "unknown number tag"
        );
    }

    #[test]
    fn attribute_lists_round_trip_through_a_dictionary() {
        let attrs = vec![
            ("a".to_string(), Value::Bytes(vec![1, 2])),
            ("b".to_string(), Value::Str("a".into())),
            ("c".to_string(), Value::Double(Bits::from_f64(2.5))),
            ("d".to_string(), Value::Bool(true)),
            ("e".to_string(), Value::Int(-1)),
        ];
        check_attributes(&attrs).unwrap();
        let mut strings = Intern::new();
        let mut out = Vec::new();
        put_attributes(&mut out, &attrs, &mut strings);
        assert_eq!(
            strings.table(),
            &["a", "b", "c", "d", "e"],
            "the value \"a\" reuses the key's entry"
        );
        let mut cur = Cursor::new(&out);
        let table: Vec<&str> = strings.table().iter().map(String::as_str).collect();
        let mut lookup = Lookup::new(&mut cur, table, &[0; 5], "dup").unwrap();
        let got: Vec<(String, Value)> = get_attributes(&mut cur, &mut lookup)
            .unwrap()
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        assert_eq!(got, attrs);
        assert!(lookup.all_used());
    }

    #[test]
    fn unsorted_and_duplicate_keys_have_no_cell() {
        let bad = vec![
            ("b".to_string(), Value::Int(1)),
            ("a".to_string(), Value::Int(2)),
        ];
        assert!(check_attributes(&bad).is_err());
        let dup = vec![
            ("a".to_string(), Value::Int(1)),
            ("a".to_string(), Value::Int(2)),
        ];
        assert!(check_attributes(&dup).is_err());
        let nan = vec![("a".to_string(), Value::Double(Bits::from_f64(f64::NAN)))];
        assert_eq!(check_attributes(&nan).unwrap_err(), "NaN is not a value");
    }
}
