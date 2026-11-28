//! Read-only cgroup v2 device-memory observations for the current process.
//!
//! The kernel exposes `dmem.capacity` only in the root cgroup and
//! `dmem.current`, `dmem.peak`, and `dmem.max` only in non-root cgroups.
//! Region identifiers are retained for display and ordering; they are never
//! interpreted as filesystem paths.

use rustix::{
    fs::{Mode, OFlags, open},
    io::Errno,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
};

const PROC_CGROUP_MAX_BYTES: usize = 4096;
const PROC_MOUNTINFO_MAX_BYTES: usize = 128 * 1024;
const PROC_MOUNTINFO_MAX_LINES: usize = 2048;
const CGROUP_TYPE_MAX_BYTES: usize = 64;
const CGROUP_PROCS_MAX_BYTES: usize = 1024 * 1024;
const CGROUP_PROCS_MAX_ENTRIES: usize = 65_536;
const DMEM_FILE_MAX_BYTES: usize = 64 * 1024;
const DMEM_MAX_REGIONS: usize = 256;
const DMEM_MAX_REGION_ID_BYTES: usize = 256;
const CGROUP_MAX_COMPONENT_BYTES: usize = 255;

/// Whether the current process belongs to the cgroup v2 hierarchy root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceMemoryScope {
    Root,
    NonRoot,
}

/// A configured `dmem.max` value, preserving the kernel's unbounded sentinel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceMemoryMax {
    Bytes(u64),
    Unbounded,
}

/// One device-memory region. `None` means that the relevant file or region key
/// was absent; a kernel-reported zero remains `Some(0)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceMemoryRegion {
    /// Stable display identifier from the kernel's nested-keyed file.
    pub identifier: String,
    /// Present only for root-cgroup probes.
    pub capacity_bytes: Option<u64>,
    /// Present only for non-root probes.
    pub current_bytes: Option<u64>,
    /// Present only for non-root probes.
    pub peak_bytes: Option<u64>,
    /// Present only for non-root probes; `Unbounded` represents the literal `max`.
    pub max_bytes: Option<DeviceMemoryMax>,
}

/// A bounded snapshot of the current process's cgroup v2 device-memory files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceMemorySnapshot {
    /// The process's normalized cgroup v2 membership path, for display only.
    pub cgroup_path: String,
    pub scope: DeviceMemoryScope,
    /// Regions are sorted lexicographically by `identifier`.
    pub regions: Vec<DeviceMemoryRegion>,
}

/// A malformed or unreadable cgroup input. Missing optional interface files and
/// missing region keys are represented as `None`, not as zero.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeviceMemoryProbeError {
    Io {
        file: &'static str,
        kind: io::ErrorKind,
    },
    MissingUnifiedMembership,
    DuplicateUnifiedMembership,
    InvalidUnifiedMembership,
    InvalidMountInfo {
        line: usize,
    },
    MissingCgroup2Mount,
    AmbiguousCgroup2Mount,
    Cgroup2MountRootMismatch,
    Cgroup2MountpointMismatch,
    CgroupDirectoryNotDirectory,
    ThreadedCgroup,
    InvalidProcessList {
        line: usize,
    },
    TooManyProcesses,
    ProcessNotInCgroup,
    CgroupMembershipChanged,
    FileTooLarge {
        file: &'static str,
    },
    InvalidUtf8 {
        file: &'static str,
    },
    TooManyRegions {
        file: &'static str,
    },
    InvalidRegionIdentifier {
        file: &'static str,
        line: usize,
    },
    DuplicateRegion {
        file: &'static str,
        line: usize,
    },
    InvalidRecord {
        file: &'static str,
        line: usize,
    },
    InvalidValue {
        file: &'static str,
        line: usize,
    },
}

