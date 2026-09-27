#![allow(dead_code)]
use fabric_o11y::{
    Event, Payload, TenantId,
    generator::{EventGenerator, WorkloadConfig},
};
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use std::{
    fs, io,
    os::raw::{c_int, c_long},
    time::Instant,
};
use storage_probe::{
    Query,
    coverage::{Digest, rows_digest},
};

#[repr(C)]
struct Timespec {
    sec: c_long,
    nsec: c_long,
}
unsafe extern "C" {
    fn clock_gettime(clock_id: c_int, tp: *mut Timespec) -> c_int;
}
const CLOCK_PROCESS_CPUTIME_ID: c_int = 2;
pub fn cpu_ns() -> io::Result<u128> {
    let mut t = Timespec { sec: 0, nsec: 0 };
    if unsafe { clock_gettime(CLOCK_PROCESS_CPUTIME_ID, &mut t) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(t.sec as u128 * 1_000_000_000 + t.nsec as u128)
}
#[derive(Clone, Copy, Serialize)]
pub struct Measure {
    pub wall_ns: u128,
    pub cpu_ns: u128,
}
pub fn measured<T>(f: impl FnOnce() -> T) -> io::Result<(T, Measure)> {
    let cpu = cpu_ns()?;
    let wall = Instant::now();
    let value = f();
    let wall_ns = wall.elapsed().as_nanos();
    let cpu_ns = cpu_ns()? - cpu;
    Ok((value, Measure { wall_ns, cpu_ns }))
}
pub fn calibration() -> io::Result<Vec<Measure>> {
    (0..100).map(|_| measured(|| ()).map(|(_, m)| m)).collect()
}
pub fn rss_kib() -> io::Result<u64> {
    fs::read_to_string("/proc/self/status")?
        .lines()
        .find_map(|line| {
            line.strip_prefix("VmHWM:")
                .and_then(|s| s.split_whitespace().next())
                .and_then(|s| s.parse().ok())
        })
        .ok_or_else(|| io::Error::other("missing VmHWM"))
}
#[derive(Clone, Copy, Serialize)]
pub struct IoCounters {
    pub rchar: u64,
    pub wchar: u64,
    pub read_bytes: u64,
    pub write_bytes: u64,
}
pub fn io_counters() -> io::Result<IoCounters> {
    let contents = fs::read_to_string("/proc/self/io")?;
    let get = |key: &str| -> io::Result<u64> {
        contents
            .lines()
            .find_map(|line| line.strip_prefix(key).and_then(|v| v.trim().parse().ok()))
            .ok_or_else(|| io::Error::other(format!("missing /proc/self/io {key}")))
    };
    Ok(IoCounters {
        rchar: get("rchar:")?,
        wchar: get("wchar:")?,
        read_bytes: get("read_bytes:")?,
        write_bytes: get("write_bytes:")?,
    })
}
impl IoCounters {
    pub fn since(self, start: Self) -> Self {
        Self {
            rchar: self.rchar - start.rchar,
            wchar: self.wchar - start.wchar,
            read_bytes: self.read_bytes - start.read_bytes,
            write_bytes: self.write_bytes - start.write_bytes,
        }
    }
}
pub fn sha_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
// Exact S1 workload/query adaptation from tools/storage-probe/src/main.rs
// workload() and queries(); 8192 scales the time modulus and repeats the
// eight-predicate cycle fourfold. E3 fault-corpus mutations are separate.
pub fn workload(name: &str, seed: u64, count: usize) -> (Vec<Event>, i64) {
    let mut events: Vec<_> = EventGenerator::new(WorkloadConfig {
        seed,
        events: count as u32,
    })
    .collect();
    let base = events[0].event_time.0;
    for (index, event) in events.iter_mut().enumerate() {
        event.tenant = TenantId((index as u64 * 17 + seed) % 1024 + 1);
        if name == "shuffled_logs" {
            event.event_time.0 =
                base + ((index as u64 * 109 + seed) % count as u64) as i64 * 1_000_000;
        }
        if name == "shuffled_logs" || (name == "mixed" && index % 2 == 0) {
            let term = if index % 257 == 0 { "rare" } else { "normal" };
            event.payload = Payload::Log {
                body: format!(
                    "common service{} request{index} {term} {}",
                    index % 16,
                    "x".repeat(128)
                ),
            };
        }
    }
    (events, base)
}
pub fn queries(base: i64, count: usize) -> Vec<Query> {
    (0..(if count == 8192 { 512 } else { 128 }))
        .map(|i| {
            let cycle = i / 8;
            let mut q = Query {
                start_ns: i64::MIN,
                end_ns: i64::MAX,
                tenant: None,
                token: None,
            };
            match i % 8 {
                1 | 6 => {
                    q.start_ns = base + ((cycle * 127) % count) as i64 * 1_000_000;
                    q.end_ns = q.start_ns + 31_000_000;
                    if i % 8 == 6 {
                        q.tenant = Some(cycle as u64 + 1);
                        q.token = Some("rare".into());
                    }
                }
                2 => q.tenant = Some(cycle as u64 + 1),
                3 => q.token = Some("rare".into()),
                4 => q.token = Some("absent".into()),
                5 => q.token = Some("common".into()),
                7 => {
                    q.start_ns = 1;
                    q.end_ns = 0;
                }
                _ => {}
            }
            q
        })
        .collect()
}
pub fn matches(event: &Event, q: &Query) -> bool {
    if event.event_time.0 < q.start_ns || event.event_time.0 > q.end_ns {
        return false;
    }
    if q.tenant.is_some_and(|tenant| event.tenant.0 != tenant) {
        return false;
    }
    match (&q.token, &event.payload) {
        (None, _) => true,
        (Some(token), Payload::Log { body }) => body.split_whitespace().any(|word| word == token),
        _ => false,
    }
}
pub fn expected(rows: &[Event], q: &Query) -> Vec<(usize, Digest)> {
    rows.iter()
        .enumerate()
        .filter(|(_, e)| matches(e, q))
        .map(|(i, e)| (i, rows_digest(std::slice::from_ref(e))))
        .collect()
}
pub fn check(actual: &[(usize, Digest)], want: &[(usize, Digest)]) -> io::Result<()> {
    if actual == want {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "query mismatch: actual={} expected={}",
            actual.len(),
            want.len()
        )))
    }
}
pub fn rank(values: &[u128], percentile: usize) -> u128 {
    let mut sorted = values.to_vec();
    sorted.sort();
    sorted[(sorted.len() * percentile).div_ceil(100).saturating_sub(1)]
}
