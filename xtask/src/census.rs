//! Read-only DATA census with descriptor-anchored traversal and finite cleanup retries.
use serde_json::json;
use std::ffi::{CString, OsStr};
use std::fs::{self, File, ReadDir};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

const ATTEMPTS: u8 = 10;
const DELAY: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Totals {
    pub data_bytes: u64,
    pub regular_path_bytes: u64,
    pub path_count: u64,
}

#[derive(Clone, Copy)]
enum EntryKind { RegularFile, OtherFile, DirectorySymlink }

impl Totals {
    fn add(&mut self, kind: EntryKind, size: u64) -> Result<(), ()> {
        let data_bytes = self.data_bytes.checked_add(size).ok_or(())?;
        let regular_path_bytes = if matches!(kind, EntryKind::RegularFile) {
            self.regular_path_bytes.checked_add(size).ok_or(())?
        } else { self.regular_path_bytes };
        let path_count = if matches!(kind, EntryKind::DirectorySymlink) {
            self.path_count
        } else {
            self.path_count.checked_add(1).ok_or(())?
        };
        self.data_bytes = data_bytes;
        self.regular_path_bytes = regular_path_bytes;
        self.path_count = path_count;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanResult {
    pub totals: Totals,
    pub attempt: u8,
}

#[derive(Debug)]
pub struct ScanError {
    pub path: PathBuf,
    pub kind: io::ErrorKind,
    pub raw_os_error: Option<i32>,
    pub attempt: u8,
    detail: String,
}

impl ScanError {
    fn io(path: PathBuf, error: io::Error) -> Self {
        Self { path, kind: error.kind(), raw_os_error: error.raw_os_error(), attempt: 0, detail: error.to_string() }
    }

    fn overflow(path: PathBuf) -> Self {
        Self { path, kind: io::ErrorKind::InvalidData, raw_os_error: None, attempt: 0, detail: "census counter overflow".into() }
    }
}

impl std::fmt::Display for ScanError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{} (errno {:?}, attempt {}): {}", self.path.display(), self.raw_os_error, self.attempt, self.detail)
    }
}

struct Frame {
    anchor: File,
    entries: ReadDir,
    logical_path: PathBuf,
}

