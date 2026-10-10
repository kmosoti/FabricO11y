//! Disposable run records: fixed-width scalars and length-prefixed UTF-8.
//! These bytes never survive a build; the journal remains durable custody.
use super::{LogRow, MetricRow, Number, SpanRow, invalid};
use std::collections::BTreeMap;
use std::io::{self, Read, Write};

const MAX_ROW_BYTES: usize = 16 * 1024 * 1024;
// Experimental build opt-in only. Unset retains the original codec calls.
pub(super) const REUSE_WORKSPACE: bool = option_env!("FABRIC_SPILL_WORKSPACE_EXPERIMENT").is_some();
const RETAIN_BYTES: usize = 256 * 1024;

#[derive(Default)]
pub(super) struct Workspace {
    bytes: Vec<u8>,
}

impl Workspace {
    fn buffer(&mut self, length: usize) -> &mut Vec<u8> {
        self.bytes.clear();
        if self.bytes.capacity() < length {
            self.bytes = Vec::with_capacity(length);
        }
        &mut self.bytes
    }

    pub(super) fn write<R: Spill>(&mut self, output: &mut impl Write, row: &R) -> io::Result<()> {
        let length = row.size();
        if length > MAX_ROW_BYTES {
            return Err(invalid("oversized private spill row"));
        }
        if length > RETAIN_BYTES {
            return write_row(output, row);
        }
        let bytes = self.buffer(length);
        if bytes.capacity() > RETAIN_BYTES {
            return Err(invalid("private spill workspace exceeds retention bound"));
        }
        row.encode(bytes);
        output.write_all(&(length as u32).to_le_bytes())?;
        output.write_all(bytes)
    }

    pub(super) fn read<R: Spill>(&mut self, input: &mut impl Read) -> io::Result<Option<R>> {
        let mut header = [0; 4];
        if input.read(&mut header[..1])? == 0 {
            return Ok(None);
        }
        input.read_exact(&mut header[1..])?;
        let length = u32::from_le_bytes(header) as usize;
        if length > MAX_ROW_BYTES {
            return Err(invalid("oversized private spill row"));
        }
        let mut large;
        let bytes = if length > RETAIN_BYTES {
            large = vec![0; length];
            &mut large
        } else {
            let bytes = self.buffer(length);
            if bytes.capacity() > RETAIN_BYTES {
                return Err(invalid("private spill workspace exceeds retention bound"));
            }
            bytes.resize(length, 0);
            bytes
        };
        input.read_exact(bytes)?;
        let mut rest = bytes.as_slice();
        let row = R::decode(&mut rest)?;
        if !rest.is_empty() {
            return Err(invalid("trailing private spill bytes"));
        }
        Ok(Some(row))
    }
}

pub(super) trait Spill: Sized {
    fn size(&self) -> usize;
    fn encode(&self, out: &mut Vec<u8>);
    fn decode(input: &mut &[u8]) -> io::Result<Self>;
}

fn take<'a>(input: &mut &'a [u8], length: usize) -> io::Result<&'a [u8]> {
    if length > input.len() {
        return Err(invalid("truncated private spill field"));
    }
    let (value, rest) = input.split_at(length);
    *input = rest;
    Ok(value)
}

macro_rules! scalar {
    ($($ty:ty),+) => {$ (
        impl Spill for $ty {
            fn size(&self) -> usize { size_of::<Self>() }
            fn encode(&self, out: &mut Vec<u8>) { out.extend_from_slice(&self.to_le_bytes()); }
            fn decode(input: &mut &[u8]) -> io::Result<Self> {
                Ok(Self::from_le_bytes(take(input, size_of::<Self>())?.try_into().unwrap()))
            }
        }
    )+};
}
scalar!(u32, u64, i32, i64);

impl Spill for [u8; 16] {
    fn size(&self) -> usize {
        16
    }
    fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(self);
    }
    fn decode(input: &mut &[u8]) -> io::Result<Self> {
        Ok(take(input, 16)?.try_into().unwrap())
    }
}

impl Spill for bool {
    fn size(&self) -> usize {
        1
    }
    fn encode(&self, out: &mut Vec<u8>) {
        out.push(u8::from(*self));
    }
    fn decode(input: &mut &[u8]) -> io::Result<Self> {
        match take(input, 1)?[0] {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(invalid("invalid private spill boolean")),
        }
    }
}

impl Spill for String {
    fn size(&self) -> usize {
        4 + self.len()
    }
    fn encode(&self, out: &mut Vec<u8>) {
        (self.len() as u32).encode(out);
        out.extend_from_slice(self.as_bytes());
    }
    fn decode(input: &mut &[u8]) -> io::Result<Self> {
        let length = u32::decode(input)? as usize;
        let bytes = take(input, length)?;
        let text = std::str::from_utf8(bytes).map_err(|_| invalid("private spill UTF-8"))?;
        Ok(text.to_owned())
    }
}

