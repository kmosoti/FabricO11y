//! CR1 private codec roundtrip; reusable byte workspace is the only variable.
use fabric_server::rows::{LogRow, MetricRow, Number, SpanRow};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::Path;
use std::time::Instant;
// This isolated snapshot exercises only the original codec subset; the actual
// workspace and opt-in merge path are covered by production-module controls.
#[allow(dead_code)]
#[path = "../src/segment/bounded/spill.rs"]
mod spill;
use spill::Spill;
const MAX_ROW: usize = 16 * 1024 * 1024;
const RETAIN: usize = 256 * 1024;
const SEED: u64 = 2703163393;
fn invalid(text: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, text)
}

#[cfg(feature = "responsibility-alloc-probe")]
mod allocation {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
    struct Counted;
    static LIVE: AtomicUsize = AtomicUsize::new(0);
    static PEAK: AtomicUsize = AtomicUsize::new(0);
    static TOTAL: AtomicUsize = AtomicUsize::new(0);
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    fn add(n: usize) {
        TOTAL.fetch_add(n, Relaxed);
        CALLS.fetch_add(1, Relaxed);
        PEAK.fetch_max(LIVE.fetch_add(n, Relaxed) + n, Relaxed);
    }
    // Measurement only, following existing allocation probes. System owns
    // pointer/layout validity; counters do not allocate. No unsafe candidate.
    unsafe impl GlobalAlloc for Counted {
        unsafe fn alloc(&self, l: Layout) -> *mut u8 {
            let p = unsafe { System.alloc(l) };
            if !p.is_null() {
                add(l.size());
            }
            p
        }
        unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
            let p = unsafe { System.alloc_zeroed(l) };
            if !p.is_null() {
                add(l.size());
            }
            p
        }
        unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
            unsafe { System.dealloc(p, l) };
            LIVE.fetch_sub(l.size(), Relaxed);
        }
        unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
            let q = unsafe { System.realloc(p, l, n) };
            if !q.is_null() {
                TOTAL.fetch_add(n, Relaxed);
                CALLS.fetch_add(1, Relaxed);
                if n >= l.size() {
                    PEAK.fetch_max(
                        LIVE.fetch_add(n - l.size(), Relaxed) + n - l.size(),
                        Relaxed,
                    );
                } else {
                    LIVE.fetch_sub(l.size() - n, Relaxed);
                }
            }
            q
        }
    }
    #[global_allocator]
    static ALLOCATOR: Counted = Counted;
    pub fn reset() -> usize {
        let n = LIVE.load(Relaxed);
        PEAK.store(n, Relaxed);
        TOTAL.store(0, Relaxed);
        CALLS.store(0, Relaxed);
        n
    }
    pub fn facts() -> (usize, usize, usize, usize) {
        (
            LIVE.load(Relaxed),
            PEAK.load(Relaxed),
            TOTAL.load(Relaxed),
            CALLS.load(Relaxed),
        )
    }
}