impl fmt::Display for DeviceMemoryProbeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { file, kind } => write!(f, "failed reading {file}: {kind}"),
            Self::MissingUnifiedMembership => {
                f.write_str("current process has no cgroup v2 membership")
            }
            Self::DuplicateUnifiedMembership => {
                f.write_str("current process has duplicate cgroup v2 memberships")
            }
            Self::InvalidUnifiedMembership => {
                f.write_str("current process has invalid cgroup v2 membership")
            }
            Self::InvalidMountInfo { line } => {
                write!(f, "invalid cgroup mount information on line {line}")
            }
            Self::MissingCgroup2Mount => f.write_str("no compatible cgroup v2 mount is visible"),
            Self::AmbiguousCgroup2Mount => {
                f.write_str("cgroup v2 mount is overmounted or ambiguous")
            }
            Self::Cgroup2MountRootMismatch => {
                f.write_str("cgroup v2 mount root is not the current hierarchy root")
            }
            Self::Cgroup2MountpointMismatch => {
                f.write_str("cgroup v2 is not uniquely mounted at /sys/fs/cgroup")
            }
            Self::CgroupDirectoryNotDirectory => {
                f.write_str("cgroup v2 membership does not resolve to a real directory")
            }
            Self::ThreadedCgroup => {
                f.write_str("threaded cgroups do not provide unambiguous process membership")
            }
            Self::InvalidProcessList { line } => {
                write!(f, "invalid cgroup.procs entry on line {line}")
            }
            Self::TooManyProcesses => f.write_str("cgroup.procs exceeds its entry limit"),
            Self::ProcessNotInCgroup => {
                f.write_str("current process is absent from its reported cgroup.procs")
            }
            Self::CgroupMembershipChanged => {
                f.write_str("current process cgroup membership changed during the probe")
            }
            Self::FileTooLarge { file } => write!(f, "{file} exceeds its byte limit"),
            Self::InvalidUtf8 { file } => write!(f, "{file} is not valid UTF-8"),
            Self::TooManyRegions { file } => write!(f, "{file} exceeds its region limit"),
            Self::InvalidRegionIdentifier { file, line } => {
                write!(f, "invalid region identifier in {file} line {line}")
            }
            Self::DuplicateRegion { file, line } => {
                write!(f, "duplicate region in {file} line {line}")
            }
            Self::InvalidRecord { file, line } => {
                write!(f, "invalid nested-keyed record in {file} line {line}")
            }
            Self::InvalidValue { file, line } => {
                write!(f, "invalid device-memory value in {file} line {line}")
            }
        }
    }
}

impl std::error::Error for DeviceMemoryProbeError {}

/// Reads the current process's actual `/proc/self/cgroup` and `/sys/fs/cgroup`.
///
/// Returns `Ok(None)` when the applicable `dmem.*` files are absent. The probe
/// is opt-in and does not activate collection or emit observations.
pub fn probe_device_memory() -> Result<Option<DeviceMemorySnapshot>, DeviceMemoryProbeError> {
    probe_from_roots(
        Path::new("/proc"),
        Path::new("/sys/fs/cgroup"),
        std::process::id(),
    )
}

