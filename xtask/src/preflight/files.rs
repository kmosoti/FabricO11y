//! Linux descriptor-relative lookup. No payload is reopened to decode JSON.
use super::json::{JSON_LIMIT, JSON_TOTAL_LIMIT};
use super::{hex, io, require, strict_json};
use serde_json::Value;
use std::collections::BTreeMap;
use std::ffi::CString;
use std::fs::{File, Metadata};
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::fs::MetadataExt;
use std::path::Path;

const CHUNK: usize = 1024 * 1024;
const FILE_LIMIT: u64 = 8 * 1024 * 1024 * 1024;
const TOTAL_LIMIT: u64 = 32 * 1024 * 1024 * 1024;
type Identity = (u64, u64, u64, i64, i64, i64, i64);

/// Artifact admission is separate from reading, so the exact limits are testable.
pub fn admitted_bytes(total: u64, size: u64) -> Result<u64, String> {
    require(size <= FILE_LIMIT, "artifact exceeds 8 GiB")?;
    total
        .checked_add(size)
        .filter(|sum| *sum <= TOTAL_LIMIT)
        .ok_or_else(|| "artifact total exceeds 32 GiB".to_owned())
}

pub(super) fn parts(path: &str) -> Result<Vec<&str>, String> {
    require(
        !path.is_empty() && !path.starts_with('/') && !path.contains('\0'),
        "relative POSIX path required",
    )?;
    let parts: Vec<_> = path.split('/').collect();
    require(
        parts.iter().all(|part| !matches!(*part, "" | "." | "..")),
        "noncanonical path",
    )?;
    Ok(parts)
}

fn identity(info: &Metadata) -> Identity {
    (
        info.dev(),
        info.ino(),
        info.len(),
        info.mtime(),
        info.mtime_nsec(),
        info.ctime(),
        info.ctime_nsec(),
    )
}

/// The only unsafe boundary: `name` is NUL-terminated and openat returns a fresh
/// descriptor. Successful ownership transfers immediately to File for RAII close.
/// Callers pass one validated component, except the fixed initial filesystem `/`.
fn open_at(parent: RawFd, name: &str, flags: i32) -> Result<File, String> {
    let name = CString::new(name).map_err(|error| error.to_string())?;
    // SAFETY: name is a live CString; no O_CREAT flags require a mode argument.
    let descriptor = unsafe { libc::openat(parent, name.as_ptr(), flags | libc::O_CLOEXEC) };
    if descriptor < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    // SAFETY: openat just returned this unique, owned, valid descriptor.
    Ok(unsafe { File::from_raw_fd(descriptor) })
}

fn open_root(root: &str) -> Result<File, String> {
    require(root.starts_with('/'), "absolute root required")?;
    let mut directory = open_at(libc::AT_FDCWD, "/", libc::O_RDONLY | libc::O_DIRECTORY)?;
    if root != "/" {
        for part in parts(&root[1..])? {
            directory = open_at(
                directory.as_raw_fd(),
                part,
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW,
            )?;
        }
    }
    Ok(directory)
}

struct Record {
    identity: Identity,
    digest: String,
    json: Option<Value>,
}

/// Invocation-local digest/identity cache. Declare JSON consumers before reading;
/// upgrading a package to JSON later would require an impermissible second read.
pub struct ArtifactStore {
    root: String,
    directory: File,
    records: BTreeMap<String, Record>,
    total_bytes: u64,
    json_total: u64,
    json_budget: u64,
    bytes_hashed: u64,
}

impl ArtifactStore {
    pub fn new(root: &Path) -> Result<Self, String> {
        Self::with_json_budget(root, JSON_TOTAL_LIMIT)
    }

    /// Divide the existing aggregate JSON budget across independent roots.
    /// A caller may tighten the ceiling, never raise it.
    pub fn with_json_budget(root: &Path, json_budget: u64) -> Result<Self, String> {
        require(
            json_budget > 0 && json_budget <= JSON_TOTAL_LIMIT,
            "invalid JSON budget",
        )?;
        let root = root.to_str().ok_or("UTF-8 root required")?.to_owned();
        let directory = open_root(&root)?;
        Ok(Self {
            root,
            directory,
            records: BTreeMap::new(),
            total_bytes: 0,
            json_total: 0,
            json_budget,
            bytes_hashed: 0,
        })
    }