impl Spill for BTreeMap<String, String> {
    fn size(&self) -> usize {
        4 + self.iter().map(|(k, v)| k.size() + v.size()).sum::<usize>()
    }
    fn encode(&self, out: &mut Vec<u8>) {
        (self.len() as u32).encode(out);
        for (key, value) in self {
            key.encode(out);
            value.encode(out);
        }
    }
    fn decode(input: &mut &[u8]) -> io::Result<Self> {
        let count = u32::decode(input)? as usize;
        if count > input.len() / 8 {
            return Err(invalid("private spill attribute count"));
        }
        let mut out = Self::new();
        for _ in 0..count {
            let key = String::decode(input)?;
            let value = String::decode(input)?;
            if out.insert(key, value).is_some() {
                return Err(invalid("duplicate private spill attribute"));
            }
        }
        Ok(out)
    }
}

impl Spill for Number {
    fn size(&self) -> usize {
        9
    }
    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Self::Int(value) => {
                out.push(0);
                value.encode(out);
            }
            Self::Double(value) => {
                out.push(1);
                value.to_bits().encode(out);
            }
        }
    }
    fn decode(input: &mut &[u8]) -> io::Result<Self> {
        match take(input, 1)?[0] {
            0 => Ok(Self::Int(i64::decode(input)?)),
            1 => Ok(Self::Double(f64::from_bits(u64::decode(input)?))),
            _ => Err(invalid("invalid private spill number")),
        }
    }
}

// One field list drives size, encode and decode. Every field is preserved;
// tests also compare final Parquet bytes with the independent whole-file path.
macro_rules! row {
    ($ty:ty; $($field:ident),+ $(,)?) => {
        impl Spill for $ty {
            fn size(&self) -> usize { 0 $(+ self.$field.size())+ }
            fn encode(&self, out: &mut Vec<u8>) { $(self.$field.encode(out);)+ }
            fn decode(input: &mut &[u8]) -> io::Result<Self> {
                Ok(Self { $($field: Spill::decode(input)?,)+ })
            }
        }
    };
}
row!(LogRow; group,node,node_id,sequence,index,observed_ns,body,attributes);
row!(MetricRow; group,node,node_id,sequence,index,name,unit,sum,monotonic,time_ns,start_ns,value,attributes);
row!(SpanRow; group,node,node_id,sequence,index,trace_id,span_id,parent_span_id,name,kind,status,start_ns,end_ns,attributes);

pub(super) fn write_row<R: Spill>(output: &mut impl Write, row: &R) -> io::Result<()> {
    let length = row.size();
    if length > MAX_ROW_BYTES {
        return Err(invalid("oversized private spill row"));
    }
    let mut bytes = Vec::with_capacity(length);
    row.encode(&mut bytes);
    output.write_all(&(length as u32).to_le_bytes())?;
    output.write_all(&bytes)
}