fn probe_from_roots(
    proc_root: &Path,
    cgroup_root: &Path,
    pid: u32,
) -> Result<Option<DeviceMemorySnapshot>, DeviceMemoryProbeError> {
    validate_cgroup2_mount(proc_root)?;
    let cgroup_path = read_process_cgroup_path(proc_root)?;
    let cgroup_dir = resolve_cgroup_dir(cgroup_root, &cgroup_path)?;
    verify_process_membership(&cgroup_dir, pid)?;
    let scope = if cgroup_path == "/" {
        DeviceMemoryScope::Root
    } else {
        DeviceMemoryScope::NonRoot
    };

    let (capacity, current, peak, max) = match scope {
        DeviceMemoryScope::Root => (
            read_nested_file(&cgroup_dir, "dmem.capacity", false)?,
            None,
            None,
            None,
        ),
        DeviceMemoryScope::NonRoot => (
            None,
            read_nested_file(&cgroup_dir, "dmem.current", false)?,
            read_nested_file(&cgroup_dir, "dmem.peak", false)?,
            read_nested_file(&cgroup_dir, "dmem.max", true)?,
        ),
    };
    let snapshot = {
        let any_file_present =
            capacity.is_some() || current.is_some() || peak.is_some() || max.is_some();
        if !any_file_present {
            None
        } else {
            let mut identifiers = BTreeSet::new();
            for (file, values) in [
                ("dmem.capacity", capacity.as_ref()),
                ("dmem.current", current.as_ref()),
                ("dmem.peak", peak.as_ref()),
                ("dmem.max", max.as_ref()),
            ] {
                if let Some(values) = values {
                    for identifier in values.keys() {
                        identifiers.insert(identifier.clone());
                        if identifiers.len() > DMEM_MAX_REGIONS {
                            return Err(DeviceMemoryProbeError::TooManyRegions { file });
                        }
                    }
                }
            }
            let regions = identifiers
                .into_iter()
                .map(|identifier| DeviceMemoryRegion {
                    capacity_bytes: lookup_bytes(capacity.as_ref(), &identifier),
                    current_bytes: lookup_bytes(current.as_ref(), &identifier),
                    peak_bytes: lookup_bytes(peak.as_ref(), &identifier),
                    max_bytes: lookup_max(max.as_ref(), &identifier),
                    identifier,
                })
                .collect();
            Some(DeviceMemorySnapshot {
                cgroup_path: cgroup_path.clone(),
                scope,
                regions,
            })
        }
    };
    if read_process_cgroup_path(proc_root)? != cgroup_path {
        return Err(DeviceMemoryProbeError::CgroupMembershipChanged);
    }
    verify_process_membership(&cgroup_dir, pid)?;
    validate_cgroup2_mount(proc_root)?;
    Ok(snapshot)
}

fn read_process_cgroup_path(proc_root: &Path) -> Result<String, DeviceMemoryProbeError> {
    let membership = read_required_bounded(
        &proc_root.join("self/cgroup"),
        "/proc/self/cgroup",
        PROC_CGROUP_MAX_BYTES,
    )?;
    parse_unified_cgroup_path(&membership)
}

fn validate_cgroup2_mount(proc_root: &Path) -> Result<(), DeviceMemoryProbeError> {
    let raw = read_required_bounded(
        &proc_root.join("self/mountinfo"),
        "/proc/self/mountinfo",
        PROC_MOUNTINFO_MAX_BYTES,
    )?;
    parse_mountinfo(&raw)
}

fn verify_process_membership(cgroup_dir: &Path, pid: u32) -> Result<(), DeviceMemoryProbeError> {
    let cgroup_type = read_required_bounded(
        &cgroup_dir.join("cgroup.type"),
        "cgroup.type",
        CGROUP_TYPE_MAX_BYTES,
    )?;
    if cgroup_type != "domain\n" {
        return Err(DeviceMemoryProbeError::ThreadedCgroup);
    }

    let processes = read_required_bounded(
        &cgroup_dir.join("cgroup.procs"),
        "cgroup.procs",
        CGROUP_PROCS_MAX_BYTES,
    )?;
    if !processes.is_empty() && !processes.ends_with('\n') {
        return Err(DeviceMemoryProbeError::InvalidProcessList {
            line: processes.lines().count(),
        });
    }
    let mut found = false;
    for (index, line) in processes.lines().enumerate() {
        let line_number = index + 1;
        if line_number > CGROUP_PROCS_MAX_ENTRIES {
            return Err(DeviceMemoryProbeError::TooManyProcesses);
        }
        let process_id = line
            .parse::<u32>()
            .ok()
            .filter(|process_id| *process_id != 0)
            .ok_or(DeviceMemoryProbeError::InvalidProcessList { line: line_number })?;
        found |= process_id == pid;
    }
    if !found {
        return Err(DeviceMemoryProbeError::ProcessNotInCgroup);
    }
    Ok(())
}