    fn parent<'a>(&self, path: &'a str) -> Result<(File, &'a str), String> {
        let parts = parts(path)?;
        let mut directory = io(self.directory.try_clone())?;
        for part in &parts[..parts.len() - 1] {
            directory = open_at(
                directory.as_raw_fd(),
                part,
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW,
            )?;
        }
        Ok((directory, parts[parts.len() - 1]))
    }

    fn path_identity(&self, path: &str) -> Result<Identity, String> {
        let (parent, name) = self.parent(path)?;
        let descriptor = open_at(parent.as_raw_fd(), name, libc::O_PATH | libc::O_NOFOLLOW)?;
        let info = io(descriptor.metadata())?;
        require(info.is_file(), "artifact is not regular")?;
        Ok(identity(&info))
    }

    pub(super) fn plan(&mut self, path: &str, expected: &str) -> Result<Value, String> {
        self.read(path, expected, true)?;
        self.records
            .get_mut(path)
            .and_then(|record| record.json.take())
            .ok_or_else(|| "plan JSON not retained".to_owned())
    }

    pub fn read(&mut self, path: &str, expected: &str, keep_json: bool) -> Result<(), String> {
        require(hex(expected, 64), "invalid SHA-256")?;
        self.read_inner(path, Some(expected), keep_json)
    }

    /// Observe a current JSON file without an external digest pin. This is not
    /// evidence admission: callers must establish its source/receipt authority.
    /// Reading, hashing, bounds and identity rechecks are identical to `read`.
    pub fn observe_json(&mut self, path: &str) -> Result<&Value, String> {
        self.read_inner(path, None, true)?;
        self.json(path)
    }

    fn read_inner(
        &mut self,
        path: &str,
        expected: Option<&str>,
        keep_json: bool,
    ) -> Result<(), String> {
        parts(path)?;
        if let Some(record) = self.records.get_mut(path) {
            require(
                expected.is_none_or(|pin| record.digest == pin),
                "conflicting path hash",
            )?;
            require(
                !keep_json || record.json.is_some(),
                "JSON payload was not retained",
            )?;
            return Ok(());
        }
        let (parent, name) = self.parent(path)?;
        let mut descriptor = open_at(
            parent.as_raw_fd(),
            name,
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK,
        )?;
        let info = io(descriptor.metadata())?;
        require(info.is_file(), "artifact is not regular")?;
        let before = identity(&info);
        require(
            before == self.path_identity(path)?,
            "artifact replaced before read",
        )?;
        admitted_bytes(self.total_bytes, info.len())?;
        if keep_json {
            self.admit_json(info.len())?;
        }
        let mut digest = crate::digest::Sha256::new();
        let mut payload = Vec::new();
        let mut buffer = vec![0; CHUNK];
        let mut size = 0;
        while size < info.len() {
            let remaining = info.len() - size;
            let limit = remaining.min(CHUNK as u64) as usize;
            let count = io(descriptor.read(&mut buffer[..limit]))?;
            #[cfg(test)]
            after_read(count);
            if count == 0 {
                break;
            }
            size += count as u64;
            admitted_bytes(self.total_bytes, size)?;
            digest.update(&buffer[..count]);
            self.bytes_hashed += count as u64;
            if keep_json {
                self.admit_json(size)?;
                payload.extend_from_slice(&buffer[..count]);
            }
        }
        // Preserve a deterministic completion seam for tests at the extent
        // boundary without issuing another read that could consume appended data.
        #[cfg(test)]
        if size == info.len() {
            after_read(0);
        }
        require(
            before == identity(&io(descriptor.metadata())?)
                && before == self.path_identity(path)?
                && size == info.len(),
            "artifact changed during read",
        )?;
        let actual = crate::digest::hex(&digest.finish());
        require(
            expected.is_none_or(|pin| actual == pin),
            "artifact hash mismatch",
        )?;
        let json = if keep_json {
            Some(strict_json(&payload)?)
        } else {
            None
        };
        if keep_json {
            self.json_total += size;
        }
        self.total_bytes = admitted_bytes(self.total_bytes, size)?;
        self.records.insert(
            path.to_owned(),
            Record {
                identity: before,
                digest: actual,
                json,
            },
        );
        Ok(())
    }

    fn admit_json(&self, size: u64) -> Result<(), String> {
        require(size <= JSON_LIMIT as u64, "JSON exceeds 4 MiB")?;
        require(
            self.json_total + size <= self.json_budget,
            "retained JSON exceeds 64 MiB",
        )
    }

    pub fn digest(&self, path: &str) -> Option<&str> {
        self.records.get(path).map(|record| record.digest.as_str())
    }

    pub fn bytes_hashed(&self) -> u64 {
        self.bytes_hashed
    }

    pub fn json_bytes(&self) -> u64 {
        self.json_total
    }

    pub(crate) fn json(&self, path: &str) -> Result<&Value, String> {
        self.records
            .get(path)
            .and_then(|record| record.json.as_ref())
            .ok_or_else(|| "receipt JSON not retained".to_owned())
    }

    /// Detect root replacement and post-read writes/replacements to every input.
    pub fn recheck(&self) -> Result<(), String> {
        let original = io(self.directory.metadata())?;
        let current = io(open_root(&self.root)?.metadata())?;
        require(
            (original.dev(), original.ino()) == (current.dev(), current.ino()),
            "root replaced",
        )?;
        for (path, record) in &self.records {
            require(
                self.path_identity(path)? == record.identity,
                "artifact changed after read",
            )?;
        }
        Ok(())
    }
}