fn openat(parent: RawFd, name: &OsStr, flags: i32) -> io::Result<File> {
    let name = CString::new(name.as_bytes()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    // SAFETY: the component is a live NUL-free CString; flags never include O_CREAT.
    let fd = unsafe { libc::openat(parent, name.as_ptr(), flags | libc::O_CLOEXEC) };
    if fd < 0 { return Err(io::Error::last_os_error()); }
    // SAFETY: openat returned a unique owned descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn open_absolute_directory(path: &Path) -> io::Result<File> {
    let bytes = CString::new(path.as_os_str().as_bytes()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    // SAFETY: path is a live NUL-free absolute CString and O_CREAT is absent.
    let fd = unsafe { libc::openat(libc::AT_FDCWD, bytes.as_ptr(), libc::O_PATH | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
    if fd < 0 { return Err(io::Error::last_os_error()); }
    // SAFETY: openat returned a unique owned descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn read_directory(anchor: &File, path: &Path) -> Result<ReadDir, ScanError> {
    let alias = PathBuf::from(format!("/proc/self/fd/{}", anchor.as_raw_fd()));
    fs::read_dir(alias).map_err(|error| ScanError::io(path.to_path_buf(), error))
}

fn valid_suffix(value: &OsStr, alphabet: impl Fn(u8) -> bool) -> bool {
    !value.is_empty() && value.as_bytes().iter().copied().all(alphabet)
}

fn named_category(path: &Path, root: &Path, prefix: &str, alphabet: impl Fn(u8) -> bool + Copy) -> bool {
    let scratch = root.join("scratch");
    let Ok(relative) = path.strip_prefix(&scratch) else { return false; };
    let mut components = relative.components();
    let Some(std::path::Component::Normal(name)) = components.next() else { return false; };
    let Some(name) = name.to_str() else { return false; };
    let Some(suffix) = name.strip_prefix(prefix) else { return false; };
    if !valid_suffix(OsStr::new(suffix), alphabet) { return false; }
    let base = scratch.join(name);
    path == base || path.starts_with(&base)
}

fn runner_controls_category(path: &Path, root: &Path) -> bool {
    named_category(path, root, "runner-controls-", |byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn owned_category(path: &Path, root: &Path) -> bool {
    named_category(path, root, "installation-qemu-", |byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        || named_category(path, root, "owner-walkthrough-", |byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn valid_owned_argument(root: &Path, path: &Path) -> Result<(), String> {
    if !path.is_absolute() { return Err("--owned-scratch must be absolute".into()); }
    let text = path.to_str().ok_or("--owned-scratch must be UTF-8")?;
    if text.as_bytes().contains(&0) { return Err("invalid owned scratch path".into()); }
    let scratch = root.join("scratch");
    let parent = fs::symlink_metadata(&scratch).map_err(|error| error.to_string())?;
    if !parent.is_dir() || parent.file_type().is_symlink() { return Err("DATA scratch must be an ordinary directory".into()); }
    if path.parent() != Some(scratch.as_path()) { return Err("owned scratch must be a direct DATA scratch child".into()); }
    if !owned_category(path, root) || path.file_name().is_none() { return Err("unrecognized exact owned scratch category".into()); }
    let name = path.file_name().and_then(OsStr::to_str).ok_or("owned scratch basename must be UTF-8")?;
    let canonical_form = format!("{}/{}", scratch.display(), name);
    if path.as_os_str().as_bytes() != canonical_form.as_bytes() { return Err("owned scratch path must be canonical".into()); }
    Ok(())
}

fn retry_allowed(root: &Path, owned: Option<&Path>, path: &Path) -> bool {
    runner_controls_category(path, root)
        || owned.is_some_and(|owned| path == owned || path.starts_with(owned))
}

fn scan_once(root: &Path) -> Result<Totals, ScanError> {
    let anchor = open_absolute_directory(root).map_err(|error| ScanError::io(root.to_path_buf(), error))?;
    let entries = read_directory(&anchor, root)?;
    let mut stack = vec![Frame { anchor, entries, logical_path: root.to_path_buf() }];
    let mut totals = Totals::default();
    while !stack.is_empty() {
        let next = {
            let frame = stack.last_mut().expect("nonempty traversal stack");
            match frame.entries.next() {
                Some(Ok(entry)) => Some((entry.file_name(), frame.logical_path.clone())),
                Some(Err(error)) => return Err(ScanError::io(frame.logical_path.clone(), error)),
                None => None,
            }
        };
        let Some((name, parent_path)) = next else { stack.pop(); continue; };
        let path = parent_path.join(&name);
        #[cfg(test)]
        tests::test_after_enumeration(&path)?;
        let parent = stack.last().expect("entry retains parent frame");
        let entry = openat(parent.anchor.as_raw_fd(), &name, libc::O_PATH | libc::O_NOFOLLOW)
            .map_err(|error| ScanError::io(path.clone(), error))?;
        let metadata = entry.metadata().map_err(|error| ScanError::io(path.clone(), error))?;
        if metadata.file_type().is_symlink() {
            let target_is_directory = openat(parent.anchor.as_raw_fd(), &name, libc::O_PATH)
                .and_then(|target| target.metadata())
                .is_ok_and(|target| target.is_dir());
            let kind = if target_is_directory { EntryKind::DirectorySymlink } else { EntryKind::OtherFile };
            totals.add(kind, metadata.size()).map_err(|_| ScanError::overflow(path))?;
        } else if metadata.is_dir() {
            let entries = read_directory(&entry, &path)?;
            stack.push(Frame { anchor: entry, entries, logical_path: path });
        } else {
            let kind = if metadata.is_file() { EntryKind::RegularFile } else { EntryKind::OtherFile };
            totals.add(kind, metadata.size()).map_err(|_| ScanError::overflow(path))?;
        }
    }
    Ok(totals)
}

fn pause_before_retry() {
    #[cfg(test)]
    { tests::test_retry_pause(DELAY); }
    #[cfg(not(test))]
    std::thread::sleep(DELAY);
}

/// Count a canonical directory tree. `owned` is an exact, caller-derived scratch root.
pub fn scan(root: &Path, owned: Option<&Path>) -> Result<ScanResult, ScanError> {
    let canonical = root.canonicalize().map_err(|error| ScanError::io(root.to_path_buf(), error))?;
    if canonical != root { return Err(ScanError { path: root.to_path_buf(), kind: io::ErrorKind::InvalidInput, raw_os_error: None, attempt: 0, detail: "canonical absolute DATA root required".into() }); }
    if let Some(path) = owned { valid_owned_argument(root, path).map_err(|detail| ScanError { path: path.to_path_buf(), kind: io::ErrorKind::InvalidInput, raw_os_error: None, attempt: 0, detail })?; }
    for attempt in 1..=ATTEMPTS {
        match scan_once(root) {
            Ok(totals) => return Ok(ScanResult { totals, attempt }),
            Err(mut error) => {
                error.attempt = attempt;
                let retry = error.raw_os_error == Some(libc::ENOENT) && retry_allowed(root, owned, &error.path);
                if !retry || attempt == ATTEMPTS { return Err(error); }
                pause_before_retry();
            }
        }
    }
    unreachable!("bounded scan loop always returns")
}

fn parse_owned(root: &Path, args: Vec<String>) -> Result<Option<PathBuf>, String> {
    let mut args = args.into_iter();
    let mut owned = None;
    while let Some(flag) = args.next() {
        if flag != "--owned-scratch" { return Err(format!("unknown argument: {flag}")); }
        if owned.is_some() { return Err("duplicate --owned-scratch".into()); }
        let value = args.next().ok_or("--owned-scratch value required")?;
        owned = Some(PathBuf::from(value));
    }
    if let Some(path) = &owned { valid_owned_argument(root, path)?; }
    Ok(owned)
}

pub fn cli(args: Vec<String>) -> ExitCode {
    let result = (|| -> Result<ScanResult, String> {
        let configured = std::env::var_os("FABRIC_STORAGE_ROOT").ok_or("explicit FABRIC_STORAGE_ROOT required")?;
        let configured = PathBuf::from(configured);
        let metadata = fs::symlink_metadata(&configured).map_err(|error| error.to_string())?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() { return Err("FABRIC_STORAGE_ROOT must be an existing ordinary directory".into()); }
        let root = configured.canonicalize().map_err(|error| error.to_string())?;
        if configured.as_os_str().as_bytes() != root.as_os_str().as_bytes() { return Err("FABRIC_STORAGE_ROOT must already be canonical".into()); }
        let owned = parse_owned(&root, args)?;
        scan(&root, owned.as_deref()).map_err(|error| error.to_string())
    })();
    match result {
        Ok(result) => {
            println!("{}", json!({"data_bytes":result.totals.data_bytes,"regular_path_bytes":result.totals.regular_path_bytes,"path_count":result.totals.path_count,"uid":unsafe { libc::getuid() },"gid":unsafe { libc::getgid() },"attempt":result.attempt}));
            ExitCode::SUCCESS
        }
        Err(error) => { eprintln!("storage census FAILED: {error}"); ExitCode::from(2) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);
    thread_local! {
        static HOOK: RefCell<Option<TestHook>> = const { RefCell::new(None) };
        static EVENTS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    #[derive(Clone)]
    enum Action {
        DeleteSecondFile { directory: PathBuf, seen: usize, target: Option<PathBuf>, restore: Vec<u8> },
        DeleteRoot { target: PathBuf, relative_file: PathBuf, restore: Vec<u8>, fired: bool },
        Inject { trigger: PathBuf, error_path: PathBuf, errno: i32 },
    }

    struct TestHook { action: Action }

    pub(super) fn test_after_enumeration(path: &Path) -> Result<(), ScanError> {
        HOOK.with(|slot| {
            let mut slot = slot.borrow_mut();
            let Some(hook) = slot.as_mut() else { return Ok(()); };
            match &mut hook.action {
                Action::DeleteSecondFile { directory, seen, target, .. }
                    if path.parent() == Some(directory.as_path())
                        && path.file_name().and_then(OsStr::to_str).is_some_and(|name| name.starts_with("file-")) => {
                    *seen += 1;
                    if *seen == 2 {
                        fs::remove_file(path).unwrap(); *target = Some(path.to_path_buf());
                        EVENTS.with(|events| events.borrow_mut().push("real-file-unlink-second-enumerated".into()));
                    }
                }
                Action::DeleteRoot { target, fired, .. } if !*fired && path == target => {
                    fs::remove_dir_all(target).unwrap(); *fired = true;
                    EVENTS.with(|events| events.borrow_mut().push("real-root-removal".into()));
                }
                Action::Inject { trigger, error_path, errno } if path == trigger => {
                    EVENTS.with(|events| events.borrow_mut().push(format!("injected-{errno}")));
                    return Err(ScanError::io(error_path.clone(), io::Error::from_raw_os_error(*errno)));
                }
                _ => {}
            }
            Ok(())
        })
    }

    pub(super) fn test_retry_pause(delay: Duration) {
        assert_eq!(delay, Duration::from_millis(250));
        EVENTS.with(|events| events.borrow_mut().push("retry-delay".into()));
        HOOK.with(|slot| {
            if let Some(hook) = slot.borrow_mut().as_mut() {
                match &hook.action {
                    Action::DeleteSecondFile { target: Some(target), restore, .. } => fs::write(target, restore).unwrap(),
                    Action::DeleteRoot { target, relative_file, restore, fired: true } => {
                        fs::create_dir_all(target.join(relative_file.parent().unwrap())).unwrap();
                        fs::write(target.join(relative_file), restore).unwrap();
                    }
                    _ => {}
                }
            }
        });
    }

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            let root = PathBuf::from(std::env::var_os("FABRIC_SCRATCH_ROOT").expect("DATA scratch"));
            let path = root.join(format!("runner-controls-census_{}_{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            clear_hook();
            if std::thread::panicking() {
                eprintln!("failed census fixture retained: {}", self.0.display());
            } else {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
    }

    fn fixture() -> (Scratch, PathBuf) {
        let scratch = Scratch::new();
        let owned = scratch.0.join("scratch/installation-qemu-case1");
        fs::create_dir_all(&owned).unwrap();
        (scratch, owned)
    }

    fn set_hook(action: Action) { EVENTS.with(|events| events.borrow_mut().clear()); HOOK.with(|slot| *slot.borrow_mut() = Some(TestHook { action })); }
    fn clear_hook() { HOOK.with(|slot| *slot.borrow_mut() = None); }
    fn events() -> Vec<String> { EVENTS.with(|events| events.borrow().clone()) }

    #[test]
    fn checked_counter_updates_are_atomic() {
        let mut totals = Totals { data_bytes: u64::MAX, regular_path_bytes: 3, path_count: 4 };
        let before = totals;
        assert!(totals.add(EntryKind::RegularFile, 1).is_err());
        assert_eq!(totals, before);
        let mut totals = Totals { data_bytes: 4, regular_path_bytes: u64::MAX, path_count: 2 };
        let before = totals;
        assert!(totals.add(EntryKind::RegularFile, 1).is_err());
        assert_eq!(totals, before);
        assert!(totals.add(EntryKind::OtherFile, 0).is_ok());
        totals.path_count = u64::MAX;
        let before = totals;
        assert!(totals.add(EntryKind::OtherFile, 0).is_err());
        assert_eq!(totals, before);
        let mut totals = Totals::default();
        totals.add(EntryKind::DirectorySymlink, 9).unwrap();
        assert_eq!(totals, Totals { data_bytes: 9, regular_path_bytes: 0, path_count: 0 });
    }

    #[test]
    fn preserves_file_link_directory_link_sparse_hardlink_and_fifo_semantics() {
        let (scratch, owned) = fixture();
        let root = &scratch.0;
        fs::write(owned.join("regular"), b"1234567").unwrap();
        fs::hard_link(owned.join("regular"), owned.join("hardlink")).unwrap();
        fs::write(owned.join("empty"), b"").unwrap();
        File::create(owned.join("sparse")).unwrap().set_len(65_537).unwrap();
        fs::create_dir(owned.join("directory")).unwrap();
        fs::write(owned.join("nested"), b"1234567890123").unwrap();
        symlink("regular", owned.join("file-link")).unwrap();
        symlink("directory", owned.join("dir-link")).unwrap();
        symlink("absent", owned.join("dangling-link")).unwrap();
        symlink("../../..", owned.join("outside-dir-link")).unwrap();
        let fifo_path = owned.join("fifo");
        let fifo_name = CString::new(fifo_path.as_os_str().as_bytes()).unwrap();
        // SAFETY: fifo_name is a live NUL-free path, and creation is confined to this test fixture.
        assert_eq!(unsafe { libc::mkfifo(fifo_name.as_ptr(), 0o600) }, 0);
        let actual = scan(root, Some(&owned)).unwrap();
        assert_eq!(actual.totals, Totals { data_bytes: 65_594, regular_path_bytes: 65_564, path_count: 8 });
        assert_eq!(actual.attempt, 1);
    }

    #[test]
    fn only_exact_owned_and_runner_control_descendants_retry() {
        let (scratch, owned) = fixture();
        assert!(retry_allowed(&scratch.0, Some(&owned), &owned.join("x")));
        assert!(retry_allowed(&scratch.0, None, &scratch.0.join("scratch/runner-controls-other/x")));
        assert!(!retry_allowed(&scratch.0, Some(&owned), &owned.with_file_name("installation-qemu-case1-sibling")));
        assert!(!retry_allowed(&scratch.0, Some(&owned), Path::new("scratch/installation-qemu-case1/x")));
        assert!(valid_owned_argument(&scratch.0, &owned).is_ok());
        assert!(parse_owned(&scratch.0, vec![]).unwrap().is_none());
        assert!(parse_owned(&scratch.0, vec!["--owned-scratch".into(), owned.display().to_string()]).unwrap().is_some());
        assert!(parse_owned(&scratch.0, vec!["--root".into(), scratch.0.display().to_string()]).is_err());
        assert!(parse_owned(&scratch.0, vec!["--owned-scratch".into(), owned.display().to_string(), "--owned-scratch".into(), owned.display().to_string()]).is_err());
    }

    #[test]
    fn real_second_file_unlink_restarts_and_discards_partial_totals() {
        let (scratch, owned) = fixture();
        for index in 0..8 { fs::write(owned.join(format!("file-{index}")), b"same").unwrap(); }
        set_hook(Action::DeleteSecondFile { directory: owned.clone(), seen: 0, target: None, restore: b"same".to_vec() });
        let result = scan(&scratch.0, Some(&owned)).unwrap();
        clear_hook();
        assert_eq!(result.attempt, 2);
        assert_eq!(events().iter().filter(|event| event.as_str() == "real-file-unlink-second-enumerated").count(), 1);
        assert_eq!(events().iter().filter(|event| event.as_str() == "retry-delay").count(), 1);
        assert_eq!(result.totals, Totals { data_bytes: 32, regular_path_bytes: 32, path_count: 8 });
    }

    #[test]
    fn real_exact_root_removal_restarts() {
        let (scratch, owned) = fixture();
        fs::write(owned.join("stable"), b"stable").unwrap();
        set_hook(Action::DeleteRoot { target: owned.clone(), relative_file: PathBuf::from("restored"), restore: b"fresh".to_vec(), fired: false });
        let result = scan(&scratch.0, Some(&owned)).unwrap();
        clear_hook();
        assert_eq!(result.attempt, 2);
        assert!(events().contains(&"real-root-removal".to_owned()));
        assert_eq!(result.totals, Totals { data_bytes: 5, regular_path_bytes: 5, path_count: 1 });
    }

    #[test]
    fn unknown_prefix_permission_io_and_exhaustion_fail_closed() {
        let (scratch, owned) = fixture();
        let trigger = scratch.0.join("trigger");
        fs::write(&trigger, b"trigger").unwrap();
        for (name, error_path, errno, expected_attempts) in [
            ("unknown", scratch.0.join("unowned/missing"), libc::ENOENT, 1u8),
            ("prefix", owned.with_file_name("installation-qemu-case1-sibling").join("missing"), libc::ENOENT, 1u8),
            ("permission", owned.join("missing"), libc::EACCES, 1u8),
            ("io", owned.join("missing"), libc::EIO, 1u8),
            ("exhaustion", owned.join("missing"), libc::ENOENT, 10u8),
        ] {
            set_hook(Action::Inject { trigger: trigger.clone(), error_path, errno });
            let error = scan(&scratch.0, Some(&owned)).unwrap_err();
            clear_hook();
            assert_eq!(error.attempt, expected_attempts, "{name}: {error}");
            assert_eq!(error.raw_os_error, Some(errno), "{name}");
            let retries = events().iter().filter(|event| event.as_str() == "retry-delay").count();
            assert_eq!(retries, usize::from(expected_attempts.saturating_sub(1)), "{name}");
        }
    }
}
