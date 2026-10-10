//! Private, bounded process diagnostics, independent of collection/delivery.
//!
//! A composition root owns the sampling interval. Do not emit an event for
//! every collected or delivered Batch: collecting this log would feed itself.

use std::ffi::CString;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_FILE_BYTES: u64 = 256 * 1024;
const MAX_LINE_BYTES: usize = 4096;
const MAX_PROC_BYTES: u64 = 16 * 1024;

/// One writer per private directory; active log and one bounded old log.
/// Diagnostics are best effort: errors are returned without affecting custody.
pub struct OperationalLog {
    directory: File,
    path: PathBuf,
    name: CString,
    old_name: CString,
    component: String,
    writer: Mutex<Option<File>>,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

fn identifier(value: &str) -> io::Result<()> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-.".contains(&byte))
    {
        return Err(invalid(
            "diagnostic identifier must be 1..64 safe ASCII bytes",
        ));
    }
    Ok(())
}

fn private_owned(metadata: &fs::Metadata) -> io::Result<()> {
    // SAFETY: geteuid takes no arguments and has no pointer preconditions.
    if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
        return Err(invalid(
            "diagnostic storage must be private and owned by this user",
        ));
    }
    Ok(())
}

fn open_log(directory: &File, name: &CString) -> io::Result<File> {
    // SAFETY: directory is a live descriptor and name is NUL-terminated.
    let descriptor = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_WRONLY
                | libc::O_APPEND
                | libc::O_CREAT
                | libc::O_CLOEXEC
                | libc::O_NOFOLLOW
                | libc::O_NONBLOCK,
            0o600,
        )
    };
    if descriptor < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: openat returned a new descriptor exclusively owned here.
    let file = unsafe { File::from_raw_fd(descriptor) };
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.nlink() != 1 || metadata.len() > MAX_FILE_BYTES {
        return Err(invalid(
            "diagnostic file must be a single-link bounded regular file",
        ));
    }
    private_owned(&metadata)?;
    Ok(file)
}