// A thread-local unit-test seam, absent from production builds. Positive counts
// follow real descriptor reads; zero marks EOF or completion of the admitted extent.
#[cfg(test)]
type ReadHook = std::cell::RefCell<Option<Box<dyn FnMut(usize)>>>;

#[cfg(test)]
thread_local! {
    static READ_HOOK: ReadHook = std::cell::RefCell::new(None);
}

#[cfg(test)]
fn after_read(count: usize) {
    READ_HOOK.with(|hook| {
        if let Some(callback) = hook.borrow_mut().as_mut() {
            callback(count);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observed_json_keeps_budget_hash_and_identity_guards() {
        let scratch = std::env::var_os("FABRIC_SCRATCH_ROOT").expect("contained DATA scratch");
        let root = Path::new(&scratch).join(format!("observed-json-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        for name in ["one.json", "two.json"] {
            std::fs::write(root.join(name), b"{}").unwrap();
        }
        assert!(ArtifactStore::with_json_budget(&root, 0).is_err());
        assert!(ArtifactStore::with_json_budget(&root, JSON_TOTAL_LIMIT + 1).is_err());
        let mut store = ArtifactStore::with_json_budget(&root, 2).unwrap();
        assert_eq!(
            store.observe_json("one.json").unwrap(),
            &serde_json::json!({})
        );
        let pin = crate::sha256_hex(b"{}");
        store.read("one.json", &pin, true).unwrap();
        assert_eq!((store.bytes_hashed(), store.json_bytes()), (2, 2));
        assert!(store.read("one.json", &"0".repeat(64), true).is_err());
        assert!(store.observe_json("two.json").is_err());
        assert_eq!(store.bytes_hashed(), 2);
        store.recheck().unwrap();
        std::fs::write(root.join("one.json"), b"{\"changed\":true}").unwrap();
        assert!(store.recheck().is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn plan_bytes_share_total_budget() {
        // Origin: the Rust contract's explicit total-read budget includes the
        // plan. The inherited Python path excluded it; exercise that boundary
        // with a seeded counter, without allocating or reading 32 GiB.
        let scratch = std::env::var_os("FABRIC_SCRATCH_ROOT").expect("data scratch");
        let root = Path::new(&scratch).join(format!("plan-budget-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("plan.json"), b"{}").unwrap();
        std::fs::write(root.join("other.json"), b"{}").unwrap();
        let pin = crate::sha256_hex(b"{}");
        let mut store = ArtifactStore::new(&root).unwrap();
        store.total_bytes = TOTAL_LIMIT - 2;
        store.plan("plan.json", &pin).unwrap();
        assert_eq!(store.total_bytes, TOTAL_LIMIT);
        assert!(store.read("other.json", &pin, false).is_err());
        assert_eq!(store.bytes_hashed(), 2);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn during_read_replacement_and_mutation_are_refused() {
        let scratch = std::env::var_os("FABRIC_SCRATCH_ROOT").expect("launcher scratch");
        let root = Path::new(&scratch).join(format!("preflight-race-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        for replace in [false, true] {
            let path = root.join("artifact");
            std::fs::write(&path, b"original").unwrap();
            let expected = crate::sha256_hex(b"original");
            let mut store = ArtifactStore::new(&root).unwrap();
            READ_HOOK.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move |count| {
                    if count == 0 {
                        if replace {
                            let next = path.with_extension("replacement");
                            std::fs::write(&next, b"original").unwrap();
                            std::fs::rename(next, &path).unwrap();
                        } else {
                            std::fs::write(&path, b"modified").unwrap();
                        }
                    }
                }));
            });
            let result = store.read("artifact", &expected, false);
            READ_HOOK.with(|hook| *hook.borrow_mut() = None);
            assert!(result.unwrap_err().contains("changed during read"));
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn growing_file_stays_within_admitted_extent() {
        let scratch = std::env::var_os("FABRIC_SCRATCH_ROOT").expect("launcher scratch");
        let root = Path::new(&scratch).join(format!("preflight-growing-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("artifact");
        std::fs::write(&path, b"original").unwrap();
        let expected = crate::sha256_hex(b"original");
        let mut store = ArtifactStore::new(&root).unwrap();
        READ_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new({
                let path = path.clone();
                let mut appended = false;
                move |count| {
                    if count > 0 && !appended {
                        let mut file = std::fs::OpenOptions::new()
                            .append(true)
                            .open(&path)
                            .unwrap();
                        std::io::Write::write_all(&mut file, b"extra").unwrap();
                        appended = true;
                    }
                }
            }));
        });
        let result = store.read("artifact", &expected, false);
        READ_HOOK.with(|hook| *hook.borrow_mut() = None);
        assert!(result.unwrap_err().contains("artifact changed during read"));
        assert!(store.bytes_hashed() <= 8);
        std::fs::remove_dir_all(root).unwrap();
    }
}