fn parse_mountinfo(raw: &str) -> Result<(), DeviceMemoryProbeError> {
    if !raw.is_empty() && !raw.ends_with('\n') {
        return Err(DeviceMemoryProbeError::InvalidMountInfo {
            line: raw.lines().count(),
        });
    }
    let mut cgroup2 = Vec::new();
    let mut expected_mounts = Vec::new();
    for (index, line) in raw.lines().enumerate() {
        let line_number = index + 1;
        if line_number > PROC_MOUNTINFO_MAX_LINES {
            return Err(DeviceMemoryProbeError::InvalidMountInfo { line: line_number });
        }
        let fields: Vec<_> = line.split_whitespace().collect();
        let separators: Vec<_> = fields
            .iter()
            .enumerate()
            .filter_map(|(index, field)| (*field == "-").then_some(index))
            .collect();
        if separators.len() != 1
            || separators[0] < 6
            || fields.len() < separators[0] + 4
            || fields.len() < 5
        {
            return Err(DeviceMemoryProbeError::InvalidMountInfo { line: line_number });
        }
        let mount = (fields[3], fields[4], fields[separators[0] + 1]);
        if mount.1 == "/sys/fs/cgroup" {
            expected_mounts.push(mount);
        }
        if mount.2 == "cgroup2" {
            cgroup2.push(mount);
        }
    }
    if cgroup2.is_empty() {
        return Err(DeviceMemoryProbeError::MissingCgroup2Mount);
    }
    if cgroup2.len() != 1 || expected_mounts.len() != 1 {
        return Err(DeviceMemoryProbeError::AmbiguousCgroup2Mount);
    }
    let (root, mountpoint, filesystem) = cgroup2[0];
    if filesystem != "cgroup2" || mountpoint != "/sys/fs/cgroup" {
        return Err(DeviceMemoryProbeError::Cgroup2MountpointMismatch);
    }
    if root != "/" {
        return Err(DeviceMemoryProbeError::Cgroup2MountRootMismatch);
    }
    if expected_mounts[0] != cgroup2[0] {
        return Err(DeviceMemoryProbeError::AmbiguousCgroup2Mount);
    }
    Ok(())
}

fn parse_unified_cgroup_path(raw: &str) -> Result<String, DeviceMemoryProbeError> {
    if !raw.is_empty() && !raw.ends_with('\n') {
        return Err(DeviceMemoryProbeError::InvalidUnifiedMembership);
    }
    let mut unified = None;
    for line in raw.lines() {
        if let Some(path) = line.strip_prefix("0::")
            && unified.replace(path).is_some()
        {
            return Err(DeviceMemoryProbeError::DuplicateUnifiedMembership);
        }
    }
    let path = unified.ok_or(DeviceMemoryProbeError::MissingUnifiedMembership)?;
    if path.is_empty()
        || path.len() > PROC_CGROUP_MAX_BYTES
        || !path.starts_with('/')
        || path.contains('\0')
        || path.chars().any(char::is_control)
    {
        return Err(DeviceMemoryProbeError::InvalidUnifiedMembership);
    }
    if path == "/" {
        return Ok(path.to_owned());
    }
    let tail = path
        .strip_prefix('/')
        .ok_or(DeviceMemoryProbeError::InvalidUnifiedMembership)?;
    if tail.is_empty() || tail.ends_with('/') {
        return Err(DeviceMemoryProbeError::InvalidUnifiedMembership);
    }
    for component in tail.split('/') {
        if component.is_empty()
            || component == "."
            || component == ".."
            || component.len() > CGROUP_MAX_COMPONENT_BYTES
        {
            return Err(DeviceMemoryProbeError::InvalidUnifiedMembership);
        }
    }
    Ok(path.to_owned())
}

fn resolve_cgroup_dir(root: &Path, cgroup_path: &str) -> Result<PathBuf, DeviceMemoryProbeError> {
    let root_meta =
        std::fs::symlink_metadata(root).map_err(|error| io_error("cgroup root", error))?;
    if root_meta.file_type().is_symlink() || !root_meta.is_dir() {
        return Err(DeviceMemoryProbeError::CgroupDirectoryNotDirectory);
    }
    let mut result = root.to_path_buf();
    if cgroup_path != "/" {
        for component in cgroup_path
            .strip_prefix('/')
            .ok_or(DeviceMemoryProbeError::InvalidUnifiedMembership)?
            .split('/')
        {
            result.push(component);
            let metadata = std::fs::symlink_metadata(&result)
                .map_err(|error| io_error("cgroup directory", error))?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(DeviceMemoryProbeError::CgroupDirectoryNotDirectory);
            }
        }
    }
    Ok(result)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ParsedValue {
    Bytes(u64),
    Unbounded,
}

