//! Bounded JSON without duplicate-key or non-finite-number ambiguity.
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};
use std::fmt;

pub const JSON_LIMIT: usize = 4 * 1024 * 1024;
pub const JSON_TOTAL_LIMIT: u64 = 64 * 1024 * 1024;

/// A container at depth 32 may contain scalars, but no further container.
struct Strict(usize);

impl<'de> DeserializeSeed<'de> for Strict {
    type Value = Value;

    fn deserialize<D: de::Deserializer<'de>>(self, decoder: D) -> Result<Value, D::Error> {
        decoder.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Strict {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("bounded JSON with unique object keys")
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Value, E> {
        Ok(Value::Bool(value))
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Value, E> {
        Ok(Value::Number(value.into()))
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Value, E> {
        Ok(Value::Number(value.into()))
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Value, E> {
        Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Value, E> {
        Ok(Value::String(value.to_owned()))
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<Value, E> {
        Ok(Value::String(value))
    }

    fn visit_unit<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Value, A::Error> {
        if self.0 >= 32 {
            return Err(de::Error::custom("JSON depth exceeds 32"));
        }
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element_seed(Strict(self.0 + 1))? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut object: A) -> Result<Value, A::Error> {
        if self.0 >= 32 {
            return Err(de::Error::custom("JSON depth exceeds 32"));
        }
        let mut values = Map::new();
        while let Some(key) = object.next_key::<String>()? {
            if values.contains_key(&key) {
                return Err(de::Error::custom("duplicate JSON key"));
            }
            values.insert(key, object.next_value_seed(Strict(self.0 + 1))?);
        }
        Ok(Value::Object(values))
    }
}

/// Decode exactly one UTF-8 JSON object, at most 4 MiB and depth 32.
pub fn strict_json(raw: &[u8]) -> Result<Value, String> {
    super::require(raw.len() <= JSON_LIMIT, "JSON size exceeds 4 MiB")?;
    let text = std::str::from_utf8(raw).map_err(|error| error.to_string())?;
    let mut decoder = serde_json::Deserializer::from_str(text);
    let value = Strict(0)
        .deserialize(&mut decoder)
        .map_err(|error| error.to_string())?;
    decoder.end().map_err(|error| error.to_string())?;
    super::require(value.is_object(), "JSON root must be an object")?;
    Ok(value)
}