pub(super) fn read_row<R: Spill>(input: &mut impl Read) -> io::Result<Option<R>> {
    let mut length = [0; 4];
    if input.read(&mut length[..1])? == 0 {
        return Ok(None);
    }
    input.read_exact(&mut length[1..])?;
    let length = u32::from_le_bytes(length) as usize;
    if length > MAX_ROW_BYTES {
        return Err(invalid("oversized private spill row"));
    }
    let mut bytes = vec![0; length];
    input.read_exact(&mut bytes)?;
    let mut rest = bytes.as_slice();
    let row = R::decode(&mut rest)?;
    if !rest.is_empty() {
        return Err(invalid("trailing private spill bytes"));
    }
    Ok(Some(row))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_preserves_bytes_errors_and_releases_maximum_row_capacity() {
        let mut encoder = Workspace::default();
        let mut decoder = Workspace::default();
        for text in ["x".repeat(MAX_ROW_BYTES - 4), "λ\0\n".into(), String::new()] {
            let mut reference = Vec::new();
            write_row(&mut reference, &text).unwrap();
            let mut candidate = Vec::new();
            encoder.write(&mut candidate, &text).unwrap();
            assert_eq!(candidate, reference);
            assert_eq!(
                decoder.read::<String>(&mut candidate.as_slice()).unwrap(),
                Some(text)
            );
            assert!(encoder.bytes.capacity() <= RETAIN_BYTES);
            assert!(decoder.bytes.capacity() <= RETAIN_BYTES);
        }
        for bytes in [
            vec![1],
            vec![255; 4],
            vec![0; 4],
            vec![4, 0, 0, 0, 255, 255, 255, 255],
        ] {
            let original = read_row::<String>(&mut bytes.as_slice()).unwrap_err();
            let reused = decoder.read::<String>(&mut bytes.as_slice()).unwrap_err();
            assert_eq!(original.kind(), reused.kind());
        }
        let mut framed = Vec::new();
        write_row(&mut framed, &"λ-valid".to_owned()).unwrap();
        for n in 1..framed.len() {
            assert!(decoder.read::<String>(&mut &framed[..n]).is_err());
        }
        let mut changed = framed.clone();
        *changed.last_mut().unwrap() ^= 1;
        let wrong = decoder
            .read::<String>(&mut changed.as_slice())
            .unwrap()
            .unwrap();
        let mut reencoded = Vec::new();
        encoder.write(&mut reencoded, &wrong).unwrap();
        assert_ne!(
            reencoded, framed,
            "changed-field negative control escaped byte comparison"
        );
        assert!(decoder.read::<String>(&mut &[][..]).unwrap().is_none());
    }

    #[test]
    fn fixed_scalar_fixture_preserves_signedness_endianness_and_float_bits() {
        // Explicit bytes are independent of encode: i64::MIN and a NaN payload.
        let mut integer: &[u8] = &[0, 0, 0, 0, 0, 0, 0, 0, 128];
        assert_eq!(Number::decode(&mut integer).unwrap(), Number::Int(i64::MIN));
        let mut double: &[u8] = &[1, 0x42, 0, 0, 0, 0, 0, 0xf8, 0x7f];
        let Number::Double(value) = Number::decode(&mut double).unwrap() else {
            panic!()
        };
        assert_eq!(value.to_bits(), 0x7ff8000000000042);
        let mut bytes = Vec::new();
        Number::Int(i64::MIN).encode(&mut bytes);
        assert_eq!(bytes, [0, 0, 0, 0, 0, 0, 0, 0, 128]);
    }

    #[test]
    fn malformed_fields_and_frames_are_rejected() {
        assert!(bool::decode(&mut &[2][..]).is_err());
        assert!(Number::decode(&mut &[2][..]).is_err());
        assert!(String::decode(&mut &[1, 0, 0, 0, 255][..]).is_err());
        assert!(String::decode(&mut &[255, 255, 255, 255][..]).is_err());
        assert!(BTreeMap::<String, String>::decode(&mut &[255, 255, 255, 255][..]).is_err());
        for bytes in [
            vec![1],
            vec![255; 4],
            vec![0; 4],
            vec![5, 0, 0, 0, 0, 0, 0, 0, 0],
        ] {
            assert!(read_row::<u32>(&mut bytes.as_slice()).is_err());
        }
        // Two equal empty keys may not silently overwrite one another.
        let mut duplicates = vec![2, 0, 0, 0];
        duplicates.extend([0; 16]);
        assert!(BTreeMap::<String, String>::decode(&mut duplicates.as_slice()).is_err());
    }

    #[test]
    fn every_projected_field_survives_a_private_run() {
        let attributes =
            BTreeMap::from([("\0λ\\\"".into(), "\nvalue".into()), ("".into(), "".into())]);
        let metric = MetricRow {
            group: u64::MAX,
            node: "λ\0".into(),
            node_id: [255; 16],
            sequence: u64::MAX - 1,
            index: u32::MAX,
            name: "total".into(),
            unit: "bytes".into(),
            sum: true,
            monotonic: true,
            time_ns: 42,
            start_ns: 17,
            value: Number::Int(i64::MIN),
            attributes: attributes.clone(),
        };
        let span = SpanRow {
            group: 2,
            node: "node".into(),
            node_id: [1; 16],
            sequence: 3,
            index: 4,
            trace_id: "trace".into(),
            span_id: "span".into(),
            parent_span_id: "parent".into(),
            name: "λ\0".into(),
            kind: i32::MIN,
            status: i32::MAX,
            start_ns: 5,
            end_ns: u64::MAX,
            attributes,
        };
        let mut bytes = Vec::new();
        write_row(&mut bytes, &metric).unwrap();
        assert_eq!(bytes.len(), 4 + metric.size());
        assert_eq!(read_row(&mut bytes.as_slice()).unwrap(), Some(metric));
        bytes.clear();
        write_row(&mut bytes, &span).unwrap();
        assert_eq!(bytes.len(), 4 + span.size());
        assert_eq!(read_row(&mut bytes.as_slice()).unwrap(), Some(span));
        for prefix in 1..bytes.len() {
            assert!(read_row::<SpanRow>(&mut &bytes[..prefix]).is_err());
        }
    }
}
