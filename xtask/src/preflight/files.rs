//! Linux descriptor-relative lookup. No payload is reopened to decode JSON.
use super::{hex, io, require, strict_json};
use super::json::{JSON_LIMIT, JSON_TOTAL_LIMIT};
use serde_json::Value;
use sha2::{Digest, Sha256};
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
    total.checked_add(size).filter(|sum| *sum <= TOTAL_LIMIT)
        .ok_or_else(|| "artifact total exceeds 32 GiB".to_owned())
}

pub(super) fn parts(path: &str) -> Result<Vec<&str>, String> {
    require(!path.is_empty() && !path.starts_with('/') && !path.contains('\0'),
            "relative POSIX path required")?;
    let parts: Vec<_> = path.split('/').collect();
    require(parts.iter().all(|part| !matches!(*part, "" | "." | "..")),
            "noncanonical path")?;
    Ok(parts)
}

fn identity(info: &Metadata) -> Identity {
    (info.dev(), info.ino(), info.len(), info.mtime(), info.mtime_nsec(),
     info.ctime(), info.ctime_nsec())
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
            directory = open_at(directory.as_raw_fd(), part,
                                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW)?;
        }
    }
    Ok(directory)
}

struct Record {
    identity: Identity,
    digest: String,
    json: Option<Value>,
    counted: bool,
}

/// Invocation-local digest/identity cache. Declare JSON consumers before reading;
/// upgrading a package to JSON later would require an impermissible second read.
pub struct ArtifactStore {
    root: String,
    directory: File,
    records: BTreeMap<String, Record>,
    artifact_total: u64,
    json_total: u64,
    bytes_hashed: u64,
}

impl ArtifactStore {
    pub fn new(root: &Path) -> Result<Self, String> {
        let root = root.to_str().ok_or("UTF-8 root required")?.to_owned();
        let directory = open_root(&root)?;
        Ok(Self { root, directory, records: BTreeMap::new(), artifact_total: 0,
                  json_total: 0, bytes_hashed: 0 })
    }

    fn parent<'a>(&self, path: &'a str) -> Result<(File, &'a str), String> {
        let parts = parts(path)?;
        let mut directory = io(self.directory.try_clone())?;
        for part in &parts[..parts.len() - 1] {
            directory = open_at(directory.as_raw_fd(), part,
                                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW)?;
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

    pub fn read(&mut self, path: &str, expected: &str, keep_json: bool) -> Result<(), String> {
        self.read_inner(path, expected, keep_json, true)
    }

    pub(super) fn plan(&mut self, path: &str, expected: &str) -> Result<Value, String> {
        self.read_inner(path, expected, true, false)?;
        self.records.get_mut(path).and_then(|record| record.json.take())
            .ok_or_else(|| "plan JSON not retained".to_owned())
    }

    fn read_inner(&mut self, path: &str, expected: &str, keep_json: bool,
                  artifact: bool) -> Result<(), String> {
        parts(path)?;
        require(hex(expected, 64), "invalid SHA-256")?;
        if let Some(record) = self.records.get_mut(path) {
            require(record.digest == expected, "conflicting path hash")?;
            require(!keep_json || record.json.is_some(), "JSON payload was not retained")?;
            if artifact && !record.counted {
                self.artifact_total = admitted_bytes(self.artifact_total, record.identity.2)?;
                record.counted = true;
            }
            return Ok(());
        }
        let (parent, name) = self.parent(path)?;
        let mut descriptor = open_at(parent.as_raw_fd(), name,
                                    libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK)?;
        let info = io(descriptor.metadata())?;
        require(info.is_file(), "artifact is not regular")?;
        let before = identity(&info);
        require(before == self.path_identity(path)?, "artifact replaced before read")?;
        admitted_bytes(if artifact { self.artifact_total } else { 0 }, info.len())?;
        if keep_json {
            self.admit_json(info.len())?;
        }
        let mut digest = Sha256::new();
        let mut payload = Vec::new();
        let mut buffer = vec![0; CHUNK];
        let mut size = 0;
        loop {
            let count = io(descriptor.read(&mut buffer))?;
            #[cfg(test)]
            after_read(count);
            if count == 0 {
                break;
            }
            size += count as u64;
            admitted_bytes(if artifact { self.artifact_total } else { 0 }, size)?;
            digest.update(&buffer[..count]);
            self.bytes_hashed += count as u64;
            if keep_json {
                self.admit_json(size)?;
                payload.extend_from_slice(&buffer[..count]);
            }
        }
        require(before == identity(&io(descriptor.metadata())?) &&
                before == self.path_identity(path)? && size == info.len(),
                "artifact changed during read")?;
        let actual = format!("{:x}", digest.finalize());
        require(actual == expected, "artifact hash mismatch")?;
        let json = if keep_json { Some(strict_json(&payload)?) } else { None };
        if keep_json {
            self.json_total += size;
        }
        if artifact {
            self.artifact_total = admitted_bytes(self.artifact_total, size)?;
        }
        self.records.insert(path.to_owned(), Record { identity: before,
                            digest: actual, json, counted: artifact });
        Ok(())
    }

    fn admit_json(&self, size: u64) -> Result<(), String> {
        require(size <= JSON_LIMIT as u64, "JSON exceeds 4 MiB")?;
        require(self.json_total + size <= JSON_TOTAL_LIMIT, "retained JSON exceeds 64 MiB")
    }

    pub fn digest(&self, path: &str) -> Option<&str> {
        self.records.get(path).map(|record| record.digest.as_str())
    }

    pub fn bytes_hashed(&self) -> u64 {
        self.bytes_hashed
    }

    pub(super) fn json(&self, path: &str) -> Result<&Value, String> {
        self.records.get(path).and_then(|record| record.json.as_ref())
            .ok_or_else(|| "receipt JSON not retained".to_owned())
    }

    /// Detect root replacement and post-read writes/replacements to every input.
    pub fn recheck(&self) -> Result<(), String> {
        let original = io(self.directory.metadata())?;
        let current = io(open_root(&self.root)?.metadata())?;
        require((original.dev(), original.ino()) == (current.dev(), current.ino()),
                "root replaced")?;
        for (path, record) in &self.records {
            require(self.path_identity(path)? == record.identity, "artifact changed after read")?;
        }
        Ok(())
    }
}

// A thread-local unit-test seam, absent from production builds. It runs after a
// real descriptor read so a fixture can deterministically mutate/replace at EOF.
#[cfg(test)]
thread_local! {
    static READ_HOOK: std::cell::RefCell<Option<Box<dyn FnMut(usize)>>> =
        std::cell::RefCell::new(None);
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
}