impl OperationalLog {
    /// Create a 0700 directory if absent, refusing unsafe existing storage.
    /// The parent directory must already exist. Exclusively locks the directory
    /// until dropped, including across rotation; a second writer is refused.
    pub fn open(directory: &Path, component: &str) -> io::Result<Self> {
        identifier(component)?;
        match DirBuilder::new().mode(0o700).create(directory) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        let directory_file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(directory)?;
        private_owned(&directory_file.metadata()?)?;
        // SAFETY: flock takes a live descriptor and fixed operation flags.
        if unsafe { libc::flock(directory_file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let name =
            CString::new(format!("{component}.log")).map_err(|_| invalid("invalid component"))?;
        let old_name =
            CString::new(format!("{component}.log.1")).map_err(|_| invalid("invalid component"))?;
        // Validate an existing rotation too. Never follow or append through it.
        // SAFETY: descriptor and name are live. O_PATH inspects any file type
        // without opening a device or waiting for a FIFO peer.
        let old_descriptor = unsafe {
            libc::openat(
                directory_file.as_raw_fd(),
                old_name.as_ptr(),
                libc::O_PATH | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            )
        };
        if old_descriptor >= 0 {
            // SAFETY: openat returned a new descriptor exclusively owned here.
            let old_file = unsafe { File::from_raw_fd(old_descriptor) };
            let metadata = old_file.metadata()?;
            if !metadata.is_file() || metadata.nlink() != 1 || metadata.len() > MAX_FILE_BYTES {
                return Err(invalid("old diagnostic file is unsafe or oversized"));
            }
            private_owned(&metadata)?;
        } else {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::NotFound {
                return Err(error);
            }
        }
        let file = open_log(&directory_file, &name)?;
        Ok(Self {
            directory: directory_file,
            path: directory.join(format!("{component}.log")),
            name,
            old_name,
            component: component.to_owned(),
            writer: Mutex::new(Some(file)),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Emit a fixed event identifier, never a payload, token or free-form error.
    pub fn event(&self, event: &str) -> io::Result<()> {
        identifier(event)?;
        self.append(&format!("{} event={event}\n", self.prefix()?))
    }

    /// Emit one process sample; call periodically (for example every 15 s).
    pub fn sample(&self) -> io::Result<()> {
        let status = read_proc(Path::new("/proc/self/status"))?;
        let stat = read_proc(Path::new("/proc/self/stat"))?;
        let rss = memory_field(&status, "VmRSS:")?;
        let hwm = memory_field(&status, "VmHWM:")?;
        let (user, system) = cpu_fields(&stat)?;
        // SAFETY: sysconf takes a fixed key and no pointers.
        let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        if hz <= 0 {
            return Err(invalid("invalid process clock tick rate"));
        }
        self.append(&format!(
            "{} event=process_sample rss_bytes={rss} hwm_bytes={hwm} cpu_user_ticks={user} cpu_system_ticks={system} ticks_per_second={hz}\n",
            self.prefix()?
        ))
    }

    fn prefix(&self) -> io::Result<String> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| invalid("clock is before Unix epoch"))?
            .as_nanos();
        Ok(format!(
            "unix_ns={now} component={} pid={}",
            self.component,
            std::process::id()
        ))
    }

    fn append(&self, line: &str) -> io::Result<()> {
        if line.len() > MAX_LINE_BYTES
            || !line.ends_with('\n')
            || line[..line.len() - 1].contains(['\r', '\n'])
        {
            return Err(invalid("diagnostic line exceeds its single-line bound"));
        }
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| io::Error::other("diagnostic writer poisoned"))?;
        if writer.is_none() {
            *writer = Some(open_log(&self.directory, &self.name)?);
        }
        if writer.as_ref().expect("writer opened").metadata()?.len() + line.len() as u64
            > MAX_FILE_BYTES
        {
            // SAFETY: both names and directory descriptor remain live. Rename
            // replaces the old directory entry without following a symlink.
            if unsafe {
                libc::renameat(
                    self.directory.as_raw_fd(),
                    self.name.as_ptr(),
                    self.directory.as_raw_fd(),
                    self.old_name.as_ptr(),
                )
            } != 0
            {
                return Err(io::Error::last_os_error());
            }
            // Drop the rotated descriptor before opening the new active file.
            *writer = None;
            *writer = Some(open_log(&self.directory, &self.name)?);
        }
        writer
            .as_mut()
            .expect("writer opened")
            .write_all(line.as_bytes())
    }
}

fn read_proc(path: &Path) -> io::Result<String> {
    let mut text = String::new();
    File::open(path)?
        .take(MAX_PROC_BYTES + 1)
        .read_to_string(&mut text)?;
    if text.len() as u64 > MAX_PROC_BYTES {
        return Err(invalid("process source exceeds read bound"));
    }
    Ok(text)
}

fn memory_field(status: &str, name: &str) -> io::Result<u64> {
    let field = status
        .lines()
        .find_map(|line| line.strip_prefix(name))
        .ok_or_else(|| invalid("missing process memory field"))?;
    let mut words = field.split_whitespace();
    let value: u64 = words
        .next()
        .ok_or_else(|| invalid("missing memory value"))?
        .parse()
        .map_err(|_| invalid("invalid memory value"))?;
    if words.next() != Some("kB") || words.next().is_some() {
        return Err(invalid("invalid process memory unit"));
    }
    value
        .checked_mul(1024)
        .ok_or_else(|| invalid("process memory overflow"))
}

fn cpu_fields(stat: &str) -> io::Result<(u64, u64)> {
    // comm (field 2) may contain spaces and ')' characters. Its last ')' is
    // the delimiter; numeric fields after it contain neither parentheses.
    let (_, tail) = stat
        .rsplit_once(')')
        .ok_or_else(|| invalid("missing process comm"))?;
    let mut fields = tail.split_whitespace();
    let user = fields
        .nth(11)
        .ok_or_else(|| invalid("missing user CPU ticks"))?
        .parse()
        .map_err(|_| invalid("invalid user CPU ticks"))?;
    let system = fields
        .next()
        .ok_or_else(|| invalid("missing system CPU ticks"))?
        .parse()
        .map_err(|_| invalid("invalid system CPU ticks"))?;
    Ok((user, system))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::var_os("FABRIC_SCRATCH_ROOT")
                .expect("resource launcher scratch required");
            let path = PathBuf::from(root).join(format!(
                "operational-log-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            Self(path)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).expect("clean diagnostic scratch");
        }
    }

    #[test]
    fn events_and_samples_are_bounded_single_lines() {
        let scratch = Scratch::new();
        let log = OperationalLog::open(&scratch.0, "server").unwrap();
        log.event("started").unwrap();
        log.sample().unwrap();
        for invalid in ["has space", "newline\n", "token=secret", ""] {
            assert!(log.event(invalid).is_err());
        }
        assert!(log.event(&"x".repeat(65)).is_err());
        assert!(log.append(&"x".repeat(MAX_LINE_BYTES + 1)).is_err());
        let text = fs::read_to_string(log.path()).unwrap();
        assert_eq!(text.lines().count(), 2);
        assert!(text.lines().all(|line| line.len() < MAX_LINE_BYTES));
        assert!(text.contains("rss_bytes="));
        assert!(text.contains("cpu_user_ticks="));
        assert!(text.contains("ticks_per_second="));
    }

    #[test]
    fn rotation_keeps_only_two_bounded_files_and_locks_the_directory() {
        let scratch = Scratch::new();
        let log = OperationalLog::open(&scratch.0, "spindle").unwrap();
        assert!(OperationalLog::open(&scratch.0, "spindle").is_err());
        let line = format!("{}\n", "x".repeat(MAX_LINE_BYTES - 1));
        for _ in 0..129 {
            log.append(&line).unwrap();
        }
        assert_eq!(
            fs::metadata(log.path()).unwrap().len(),
            MAX_LINE_BYTES as u64
        );
        assert_eq!(
            fs::metadata(scratch.0.join("spindle.log.1")).unwrap().len(),
            MAX_FILE_BYTES
        );
        assert_eq!(fs::read_dir(&scratch.0).unwrap().count(), 2);
        assert_eq!(
            fs::metadata(log.path()).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn unsafe_file_types_and_permissions_are_refused() {
        let scratch = Scratch::new();
        DirBuilder::new().mode(0o700).create(&scratch.0).unwrap();
        let path = scratch.0.join("server.log");
        symlink("missing", &path).unwrap();
        assert!(OperationalLog::open(&scratch.0, "server").is_err());
        fs::remove_file(&path).unwrap();
        use std::os::unix::ffi::OsStrExt;
        let fifo = CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: fifo is a valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        assert!(OperationalLog::open(&scratch.0, "server").is_err());
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(OperationalLog::open(&scratch.0, "server").is_err());
        fs::remove_dir(&path).unwrap();
        symlink("missing", scratch.0.join("server.log.1")).unwrap();
        assert!(OperationalLog::open(&scratch.0, "server").is_err());
        fs::remove_file(scratch.0.join("server.log.1")).unwrap();
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        file.set_len(MAX_FILE_BYTES + 1).unwrap();
        assert!(OperationalLog::open(&scratch.0, "server").is_err());
        fs::remove_file(&path).unwrap();
        fs::set_permissions(&scratch.0, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(OperationalLog::open(&scratch.0, "server").is_err());
    }

    #[test]
    fn process_parser_handles_parentheses_and_rejects_overflow() {
        assert_eq!(
            cpu_fields("1 (worker ) name) R 0 0 0 0 0 0 0 0 0 0 123 456 0").unwrap(),
            (123, 456)
        );
        assert_eq!(memory_field("VmRSS: 42 kB\n", "VmRSS:").unwrap(), 42 * 1024);
        assert!(memory_field("VmRSS: 18446744073709551615 kB", "VmRSS:").is_err());
        assert!(memory_field("VmRSS: 42 bytes", "VmRSS:").is_err());
    }
}
