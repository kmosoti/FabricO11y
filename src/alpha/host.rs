//! Bounded Linux host reads for the node's small OTLP metric profile.

use std::ffi::CString;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

const MAX_PROC_BYTES: u64 = 256 * 1024;
const MAX_DEVICES: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Gauge,
    Counter,
}

#[derive(Clone, Debug)]
pub enum Value {
    Int(u64),
    Double(f64),
}

impl Value {
    pub fn comparable(&self) -> f64 {
        match self {
            Self::Int(value) => *value as f64,
            Self::Double(value) => *value,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Point {
    pub name: &'static str,
    pub unit: &'static str,
    pub kind: Kind,
    pub source: String,
    pub value: Value,
    pub known_start_ns: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub hostname: String,
    pub boot_id: String,
    pub points: Vec<Point>,
}

#[derive(Clone)]
pub struct Paths {
    pub proc_stat: PathBuf,
    pub proc_meminfo: PathBuf,
    pub proc_diskstats: PathBuf,
    pub proc_net_dev: PathBuf,
    pub boot_id: PathBuf,
    pub hostname: PathBuf,
    pub filesystem: PathBuf,
}

impl Default for Paths {
    fn default() -> Self {
        Self {
            proc_stat: "/proc/stat".into(),
            proc_meminfo: "/proc/meminfo".into(),
            proc_diskstats: "/proc/diskstats".into(),
            proc_net_dev: "/proc/net/dev".into(),
            boot_id: "/proc/sys/kernel/random/boot_id".into(),
            hostname: "/proc/sys/kernel/hostname".into(),
            filesystem: "/".into(),
        }
    }
}

fn bad(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn read_text(path: &Path, cap: u64) -> io::Result<String> {
    let mut bytes = Vec::new();
    File::open(path)?.take(cap + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > cap {
        return Err(bad("host source exceeds byte cap"));
    }
    String::from_utf8(bytes).map_err(|_| bad("host source is not UTF-8"))
}

fn number(text: &str) -> io::Result<u64> {
    text.parse().map_err(|_| bad("invalid host counter"))
}

fn bytes_from_kib(value: u64) -> io::Result<u64> {
    value
        .checked_mul(1024)
        .ok_or_else(|| bad("memory byte count overflow"))
}

fn point(
    name: &'static str,
    unit: &'static str,
    kind: Kind,
    source: &str,
    value: u64,
    known_start_ns: Option<u64>,
) -> Point {
    Point {
        name,
        unit,
        kind,
        source: source.to_owned(),
        value: Value::Int(value),
        known_start_ns,
    }
}

fn filesystem_bytes(path: &Path) -> io::Result<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let name = CString::new(path.as_os_str().as_bytes()).map_err(|_| bad("NUL filesystem path"))?;
    let mut info = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: the CString is NUL-terminated and info points to writable storage.
    if unsafe { libc::statvfs(name.as_ptr(), info.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: statvfs returned success and initialized info.
    let info = unsafe { info.assume_init() };
    let size = if info.f_frsize == 0 {
        info.f_bsize
    } else {
        info.f_frsize
    };
    let total = info
        .f_blocks
        .checked_mul(size)
        .ok_or_else(|| bad("filesystem size overflow"))?;
    let available = info
        .f_bavail
        .checked_mul(size)
        .ok_or_else(|| bad("filesystem available overflow"))?;
    Ok((total, available))
}

/// Host name and boot ID, the resource identity of every batch.
pub fn identity(paths: &Paths) -> io::Result<(String, String)> {
    let hostname = read_text(&paths.hostname, 256)?.trim().to_owned();
    let boot_id = read_text(&paths.boot_id, 128)?.trim().to_owned();
    if hostname.is_empty() || hostname.len() > 255 || boot_id.len() != 36 {
        return Err(bad("invalid host identity"));
    }
    Ok((hostname, boot_id))
}

pub fn sample(paths: &Paths) -> io::Result<Snapshot> {
    let (hostname, boot_id) = identity(paths)?;
    let mut points = Vec::with_capacity(200);
    let stat = read_text(&paths.proc_stat, MAX_PROC_BYTES)?;
    let cpu = stat.lines().next().ok_or_else(|| bad("missing CPU line"))?;
    let mut fields = cpu.split_whitespace();
    if fields.next() != Some("cpu") {
        return Err(bad("missing aggregate CPU"));
    }
    let ticks: Vec<u64> = fields.take(10).map(number).collect::<io::Result<_>>()?;
    if ticks.len() < 8 {
        return Err(bad("short aggregate CPU line"));
    }
    let boot_seconds = stat
        .lines()
        .find_map(|line| line.strip_prefix("btime "))
        .ok_or_else(|| bad("missing boot time"))
        .and_then(number)?;
    let start = boot_seconds
        .checked_mul(1_000_000_000)
        .ok_or_else(|| bad("boot time overflow"))?;
    // SAFETY: sysconf takes a fixed process configuration key and no pointer.
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if hz <= 0 {
        return Err(bad("invalid clock tick rate"));
    }
    let sums = [
        ("user", ticks[0].checked_add(ticks[1])),
        (
            "system",
            ticks[2]
                .checked_add(ticks[5])
                .and_then(|v| v.checked_add(ticks[6])),
        ),
        ("idle", ticks[3].checked_add(ticks[4])),
    ];
    for (state, ticks) in sums {
        let ticks = ticks.ok_or_else(|| bad("CPU counter overflow"))?;
        points.push(Point {
            name: "system.cpu.time",
            unit: "s",
            kind: Kind::Counter,
            source: state.into(),
            value: Value::Double(ticks as f64 / hz as f64),
            known_start_ns: Some(start),
        });
    }

    let mem = read_text(&paths.proc_meminfo, 16 * 1024)?;
    for (field, name) in [
        ("MemTotal:", "system.memory.total"),
        ("MemAvailable:", "system.memory.available"),
    ] {
        let line = mem
            .lines()
            .find(|line| line.starts_with(field))
            .ok_or_else(|| bad("missing memory field"))?;
        let mut words = line.split_whitespace();
        words.next();
        let value = number(words.next().ok_or_else(|| bad("short memory field"))?)?;
        if words.next() != Some("kB") {
            return Err(bad("unexpected memory unit"));
        }
        points.push(point(
            name,
            "By",
            Kind::Gauge,
            "host",
            bytes_from_kib(value)?,
            None,
        ));
    }
    let (total, available) = filesystem_bytes(&paths.filesystem)?;
    points.push(point(
        "system.filesystem.capacity",
        "By",
        Kind::Gauge,
        "/",
        total,
        None,
    ));
    points.push(point(
        "system.filesystem.available",
        "By",
        Kind::Gauge,
        "/",
        available,
        None,
    ));

    let disks = read_text(&paths.proc_diskstats, MAX_PROC_BYTES)?;
    let mut disk_count = 0;
    for line in disks.lines() {
        let fields: Vec<_> = line.split_whitespace().take(10).collect();
        if fields.len() < 10 {
            return Err(bad("short diskstats row"));
        }
        disk_count += 1;
        if disk_count > MAX_DEVICES || fields[2].len() > 64 {
            return Err(bad("disk cardinality cap"));
        }
        for (index, name) in [
            (5, "system.disk.read.bytes"),
            (9, "system.disk.write.bytes"),
        ] {
            let value = number(fields[index])?
                .checked_mul(512)
                .ok_or_else(|| bad("disk byte overflow"))?;
            points.push(point(name, "By", Kind::Counter, fields[2], value, None));
        }
    }
    let net = read_text(&paths.proc_net_dev, MAX_PROC_BYTES)?;
    let mut net_count = 0;
    for line in net.lines().skip(2) {
        let (interface, counters) = line
            .split_once(':')
            .ok_or_else(|| bad("invalid network row"))?;
        let interface = interface.trim();
        let fields: Vec<_> = counters.split_whitespace().take(16).collect();
        if fields.len() < 16 {
            return Err(bad("short network row"));
        }
        net_count += 1;
        if net_count > MAX_DEVICES || interface.len() > 64 {
            return Err(bad("network cardinality cap"));
        }
        points.push(point(
            "system.network.receive.bytes",
            "By",
            Kind::Counter,
            interface,
            number(fields[0])?,
            None,
        ));
        points.push(point(
            "system.network.transmit.bytes",
            "By",
            Kind::Counter,
            interface,
            number(fields[8])?,
            None,
        ));
    }
    Ok(Snapshot {
        hostname,
        boot_id,
        points,
    })
}
