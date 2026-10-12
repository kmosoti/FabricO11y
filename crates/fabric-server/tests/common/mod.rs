//! Credential files for server integration tests, owner-only as readers require.
use std::{fs::OpenOptions, io::Write, os::unix::fs::OpenOptionsExt, path::Path};

pub fn write_secret(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(contents.as_ref()).unwrap();
}
