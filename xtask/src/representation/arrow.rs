//! Deliberately narrow, uncompressed fixed-schema Arrow IPC experiment.
//! Metadata is checked before the generic Arrow reader allocates arrays.
use super::{MAX_FRAME, MAX_INPUT, direct};
use arrow_array::{
    Array, ArrayRef, Float64Array, RecordBatch, StringArray, UInt64Array,
    builder::{Float64Builder, StringBuilder, UInt64Builder},
};
use arrow_ipc::{reader::StreamReader, writer::StreamWriter};
use arrow_schema::{DataType, Field, Schema};
use fabric::observation::Observation;
use std::{
    io::Cursor,
    sync::{Arc, OnceLock},
};
fn schema() -> Arc<Schema> {
    Arc::new(Schema::new(
        [
            "registry",
            "producer",
            "generation",
            "resource",
            "a",
            "b",
            "c",
        ]
        .iter()
        .map(|s| Field::new(*s, DataType::Utf8, true))
        .chain(
            [
                "version",
                "sequence",
                "timestamp_ns",
                "kind",
                "duration_ns",
                "flags",
                "value",
            ]
            .iter()
            .map(|s| {
                Field::new(
                    *s,
                    if *s == "value" {
                        DataType::Float64
                    } else {
                        DataType::UInt64
                    },
                    false,
                )
            }),
        )
        .collect::<Vec<_>>(),
    ))
}
fn schema_prefix() -> &'static [u8] {
    static PREFIX: OnceLock<Vec<u8>> = OnceLock::new();
    PREFIX.get_or_init(|| {
        let mut out = Vec::new();
        {
            let mut w = StreamWriter::try_new(&mut out, &schema()).unwrap();
            w.finish().unwrap();
        }
        out.truncate(out.len() - 8);
        out
    })
}
pub fn encode(records: &[Observation]) -> Result<Vec<u8>, String> {
    direct::validate(records)?;
    let mut strings: [StringBuilder; direct::STRINGS] =
        std::array::from_fn(|_| StringBuilder::with_capacity(records.len(), 0));
    let mut numbers: [UInt64Builder; 6] =
        std::array::from_fn(|_| UInt64Builder::with_capacity(records.len()));
    let mut values = Float64Builder::with_capacity(records.len());
    for o in records {
        let r = direct::row(o)?;
        for (builder, s) in strings.iter_mut().zip(r.strings) {
            builder.append_option(s.as_deref());
        }
        values.append_value(f64::from_bits(r.numbers[6]));
        for (builder, n) in numbers.iter_mut().zip(r.numbers) {
            builder.append_value(n);
        }
    }
    let arrays: Vec<ArrayRef> = strings
        .iter_mut()
        .map(|b| Arc::new(b.finish()) as ArrayRef)
        .chain(numbers.iter_mut().map(|b| Arc::new(b.finish()) as ArrayRef))
        .chain(std::iter::once(Arc::new(values.finish()) as ArrayRef))
        .collect();
    let batch = RecordBatch::try_new(schema(), arrays).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    {
        let mut writer =
            StreamWriter::try_new(&mut out, &batch.schema()).map_err(|e| e.to_string())?;
        writer.write(&batch).map_err(|e| e.to_string())?;
        writer.finish().map_err(|e| e.to_string())?;
    }
    if out.len() > MAX_FRAME {
        return Err("Arrow frame limit".into());
    }
    Ok(out)
}
fn preflight(bytes: &[u8]) -> Result<(), String> {
    let prefix = schema_prefix();
    if bytes.len() > MAX_FRAME || !bytes.starts_with(prefix) {
        return Err("Arrow fixed schema".into());
    }
    let rest = &bytes[prefix.len()..];
    if rest.len() < 16 || rest[..4] != [255; 4] {
        return Err("Arrow message framing".into());
    }
    let metadata = u32::from_le_bytes(rest[4..8].try_into().unwrap()) as usize;
    if metadata == 0 || metadata > 16 * 1024 || metadata > rest.len() - 8 {
        return Err("Arrow metadata limit".into());
    }
    let message = arrow_ipc::root_as_message(&rest[8..8 + metadata]).map_err(|e| e.to_string())?;
    let body = usize::try_from(message.bodyLength()).map_err(|_| "negative body")?;
    if body > MAX_FRAME
        || 8 + metadata + body + 8 != rest.len()
        || rest[rest.len() - 8..] != [255, 255, 255, 255, 0, 0, 0, 0]
    {
        return Err("Arrow body framing".into());
    }
    let record = message
        .header_as_record_batch()
        .ok_or("Arrow record required; dictionaries unsupported")?;
    if !(1..=32).contains(&record.length())
        || record.compression().is_some()
        || record.variadicBufferCounts().is_some()
    {
        return Err("Arrow rows/compression".into());
    }
    let nodes = record.nodes().ok_or("Arrow nodes")?;
    let buffers = record.buffers().ok_or("Arrow buffers")?;
    if nodes.len() != 14 || buffers.len() != 35 {
        return Err("Arrow fixed layout".into());
    }
    for node in nodes {
        if node.length() != record.length()
            || node.null_count() < 0
            || node.null_count() > node.length()
        {
            return Err("Arrow node length".into());
        }
    }
    // The generic reader assumes typed buffers have correctly sized elements.
    // Enforce the exact flat layout before handing untrusted metadata to it.
    let rows = record.length() as usize;
    for (index, buffer) in buffers.iter().enumerate() {
        let len = usize::try_from(buffer.length()).map_err(|_| "negative length")?;
        let offset = usize::try_from(buffer.offset()).map_err(|_| "negative offset")?;
        if offset % 8 != 0 {
            return Err("Arrow buffer alignment".into());
        }
        let role = if index < 21 {
            index % 3
        } else {
            (index - 21) % 2
        };
        if role == 0 && len != 0 && len != rows.div_ceil(8) {
            return Err("Arrow validity size".into());
        }
        if index < 21 && role == 1 && len != (rows + 1) * 4 {
            return Err("Arrow offsets size".into());
        }
        if index >= 21 && role == 1 && len != rows * 8 {
            return Err("Arrow primitive size".into());
        }
    }
    let mut total = 0usize;
    for buffer in buffers {
        let offset = usize::try_from(buffer.offset()).map_err(|_| "offset")?;
        let len = usize::try_from(buffer.length()).map_err(|_| "length")?;
        if offset > body || len > body - offset {
            return Err("Arrow buffer bounds".into());
        }
        total = total.checked_add(len).ok_or("buffers")?;
    }
    if total > MAX_FRAME {
        return Err("Arrow overlapping buffer amplification".into());
    }
    Ok(())
}
pub fn decode(bytes: &[u8]) -> Result<Vec<Observation>, String> {
    preflight(bytes)?;
    let mut reader = StreamReader::try_new(Cursor::new(bytes), None).map_err(|e| e.to_string())?;
    let batch = reader
        .next()
        .ok_or("missing batch")?
        .map_err(|e| e.to_string())?;
    if reader.next().is_some() {
        return Err("extra batch".into());
    }
    let strings: Vec<_> = batch.columns()[..7]
        .iter()
        .map(|a| {
            a.as_any()
                .downcast_ref::<StringArray>()
                .ok_or("string type")
        })
        .collect::<Result<_, _>>()?;
    let numbers: Vec<_> = batch.columns()[7..13]
        .iter()
        .map(|a| {
            a.as_any()
                .downcast_ref::<UInt64Array>()
                .ok_or("number type")
        })
        .collect::<Result<_, _>>()?;
    let values = batch
        .column(13)
        .as_any()
        .downcast_ref::<Float64Array>()
        .ok_or("float type")?;
    let mut out = Vec::with_capacity(batch.num_rows());
    let mut expanded = 0usize;
    for row in 0..batch.num_rows() {
        // Bound materialization before cloning repeated or overlapping strings.
        for a in &strings {
            if !a.is_null(row) {
                let n = a.value(row).len();
                expanded = expanded.checked_add(n).ok_or("strings")?;
                if n > 65536 || expanded > MAX_INPUT {
                    return Err("Arrow expanded strings".into());
                }
            }
        }
        if values.is_null(row) || numbers.iter().any(|a| a.is_null(row)) {
            return Err("null number".into());
        }
        let s = std::array::from_fn(|i| {
            if strings[i].is_null(row) {
                None
            } else {
                Some(strings[i].value(row).to_owned())
            }
        });
        let n = std::array::from_fn(|i| {
            if i == 6 {
                values.value(row).to_bits()
            } else {
                numbers[i].value(row)
            }
        });
        out.push(direct::observation(s, n)?);
    }
    direct::validate(&out)?;
    Ok(out)
}