#[derive(Default)]
struct Workspace {
    bytes: Vec<u8>,
    max_capacity: usize,
}
impl Workspace {
    fn buffer(&mut self, length: usize) -> &mut Vec<u8> {
        self.bytes.clear();
        if self.bytes.capacity() < length {
            // Exact replacement avoids geometric growth beyond the retention cap.
            self.bytes = Vec::with_capacity(length);
        }
        self.max_capacity = self.max_capacity.max(self.bytes.capacity());
        &mut self.bytes
    }
    fn write<R: Spill>(&mut self, out: &mut impl Write, row: &R) -> io::Result<()> {
        let n = row.size();
        if n > MAX_ROW {
            return Err(invalid("oversized private spill row"));
        }
        if n > RETAIN {
            return spill::write_row(out, row); // Large temporary is never pooled.
        }
        let bytes = self.buffer(n);
        row.encode(bytes);
        out.write_all(&(n as u32).to_le_bytes())?;
        out.write_all(bytes)
    }
    fn read<R: Spill>(&mut self, input: &mut impl Read) -> io::Result<Option<R>> {
        let mut header = [0; 4];
        if input.read(&mut header[..1])? == 0 {
            return Ok(None);
        }
        input.read_exact(&mut header[1..])?;
        let n = u32::from_le_bytes(header) as usize;
        if n > MAX_ROW {
            return Err(invalid("oversized private spill row"));
        }
        let mut large;
        let bytes = if n > RETAIN {
            large = vec![0; n];
            &mut large
        } else {
            let b = self.buffer(n);
            b.resize(n, 0);
            b
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

fn hash(path: &Path) -> io::Result<String> {
    let mut f = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0; 64 * 1024];
    loop {
        let n = f.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
    }
    Ok(format!("{:x}", digest.finalize()))
}
fn owned_path(path: &Path) -> io::Result<()> {
    let root = Path::new(&std::env::var("FABRIC_SCRATCH_ROOT").map_err(io::Error::other)?)
        .canonicalize()?;
    let parent = path
        .parent()
        .ok_or_else(|| invalid("missing parent"))?
        .canonicalize()?;
    if !parent.starts_with(root) {
        return Err(invalid("owned data-drive scratch required"));
    }
    Ok(())
}
fn generate(path: &Path, mib: usize) -> io::Result<()> {
    owned_path(path)?;
    let mut writer = BufWriter::new(File::create_new(path)?);
    let mut rng = SEED;
    let mut bytes = 0;
    let mut index = 0u64;
    while bytes < mib * 1024 * 1024 {
        let mut body = String::with_capacity(1024);
        for _ in 0..1024 {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            body.push(if index.is_multiple_of(2) {
                'R'
            } else {
                (b'!' + (rng % 90) as u8) as char
            });
        }
        let row = LogRow {
            group: index / 128,
            node: "node-λ".into(),
            node_id: [7; 16],
            sequence: index / 128 + 1,
            index: (index % 128) as u32,
            observed_ns: SEED + index,
            body,
            attributes: BTreeMap::from([("key\0λ".into(), "value\n\\\"".into())]),
        };
        spill::write_row(&mut writer, &row)?;
        bytes += 4 + row.size();
        index += 1;
    }
    writer.flush()?;
    println!(
        "{}",
        json!({"seed":SEED,"rows":index,"bytes":bytes,"sha256":hash(path)?})
    );
    Ok(())
}
fn controls() -> io::Result<()> {
    let mut write = Workspace::default();
    let mut read = Workspace::default();
    // Maximum legal private row followed by small rows must not pin its capacity.
    for value in ["x".repeat(MAX_ROW - 4), "λ\0\n".into(), String::new()] {
        let mut reference = Vec::new();
        spill::write_row(&mut reference, &value)?;
        let mut candidate = Vec::new();
        write.write(&mut candidate, &value)?;
        if reference != candidate {
            return Err(invalid("encode byte mismatch"));
        }
        let decoded = read.read::<String>(&mut candidate.as_slice())?.unwrap();
        if decoded != value {
            return Err(invalid("decode value mismatch"));
        }
        if write.bytes.capacity() > RETAIN || read.bytes.capacity() > RETAIN {
            return Err(invalid("maximum row retained"));
        }
    }
    for bytes in [
        vec![1],
        vec![255; 4],
        vec![0; 4],
        vec![5, 0, 0, 0, 0, 0, 0, 0, 0],
        vec![4, 0, 0, 0, 255, 255, 255, 255],
    ] {
        let a = spill::read_row::<String>(&mut bytes.as_slice());
        let b = read.read::<String>(&mut bytes.as_slice());
        if a.is_ok() || b.is_ok() {
            return Err(invalid("malformed frame accepted"));
        }
        if a.unwrap_err().kind() != b.unwrap_err().kind() {
            return Err(invalid("error kind mismatch"));
        }
    }
    let mut valid = Vec::new();
    spill::write_row(&mut valid, &"λ-control".to_owned())?;
    for n in 1..valid.len() {
        if spill::read_row::<String>(&mut &valid[..n]).is_ok()
            || read.read::<String>(&mut &valid[..n]).is_ok()
        {
            return Err(invalid("truncated frame accepted"));
        }
    }
    // Independent little-endian signed integer and NaN-payload byte controls.
    for fixture in [
        vec![9, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 128],
        vec![9, 0, 0, 0, 1, 0x42, 0, 0, 0, 0, 0, 0xf8, 0x7f],
    ] {
        let number = read.read::<Number>(&mut fixture.as_slice())?.unwrap();
        let mut encoded = Vec::new();
        write.write(&mut encoded, &number)?;
        if encoded != fixture {
            return Err(invalid("scalar bits changed"));
        }
    }
    // An over-limit output is rejected before any prefix is emitted.
    let over = "x".repeat(MAX_ROW - 3);
    if write.write(&mut Vec::new(), &over).is_ok()
        || spill::write_row(&mut Vec::new(), &over).is_ok()
    {
        return Err(invalid("oversized encoding accepted"));
    }
    if read.read::<String>(&mut &[][..])?.is_some() {
        return Err(invalid("EOF changed"));
    }
    println!(
        "{}",
        json!({"controls":true,"max_row_bytes":MAX_ROW,"retain_limit_bytes":RETAIN,
        "malformed_rejected":true,"truncations_rejected":true,"scalar_bits_exact":true,"large_then_small_bounded":true})
    );
    Ok(())
}
fn run(input: &Path, output: &Path, variant: &str) -> io::Result<()> {
    owned_path(input)?;
    owned_path(output)?;
    let encode = matches!(variant, "encode" | "combined");
    let decode = matches!(variant, "decode" | "combined");
    if !matches!(variant, "baseline" | "encode" | "decode" | "combined") {
        return Err(invalid("unknown variant"));
    }
    let mut reader = BufReader::with_capacity(256 * 1024, File::open(input)?);
    let mut writer = BufWriter::with_capacity(256 * 1024, File::create_new(output)?);
    let (mut enc, mut dec) = (Workspace::default(), Workspace::default());
    #[cfg(feature = "responsibility-alloc-probe")]
    let start_heap = allocation::reset();
    let began = Instant::now();
    let mut rows = 0;
    loop {
        let row = if decode {
            dec.read::<LogRow>(&mut reader)?
        } else {
            spill::read_row::<LogRow>(&mut reader)?
        };
        let Some(row) = row else {
            break;
        };
        if encode {
            enc.write(&mut writer, &row)?;
        } else {
            spill::write_row(&mut writer, &row)?;
        }
        rows += 1;
    }
    writer.flush()?;
    let seconds = began.elapsed().as_secs_f64();
    #[cfg(feature = "responsibility-alloc-probe")]
    let (live, peak, total, calls) = allocation::facts();
    let capacities = (enc.bytes.capacity(), dec.bytes.capacity());
    if capacities.0 > RETAIN || capacities.1 > RETAIN {
        return Err(invalid("workspace retained above bound"));
    }
    drop(enc);
    drop(dec);
    #[cfg(feature = "responsibility-alloc-probe")]
    let after_drop = allocation::facts().0;
    #[cfg(feature = "responsibility-alloc-probe")]
    let allocator = json!({"start_live":start_heap,"after_phase_live":live,"incremental_peak":peak.saturating_sub(start_heap),
        "cumulative_requested_bytes":total,"successful_alloc_realloc_calls":calls,"after_workspace_drop_live":after_drop});
    #[cfg(not(feature = "responsibility-alloc-probe"))]
    let allocator = serde_json::Value::Null;
    let input_hash = hash(input)?;
    let output_hash = hash(output)?;
    if input_hash != output_hash {
        return Err(invalid("exact roundtrip bytes differ"));
    }
    println!(
        "{}",
        json!({"variant":variant,"seed":SEED,"rows":rows,"bytes":input.metadata()?.len(),
        "input_sha256":input_hash,"output_sha256":output_hash,"exact_bytes":true,"phase_wall_s":seconds,
        "encode_retained_capacity":capacities.0,"decode_retained_capacity":capacities.1,"allocator":allocator})
    );
    Ok(())
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("controls") if args.len() == 1 => controls()?,
        Some("gen") if args.len() == 3 => {
            let mib = args[2].parse()?;
            if mib != 64 {
                return Err("only registered 64 MiB input".into());
            }
            generate(Path::new(&args[1]), mib)?;
        }
        Some("run") if args.len() == 4 => run(Path::new(&args[1]), Path::new(&args[2]), &args[3])?,
        _ => return Err(
            "controls | gen NEW_INPUT 64 | run INPUT NEW_OUTPUT baseline|encode|decode|combined"
                .into(),
        ),
    }
    Ok(())
}
