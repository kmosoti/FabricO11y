//! Fixture helpers shared by the server integration tests.
use std::{fs, io::Write, os::unix::fs::OpenOptionsExt, path::Path};

/// Write a credential file the way the token readers require: owner-only.
pub fn write_secret(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(contents.as_ref()).unwrap();
}
