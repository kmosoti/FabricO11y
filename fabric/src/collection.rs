use serde::{Deserialize, Serialize};
use std::{
    fs::OpenOptions,
    io::{Read, Write},
    path::Path,
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Observation {
    pub version: u8,
    pub id: String,
    pub producer: String,
    pub timestamp: u64,
    pub kind: String,
    pub text: String,
    pub value: f64,
    pub vector: Vec<f64>,
    pub related: Vec<String>,
}

impl Observation {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.version != 1
            || self.id.is_empty()
            || self.producer.is_empty()
            || self.kind.is_empty()
        {
            return Err("invalid identity or schema version");
        }
        if !self.value.is_finite() || self.vector.iter().any(|x| !x.is_finite()) {
            return Err("non-finite number");
        }
        if self.vector.len() > 256 || self.related.len() > 256 {
            return Err("collection limit exceeded");
        }
        Ok(())
    }
}

/// Append one bounded record and synchronize it. This requires a single writer.
/// A write error can leave a partial tail; callers must treat an error as an
/// unknown outcome and repair the file. The parent directory is not synced.
pub fn durable_append(
    path: &Path,
    bytes: &[u8],
    max_record: usize,
    max_file: u64,
) -> Result<(), String> {
    if bytes.len() > max_record || bytes.contains(&b'\n') {
        return Err("invalid frame size or delimiter".into());
    }
    let observation: Observation = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    observation.validate().map_err(str::to_owned)?;
    let mut file = OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    let length = file.metadata().map_err(|e| e.to_string())?.len();
    if length
        .checked_add(bytes.len() as u64 + 1)
        .is_none_or(|n| n > max_file)
    {
        return Err("store full".into());
    }
    file.write_all(bytes)
        .and_then(|_| file.write_all(b"\n"))
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())
}

/// Read a complete local file, rejecting oversized or incomplete data.
pub fn read_store(
    path: &Path,
    max_file: u64,
    max_record: usize,
) -> Result<Vec<Observation>, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take(max_file.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > max_file {
        return Err("store too large".into());
    }
    if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        return Err("incomplete frame".into());
    }
    bytes
        .split(|b| *b == b'\n')
        .filter(|frame| !frame.is_empty())
        .map(|frame| {
            if frame.len() > max_record {
                return Err("record too large".into());
            }
            let record: Observation = serde_json::from_slice(frame).map_err(|e| e.to_string())?;
            record.validate().map_err(str::to_owned)?;
            Ok(record)
        })
        .collect()
}