fn read_nested_file(
    cgroup_dir: &Path,
    filename: &'static str,
    allow_unbounded: bool,
) -> Result<Option<BTreeMap<String, ParsedValue>>, DeviceMemoryProbeError> {
    let path = cgroup_dir.join(filename);
    let Some(text) = read_optional_bounded(&path, filename, DMEM_FILE_MAX_BYTES)? else {
        return Ok(None);
    };
    parse_nested_file(&text, filename, allow_unbounded).map(Some)
}

fn parse_nested_file(
    text: &str,
    filename: &'static str,
    allow_unbounded: bool,
) -> Result<BTreeMap<String, ParsedValue>, DeviceMemoryProbeError> {
    if !text.is_empty() && !text.ends_with('\n') {
        return Err(DeviceMemoryProbeError::InvalidRecord {
            file: filename,
            line: text.lines().count(),
        });
    }
    let mut values = BTreeMap::new();
    for (index, line) in text.lines().enumerate() {
        let line_number = index + 1;
        let mut fields = line.split_whitespace();
        let Some(identifier) = fields.next() else {
            return Err(DeviceMemoryProbeError::InvalidRecord {
                file: filename,
                line: line_number,
            });
        };
        let Some(raw_value) = fields.next() else {
            return Err(DeviceMemoryProbeError::InvalidRecord {
                file: filename,
                line: line_number,
            });
        };
        if fields.next().is_some() {
            return Err(DeviceMemoryProbeError::InvalidRecord {
                file: filename,
                line: line_number,
            });
        }
        if !valid_region_identifier(identifier) {
            return Err(DeviceMemoryProbeError::InvalidRegionIdentifier {
                file: filename,
                line: line_number,
            });
        }
        let value = if allow_unbounded && raw_value == "max" {
            ParsedValue::Unbounded
        } else {
            ParsedValue::Bytes(raw_value.parse().map_err(|_| {
                DeviceMemoryProbeError::InvalidValue {
                    file: filename,
                    line: line_number,
                }
            })?)
        };
        if values.insert(identifier.to_owned(), value).is_some() {
            return Err(DeviceMemoryProbeError::DuplicateRegion {
                file: filename,
                line: line_number,
            });
        }
        if values.len() > DMEM_MAX_REGIONS {
            return Err(DeviceMemoryProbeError::TooManyRegions { file: filename });
        }
    }
    Ok(values)
}

fn valid_region_identifier(identifier: &str) -> bool {
    if identifier.is_empty()
        || identifier.len() > DMEM_MAX_REGION_ID_BYTES
        || !identifier.bytes().all(|byte| byte.is_ascii_graphic())
    {
        return false;
    }
    !identifier
        .split('/')
        .any(|component| component.is_empty() || component == "." || component == "..")
}

fn lookup_bytes(values: Option<&BTreeMap<String, ParsedValue>>, identifier: &str) -> Option<u64> {
    match values?.get(identifier)? {
        ParsedValue::Bytes(bytes) => Some(*bytes),
        ParsedValue::Unbounded => None,
    }
}

fn lookup_max(
    values: Option<&BTreeMap<String, ParsedValue>>,
    identifier: &str,
) -> Option<DeviceMemoryMax> {
    match values?.get(identifier)? {
        ParsedValue::Bytes(bytes) => Some(DeviceMemoryMax::Bytes(*bytes)),
        ParsedValue::Unbounded => Some(DeviceMemoryMax::Unbounded),
    }
}

