//! Typed experimental rows shared by direct binary and Arrow; no full-batch Value tree.
use super::{MAX_FRAME, MAX_INPUT};
use fabric::observation::{Data, Identity, Level, MetricType, Observation, Outcome};
use std::{borrow::Cow, io::Write};
pub const STRINGS: usize = 7;
pub const NUMBERS: usize = 7;
pub struct Row<'a> {
    pub strings: [Option<Cow<'a, str>>; STRINGS],
    pub numbers: [u64; NUMBERS],
}
fn outcome(x: Outcome) -> u64 {
    match x {
        Outcome::Ok => 0,
        Outcome::Error => 1,
        Outcome::Cancelled => 2,
    }
}
pub fn row(o: &Observation) -> Result<Row<'_>, String> {
    let mut r = Row {
        strings: [
            Some(Cow::Borrowed(&o.registry)),
            Some(Cow::Borrowed(&o.identity.producer)),
            Some(Cow::Borrowed(&o.identity.generation)),
            Some(Cow::Borrowed(&o.resource)),
            None,
            None,
            None,
        ],
        numbers: [
            o.version as u64,
            o.identity.sequence,
            o.timestamp_ns,
            0,
            0,
            0,
            0,
        ],
    };
    let (kind, a, b, c, n, f, v) = match &o.data {
        Data::ServiceRequest {
            service,
            operation,
            duration_ns,
            status,
            trace_id,
        } => (
            0,
            Some(Cow::Borrowed(service.as_str())),
            Some(Cow::Borrowed(operation.as_str())),
            trace_id.as_deref().map(Cow::Borrowed),
            *duration_ns,
            outcome(*status),
            0,
        ),
        Data::AgentRun {
            agent,
            run_id,
            duration_ns,
            status,
        } => (
            1,
            Some(Cow::Borrowed(agent.as_str())),
            Some(Cow::Borrowed(run_id.as_str())),
            None,
            *duration_ns,
            outcome(*status),
            0,
        ),
        Data::ToolExecution {
            tool,
            run_id,
            duration_ns,
            status,
        } => (
            2,
            Some(Cow::Borrowed(tool.as_str())),
            Some(Cow::Borrowed(run_id.as_str())),
            None,
            *duration_ns,
            outcome(*status),
            0,
        ),
        Data::Log { level, message } => (
            3,
            Some(Cow::Borrowed(message.as_str())),
            None,
            None,
            0,
            match level {
                Level::Trace => 0,
                Level::Debug => 1,
                Level::Info => 2,
                Level::Warn => 3,
                Level::Error => 4,
            },
            0,
        ),
        Data::JsonLog { fields } => (
            4,
            Some(Cow::Owned(
                serde_json::to_string(fields).map_err(|e| e.to_string())?,
            )),
            None,
            None,
            0,
            0,
            0,
        ),
        Data::Event { name, message } => (
            5,
            Some(Cow::Borrowed(name.as_str())),
            Some(Cow::Borrowed(message.as_str())),
            None,
            0,
            0,
            0,
        ),
        Data::Metric {
            name,
            unit,
            value,
            metric_type,
            reset,
        } => (
            6,
            Some(Cow::Borrowed(name.as_str())),
            Some(Cow::Borrowed(unit.as_str())),
            None,
            0,
            (if *metric_type == MetricType::Gauge {
                1
            } else {
                0
            }) | ((*reset as u64) << 1),
            value.to_bits(),
        ),
        Data::Gap { source, reason } => (
            7,
            Some(Cow::Borrowed(source.as_str())),
            Some(Cow::Borrowed(reason.as_str())),
            None,
            0,
            0,
            0,
        ),
    };
    r.strings[4] = a;
    r.strings[5] = b;
    r.strings[6] = c;
    r.numbers[3] = kind;
    r.numbers[4] = n;
    r.numbers[5] = f;
    r.numbers[6] = v;
    Ok(r)
}
pub fn observation(
    mut s: [Option<String>; STRINGS],
    n: [u64; NUMBERS],
) -> Result<Observation, String> {
    fn take(s: &mut [Option<String>; STRINGS], i: usize) -> Result<String, String> {
        s[i].take().ok_or_else(|| "missing string".into())
    }
    let status = || match n[5] {
        0 => Ok(Outcome::Ok),
        1 => Ok(Outcome::Error),
        2 => Ok(Outcome::Cancelled),
        _ => Err("status".to_owned()),
    };
    let data = match n[3] {
        0 => Data::ServiceRequest {
            service: take(&mut s, 4)?,
            operation: take(&mut s, 5)?,
            trace_id: s[6].take(),
            duration_ns: n[4],
            status: status()?,
        },
        1 => Data::AgentRun {
            agent: take(&mut s, 4)?,
            run_id: take(&mut s, 5)?,
            duration_ns: n[4],
            status: status()?,
        },
        2 => Data::ToolExecution {
            tool: take(&mut s, 4)?,
            run_id: take(&mut s, 5)?,
            duration_ns: n[4],
            status: status()?,
        },
        3 => Data::Log {
            message: take(&mut s, 4)?,
            level: match n[5] {
                0 => Level::Trace,
                1 => Level::Debug,
                2 => Level::Info,
                3 => Level::Warn,
                4 => Level::Error,
                _ => return Err("level".into()),
            },
        },
        4 => {
            let raw = take(&mut s, 4)?;
            fabric::observation::check_json(raw.as_bytes(), 65536)?;
            Data::JsonLog {
                fields: serde_json::from_str(&raw).map_err(|e| e.to_string())?,
            }
        }
        5 => Data::Event {
            name: take(&mut s, 4)?,
            message: take(&mut s, 5)?,
        },
        6 if n[5] < 4 => Data::Metric {
            name: take(&mut s, 4)?,
            unit: take(&mut s, 5)?,
            value: f64::from_bits(n[6]),
            metric_type: if n[5] & 1 == 1 {
                MetricType::Gauge
            } else {
                MetricType::Counter
            },
            reset: n[5] & 2 != 0,
        },
        7 => Data::Gap {
            source: take(&mut s, 4)?,
            reason: take(&mut s, 5)?,
        },
        _ => return Err("kind".into()),
    };
    let o = Observation {
        version: n[0].try_into().map_err(|_| "version")?,
        registry: take(&mut s, 0)?,
        identity: Identity {
            producer: take(&mut s, 1)?,
            generation: take(&mut s, 2)?,
            sequence: n[1],
        },
        timestamp_ns: n[2],
        resource: take(&mut s, 3)?,
        data,
    };
    o.validate()?;
    Ok(o)
}
struct Limit {
    len: usize,
    limit: usize,
    depth: usize,
    quoted: bool,
    escaped: bool,
    max_depth: usize,
}
impl Write for Limit {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        if b.len() > self.limit - self.len {
            return Err(std::io::Error::other("canonical limit"));
        }
        for &c in b {
            if self.quoted {
                if self.escaped {
                    self.escaped = false;
                } else if c == b'\\' {
                    self.escaped = true;
                } else if c == b'"' {
                    self.quoted = false;
                }
            } else {
                match c {
                    b'"' => self.quoted = true,
                    b'{' | b'[' => {
                        self.depth += 1;
                        if self.depth > self.max_depth {
                            return Err(std::io::Error::other("nesting limit"));
                        }
                    }
                    b'}' | b']' => self.depth = self.depth.saturating_sub(1),
                    _ => (),
                }
            }
        }
        self.len += b.len();
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
/// Validate size by streaming to a counter, without allocating a second batch.
pub fn validate(records: &[Observation]) -> Result<(), String> {
    if records.is_empty() || records.len() > 32 {
        return Err("record count".into());
    }
    for o in records {
        o.validate()?;
        let mut size = Limit {
            len: 0,
            limit: 65536,
            depth: 0,
            quoted: false,
            escaped: false,
            max_depth: 16,
        };
        serde_json::to_writer(&mut size, o).map_err(|e| e.to_string())?;
    }
    let mut size = Limit {
        len: 0,
        limit: MAX_INPUT,
        depth: 0,
        quoted: false,
        escaped: false,
        max_depth: 17,
    };
    serde_json::to_writer(&mut size, records).map_err(|e| e.to_string())?;
    Ok(())
}
pub fn encode(records: &[Observation]) -> Result<Vec<u8>, String> {
    validate(records)?;
    let mut out = Vec::new();
    out.extend_from_slice(&(records.len() as u32).to_le_bytes());
    for o in records {
        let r = row(o)?;
        for n in r.numbers {
            out.extend_from_slice(&n.to_le_bytes());
        }
        for s in r.strings {
            if let Some(s) = s {
                out.extend_from_slice(&(s.len() as u32).to_le_bytes());
                out.extend_from_slice(s.as_bytes());
            } else {
                out.extend_from_slice(&u32::MAX.to_le_bytes());
            }
        }
    }
    if out.len() > MAX_FRAME {
        return Err("frame".into());
    }
    Ok(out)
}
pub fn decode(bytes: &[u8]) -> Result<Vec<Observation>, String> {
    if bytes.len() > MAX_FRAME {
        return Err("frame".into());
    }
    let mut b = bytes;
    fn take<'a>(b: &mut &'a [u8], n: usize) -> Result<&'a [u8], String> {
        if n > b.len() {
            return Err("truncated".into());
        }
        let (a, rest) = b.split_at(n);
        *b = rest;
        Ok(a)
    }
    let count = u32::from_le_bytes(take(&mut b, 4)?.try_into().unwrap()) as usize;
    if count == 0 || count > 32 {
        return Err("count".into());
    }
    let mut out = Vec::with_capacity(count);
    let mut expanded = 0usize;
    for _ in 0..count {
        let mut nums = [0; NUMBERS];
        for n in &mut nums {
            *n = u64::from_le_bytes(take(&mut b, 8)?.try_into().unwrap());
        }
        let mut strings = std::array::from_fn(|_| None);
        for s in &mut strings {
            let n = u32::from_le_bytes(take(&mut b, 4)?.try_into().unwrap());
            if n != u32::MAX {
                let n = n as usize;
                expanded = expanded.checked_add(n).ok_or("length")?;
                if n > 65536 || expanded > MAX_INPUT {
                    return Err("string budget".into());
                }
                *s = Some(
                    std::str::from_utf8(take(&mut b, n)?)
                        .map_err(|e| e.to_string())?
                        .to_owned(),
                );
            }
        }
        out.push(observation(strings, nums)?);
    }
    if !b.is_empty() {
        return Err("trailing".into());
    }
    validate(&out)?;
    Ok(out)
}