fn read_required_bounded(
    path: &Path,
    file: &'static str,
    max_bytes: usize,
) -> Result<String, DeviceMemoryProbeError> {
    read_bounded(path, file, max_bytes)?.ok_or(DeviceMemoryProbeError::Io {
        file,
        kind: io::ErrorKind::NotFound,
    })
}

fn read_optional_bounded(
    path: &Path,
    file: &'static str,
    max_bytes: usize,
) -> Result<Option<String>, DeviceMemoryProbeError> {
    read_bounded(path, file, max_bytes)
}

fn read_bounded(
    path: &Path,
    file: &'static str,
    max_bytes: usize,
) -> Result<Option<String>, DeviceMemoryProbeError> {
    let fd = match open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(Errno::NOENT) => return Ok(None),
        Err(error) => return Err(io_error(file, io::Error::from(error))),
    };
    let mut bytes = Vec::new();
    File::from(fd)
        .take(max_bytes as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| io_error(file, error))?;
    if bytes.len() > max_bytes {
        return Err(DeviceMemoryProbeError::FileTooLarge { file });
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| DeviceMemoryProbeError::InvalidUtf8 { file })
}

fn io_error(file: &'static str, error: io::Error) -> DeviceMemoryProbeError {
    DeviceMemoryProbeError::Io {
        file,
        kind: error.kind(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct Fixture {
        _temp: tempfile::TempDir,
        proc_root: PathBuf,
        cgroup_root: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::Builder::new()
                .prefix("fabrico11y-device-memory-")
                .tempdir_in(std::env::var_os("TMPDIR").expect("disk-backed TMPDIR"))
                .unwrap();
            let root = temp.path().to_path_buf();
            let proc_root = root.join("proc");
            let cgroup_root = root.join("sys/fs/cgroup");
            fs::create_dir_all(proc_root.join("self")).unwrap();
            fs::create_dir_all(&cgroup_root).unwrap();
            fs::write(
                proc_root.join("self/mountinfo"),
                "35 25 0:29 / /sys/fs/cgroup rw,nosuid,nodev,noexec - cgroup2 cgroup rw\n",
            )
            .unwrap();
            Self {
                _temp: temp,
                proc_root,
                cgroup_root,
            }
        }

        fn membership(&self, path: &str) {
            fs::write(self.proc_root.join("self/cgroup"), format!("0::{path}\n")).unwrap();
        }

        fn cgroup(&self, path: &str) -> PathBuf {
            let dir = if path == "/" {
                self.cgroup_root.clone()
            } else {
                self.cgroup_root.join(path.trim_start_matches('/'))
            };
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("cgroup.type"), "domain\n").unwrap();
            fs::write(
                dir.join("cgroup.procs"),
                format!("{}\n", std::process::id()),
            )
            .unwrap();
            dir
        }

        fn probe(&self) -> Result<Option<DeviceMemorySnapshot>, DeviceMemoryProbeError> {
            self.probe_as(std::process::id())
        }

        fn probe_as(
            &self,
            pid: u32,
        ) -> Result<Option<DeviceMemorySnapshot>, DeviceMemoryProbeError> {
            probe_from_roots(&self.proc_root, &self.cgroup_root, pid)
        }

        fn mountinfo(&self, text: &str) {
            fs::write(self.proc_root.join("self/mountinfo"), text).unwrap();
        }
    }

    #[test]
    fn root_reads_capacity_only_and_preserves_zero_and_order() {
        let fixture = Fixture::new();
        fixture.membership("/");
        fs::write(
            fixture.cgroup("/").join("dmem.capacity"),
            "drm/0000:03:00.0/vram1 0\ndrm/0000:03:00.0/vram0 4096\n",
        )
        .unwrap();
        let snapshot = fixture.probe().unwrap().unwrap();
        assert_eq!(snapshot.scope, DeviceMemoryScope::Root);
        assert_eq!(snapshot.cgroup_path, "/");
        assert_eq!(snapshot.regions[0].identifier, "drm/0000:03:00.0/vram0");
        assert_eq!(snapshot.regions[0].capacity_bytes, Some(4096));
        assert_eq!(snapshot.regions[0].current_bytes, None);
        assert_eq!(snapshot.regions[1].capacity_bytes, Some(0));
        assert_eq!(snapshot.regions[1].max_bytes, None);
    }

    #[test]
    fn child_reads_current_peak_and_bounded_or_unbounded_max() {
        let fixture = Fixture::new();
        fixture.membership("/user.slice/job.scope");
        let dir = fixture.cgroup("/user.slice/job.scope");
        fs::write(dir.join("dmem.current"), "gpu/vram0 0\ngpu/stolen 8\n").unwrap();
        fs::write(dir.join("dmem.peak"), "gpu/vram0 32\n").unwrap();
        fs::write(dir.join("dmem.max"), "gpu/vram0 max\ngpu/stolen 0\n").unwrap();
        let snapshot = fixture.probe().unwrap().unwrap();
        assert_eq!(snapshot.scope, DeviceMemoryScope::NonRoot);
        assert_eq!(snapshot.regions.len(), 2);
        assert_eq!(
            snapshot
                .regions
                .iter()
                .map(|region| region.identifier.as_str())
                .collect::<Vec<_>>(),
            ["gpu/stolen", "gpu/vram0"]
        );
        let vram = snapshot
            .regions
            .iter()
            .find(|region| region.identifier == "gpu/vram0")
            .unwrap();
        let stolen = snapshot
            .regions
            .iter()
            .find(|region| region.identifier == "gpu/stolen")
            .unwrap();
        assert_eq!(vram.current_bytes, Some(0));
        assert_eq!(vram.peak_bytes, Some(32));
        assert_eq!(vram.max_bytes, Some(DeviceMemoryMax::Unbounded));
        assert_eq!(stolen.peak_bytes, None);
        assert_eq!(stolen.max_bytes, Some(DeviceMemoryMax::Bytes(0)));
        assert_eq!(vram.capacity_bytes, None);
    }

    #[test]
    fn missing_interface_is_none_but_unreadable_or_malformed_is_an_error() {
        let fixture = Fixture::new();
        fixture.membership("/child");
        let dir = fixture.cgroup("/child");
        assert_eq!(fixture.probe().unwrap(), None);

        fs::create_dir(dir.join("dmem.current")).unwrap();
        assert!(matches!(
            fixture.probe(),
            Err(DeviceMemoryProbeError::Io { .. })
        ));
        fs::remove_dir(dir.join("dmem.current")).unwrap();
        fs::write(dir.join("dmem.current"), "gpu/vram0 1\ngpu/vram0 2\n").unwrap();
        assert!(matches!(
            fixture.probe(),
            Err(DeviceMemoryProbeError::DuplicateRegion { .. })
        ));
    }

    #[test]
    fn oversized_dmem_file_is_rejected_at_eof_plus_one() {
        let fixture = Fixture::new();
        fixture.membership("/child");
        let dir = fixture.cgroup("/child");
        fs::write(
            dir.join("dmem.current"),
            vec![b'x'; DMEM_FILE_MAX_BYTES + 1],
        )
        .unwrap();
        assert_eq!(
            fixture.probe(),
            Err(DeviceMemoryProbeError::FileTooLarge {
                file: "dmem.current"
            })
        );
    }

    #[test]
    fn wrong_mount_root_rejects_same_named_cgroup_directory() {
        let fixture = Fixture::new();
        fixture.membership("/child");
        let dir = fixture.cgroup("/child");
        fs::write(dir.join("dmem.current"), "gpu/vram0 1\n").unwrap();
        fixture.mountinfo("35 25 0:29 /host-slice /sys/fs/cgroup rw - cgroup2 cgroup rw\n");
        assert_eq!(
            fixture.probe(),
            Err(DeviceMemoryProbeError::Cgroup2MountRootMismatch)
        );
    }

    #[test]
    fn duplicate_and_overmounted_cgroup_mounts_are_refused() {
        let fixture = Fixture::new();
        fixture.membership("/child");
        fixture.cgroup("/child");
        fixture.mountinfo(
            "35 25 0:29 / /sys/fs/cgroup rw - cgroup2 cgroup rw\n36 25 0:30 / /sys/fs/cgroup rw - tmpfs tmpfs rw\n",
        );
        assert_eq!(
            fixture.probe(),
            Err(DeviceMemoryProbeError::AmbiguousCgroup2Mount)
        );
        fixture.mountinfo("35 25 0:29 / /somewhere-else rw - cgroup2 cgroup rw\n");
        assert!(matches!(
            fixture.probe(),
            Err(DeviceMemoryProbeError::AmbiguousCgroup2Mount
                | DeviceMemoryProbeError::Cgroup2MountpointMismatch)
        ));
    }

    #[test]
    fn process_presence_and_domain_type_are_required() {
        let fixture = Fixture::new();
        fixture.membership("/child");
        let dir = fixture.cgroup("/child");
        fs::write(dir.join("cgroup.procs"), "99999999\n").unwrap();
        assert_eq!(
            fixture.probe(),
            Err(DeviceMemoryProbeError::ProcessNotInCgroup)
        );
        fs::remove_file(dir.join("cgroup.procs")).unwrap();
        assert_eq!(
            fixture.probe(),
            Err(DeviceMemoryProbeError::Io {
                file: "cgroup.procs",
                kind: io::ErrorKind::NotFound,
            })
        );
        fs::write(
            dir.join("cgroup.procs"),
            format!("{}\n", std::process::id()),
        )
        .unwrap();
        fs::write(dir.join("cgroup.type"), "domain threaded\n").unwrap();
        assert_eq!(fixture.probe(), Err(DeviceMemoryProbeError::ThreadedCgroup));
        fs::write(dir.join("cgroup.type"), "domain\n").unwrap();
        assert_eq!(
            fixture.probe_as(std::process::id().saturating_add(1)),
            Err(DeviceMemoryProbeError::ProcessNotInCgroup)
        );
    }

    #[test]
    fn membership_and_input_bounds_fail_closed() {
        assert_eq!(
            parse_unified_cgroup_path("0::/a/../b"),
            Err(DeviceMemoryProbeError::InvalidUnifiedMembership)
        );
        assert_eq!(
            parse_unified_cgroup_path("0::/a//b"),
            Err(DeviceMemoryProbeError::InvalidUnifiedMembership)
        );
        assert_eq!(
            parse_unified_cgroup_path("0::/child"),
            Err(DeviceMemoryProbeError::InvalidUnifiedMembership)
        );
        assert_eq!(
            parse_unified_cgroup_path("2:cpu:/legacy\n0::/child\n"),
            Ok("/child".into())
        );
        assert!(matches!(
            parse_nested_file("gpu/vram0 18446744073709551616\n", "dmem.current", false),
            Err(DeviceMemoryProbeError::InvalidValue { .. })
        ));
        assert!(matches!(
            parse_nested_file("gpu/../vram0 1\n", "dmem.current", false),
            Err(DeviceMemoryProbeError::InvalidRegionIdentifier { .. })
        ));
        assert!(matches!(
            parse_nested_file("gpu/vram0 max\n", "dmem.current", false),
            Err(DeviceMemoryProbeError::InvalidValue { .. })
        ));
        assert!(matches!(
            parse_nested_file("gpu/vram0 1", "dmem.current", false),
            Err(DeviceMemoryProbeError::InvalidRecord { .. })
        ));
        assert!(matches!(
            parse_nested_file(
                &format!("{} 1\n", "x".repeat(DMEM_MAX_REGION_ID_BYTES + 1)),
                "dmem.current",
                false
            ),
            Err(DeviceMemoryProbeError::InvalidRegionIdentifier { .. })
        ));
        let too_many = (0..=DMEM_MAX_REGIONS)
            .map(|index| format!("gpu/region{index} 1\n"))
            .collect::<String>();
        assert!(matches!(
            parse_nested_file(&too_many, "dmem.current", false),
            Err(DeviceMemoryProbeError::TooManyRegions { .. })
        ));
    }
}
