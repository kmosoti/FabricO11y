//! Bounded verification of already-read, local archive evidence.
//!
//! Trusted callers supply identity and digests independently. This module does
//! not authenticate a backend, retain an artifact, or perform eviction.
use fabric_core::retention::VerifiedArchive;
use serde::{Deserialize, Serialize};
use std::fs::{File, Metadata};
use std::os::unix::fs::{FileExt, MetadataExt};

pub const MAX_MANIFEST_BYTES: usize = 4096;
pub const MAX_ARTIFACT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    Exact,
    Aggregate,
}

/// Version-one canonical JSON: serde field order and no insignificant whitespace.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub version: u8,
    pub authority: String,
    pub origin: String,
    pub candidate_id: u64,
    pub profile: Profile,
    pub artifact_len: u64,
    pub artifact_digest: [u8; 32],
}

/// Values from a separately trusted authority and expected artifact identity.
#[derive(Clone, Debug)]
pub struct ExpectedRestore {
    pub authority: String,
    pub origin: String,
    pub candidate_id: u64,
    pub manifest_digest: [u8; 32],
    pub artifact_digest: [u8; 32],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RestoreError {
    MissingManifest,
    MissingArtifact,
    Size,
    MalformedManifest,
    ManifestDigest,
    Identity,
    UnsupportedProfile,
    ArtifactDigest,
    /// A handle could not be read or inspected.
    Unavailable,
    /// A supplied handle is not a regular file.
    NotRegular,
    /// The file changed while it was being read.
    Changed,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
}
fn digest_matches(bytes: &[u8], digest: &[u8; 32]) -> bool {
    let actual = crate::olap::sha256_bytes_hex(bytes);
    let expected: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    actual == expected
}

/// Verify the current restored bytes and only then construct a planner assertion.
pub fn verify_restore(
    manifest_bytes: Option<&[u8]>,
    artifact: Option<&[u8]>,
    expected: &ExpectedRestore,
) -> Result<VerifiedArchive, RestoreError> {
    let manifest_bytes = manifest_bytes.ok_or(RestoreError::MissingManifest)?;
    let manifest = validate_manifest(manifest_bytes, expected)?;
    let artifact = artifact.ok_or(RestoreError::MissingArtifact)?;
    if artifact.len() > MAX_ARTIFACT_BYTES {
        return Err(RestoreError::Size);
    }
    if artifact.len() as u64 != manifest.artifact_len
        || manifest.artifact_digest != expected.artifact_digest
        || !digest_matches(artifact, &expected.artifact_digest)
    {
        return Err(RestoreError::ArtifactDigest);
    }
    Ok(VerifiedArchive {
        id: expected.candidate_id,
        digest: expected.artifact_digest,
    })
}

fn validate_manifest(
    manifest_bytes: &[u8],
    expected: &ExpectedRestore,
) -> Result<Manifest, RestoreError> {
    if manifest_bytes.len() > MAX_MANIFEST_BYTES {
        return Err(RestoreError::Size);
    }
    if !digest_matches(manifest_bytes, &expected.manifest_digest) {
        return Err(RestoreError::ManifestDigest);
    }
    let manifest: Manifest =
        serde_json::from_slice(manifest_bytes).map_err(|_| RestoreError::MalformedManifest)?;
    if manifest.version != 1
        || serde_json::to_vec(&manifest).map_err(|_| RestoreError::MalformedManifest)?
            != manifest_bytes
    {
        return Err(RestoreError::MalformedManifest);
    }
    if !valid_id(&manifest.authority)
        || !valid_id(&manifest.origin)
        || manifest.authority != expected.authority
        || manifest.origin != expected.origin
        || manifest.candidate_id != expected.candidate_id
    {
        return Err(RestoreError::Identity);
    }
    if manifest.profile != Profile::Exact {
        return Err(RestoreError::UnsupportedProfile);
    }
    if manifest.artifact_len == 0 || manifest.artifact_len > MAX_ARTIFACT_BYTES as u64 {
        return Err(RestoreError::Size);
    }
    Ok(manifest)
}

fn regular_metadata(file: &File) -> Result<Metadata, RestoreError> {
    let metadata = file.metadata().map_err(|_| RestoreError::Unavailable)?;
    if !metadata.is_file() {
        return Err(RestoreError::NotRegular);
    }
    Ok(metadata)
}

fn same_snapshot(a: &Metadata, b: &Metadata) -> bool {
    a.dev() == b.dev()
        && a.ino() == b.ino()
        && a.len() == b.len()
        && a.mtime() == b.mtime()
        && a.mtime_nsec() == b.mtime_nsec()
        && a.ctime() == b.ctime()
        && a.ctime_nsec() == b.ctime_nsec()
}

fn read_at_exact(file: &File, mut offset: u64, mut out: &mut [u8]) -> Result<(), RestoreError> {
    while !out.is_empty() {
        match file.read_at(out, offset) {
            Ok(0) => return Err(RestoreError::Changed),
            Ok(n) => {
                offset += n as u64;
                out = &mut out[n..];
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(RestoreError::Unavailable),
        }
    }
    Ok(())
}

fn check_end(file: &File, length: u64, initial: &Metadata) -> Result<(), RestoreError> {
    let mut extra = [0];
    let n = loop {
        match file.read_at(&mut extra, length) {
            Ok(n) => break n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(RestoreError::Unavailable),
        }
    };
    if n != 0 || !same_snapshot(initial, &regular_metadata(file)?) {
        return Err(RestoreError::Changed);
    }
    Ok(())
}

/// Verify caller-authorized open regular files without changing their offsets.
/// The result attests only to bytes read in this call, not future availability.
pub fn verify_restore_files(
    manifest_file: Option<&File>,
    artifact_file: Option<&File>,
    expected: &ExpectedRestore,
) -> Result<VerifiedArchive, RestoreError> {
    let manifest_file = manifest_file.ok_or(RestoreError::MissingManifest)?;
    let manifest_meta = regular_metadata(manifest_file)?;
    if manifest_meta.len() > MAX_MANIFEST_BYTES as u64 {
        return Err(RestoreError::Size);
    }
    let mut manifest_bytes = vec![0; manifest_meta.len() as usize];
    read_at_exact(manifest_file, 0, &mut manifest_bytes)?;
    check_end(manifest_file, manifest_meta.len(), &manifest_meta)?;
    let manifest = validate_manifest(&manifest_bytes, expected)?;
    let artifact_file = artifact_file.ok_or(RestoreError::MissingArtifact)?;
    let artifact_meta = regular_metadata(artifact_file)?;
    if artifact_meta.len() > MAX_ARTIFACT_BYTES as u64 {
        return Err(RestoreError::Size);
    }
    if artifact_meta.len() != manifest.artifact_len
        || manifest.artifact_digest != expected.artifact_digest
    {
        return Err(RestoreError::ArtifactDigest);
    }
    let mut hasher = crate::olap::Sha256::new();
    let mut buf = [0u8; 8192];
    let mut offset = 0;
    while offset < manifest.artifact_len {
        let n = (manifest.artifact_len - offset).min(buf.len() as u64) as usize;
        read_at_exact(artifact_file, offset, &mut buf[..n])?;
        hasher.update(&buf[..n]);
        offset += n as u64;
    }
    check_end(artifact_file, offset, &artifact_meta)?;
    let expected_hex: String = expected
        .artifact_digest
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if hasher.finish_hex() != expected_hex {
        return Err(RestoreError::ArtifactDigest);
    }
    Ok(VerifiedArchive {
        id: expected.candidate_id,
        digest: expected.artifact_digest,
    })
}

/// Outcome of publishing one verified artifact in an authorized local directory.
#[derive(Debug)]
pub struct PublishedRestore {
    pub basename: String,
    pub archive: VerifiedArchive,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PublishError {
    Verification(RestoreError),
    /// No visible artifact was created by this call.
    Unavailable,
    /// The filesystem does not support unnamed temporary files.
    Unsupported,
    /// A pre-existing destination is not the independently expected artifact.
    ExistingMismatch,
    /// The collision could not be inspected; no claim about its contents is made.
    ExistingUnavailable(ExistingFailure),
    /// The destination may be visible, but its durability was not established.
    DurabilityUnknown,
}

/// Cause of an inconclusive check of an existing destination.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExistingFailure {
    Open(i32),
    Inspect(i32),
    Read(i32),
    Changed,
}

fn existing_open_error(error: rustix::io::Errno) -> PublishError {
    if error == rustix::io::Errno::LOOP {
        // The final path component was a symlink and NOFOLLOW rejected it.
        PublishError::ExistingMismatch
    } else {
        // Includes disappearance after EEXIST, permission and transient I/O errors.
        PublishError::ExistingUnavailable(ExistingFailure::Open(error.raw_os_error()))
    }
}

fn verify_existing(file: &File, length: u64, expected_hex: &str) -> Result<(), PublishError> {
    let meta = file.metadata().map_err(|error| {
        PublishError::ExistingUnavailable(ExistingFailure::Inspect(
            error.raw_os_error().unwrap_or(0),
        ))
    })?;
    if !meta.is_file() || meta.len() != length {
        return Err(PublishError::ExistingMismatch);
    }
    let mut hasher = crate::olap::Sha256::new();
    let mut buf = [0u8; 8192];
    let mut offset = 0;
    while offset < length {
        let limit = (length - offset).min(buf.len() as u64) as usize;
        let n = loop {
            match file.read_at(&mut buf[..limit], offset) {
                Ok(n) => break n,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    return Err(PublishError::ExistingUnavailable(ExistingFailure::Read(
                        error.raw_os_error().unwrap_or(0),
                    )));
                }
            }
        };
        if n == 0 {
            return Err(PublishError::ExistingUnavailable(ExistingFailure::Changed));
        }
        hasher.update(&buf[..n]);
        offset += n as u64;
    }
    let current = file.metadata().map_err(|error| {
        PublishError::ExistingUnavailable(ExistingFailure::Inspect(
            error.raw_os_error().unwrap_or(0),
        ))
    })?;
    if !same_snapshot(&meta, &current) {
        return Err(PublishError::ExistingUnavailable(ExistingFailure::Changed));
    }

    if hasher.finish_hex() != expected_hex {
        return Err(PublishError::ExistingMismatch);
    }
    Ok(())
}

impl From<RestoreError> for PublishError {
    fn from(value: RestoreError) -> Self {
        Self::Verification(value)
    }
}

/// Publish exact verified bytes relative to an already-authorized directory handle.
/// The filesystem must support unnamed temporary files and linking them by fd;
/// unsupported filesystems fail closed. A caller must control concurrent mutation of
/// the destination directory. The returned assertion says nothing about future retention.
/// Input file offsets are preserved. A durability-unknown result can be retried.
pub fn publish_restore_files(
    manifest_file: &File,
    artifact_file: &File,
    directory: &File,
    expected: &ExpectedRestore,
) -> Result<PublishedRestore, PublishError> {
    publish_restore_files_inner(manifest_file, artifact_file, directory, expected, || {})
}

// The callback is reached only after the unnamed inode has been verified and
// synced, immediately before the first publication attempt.
fn publish_restore_files_inner(
    manifest_file: &File,
    artifact_file: &File,
    directory: &File,
    expected: &ExpectedRestore,
    before_link: impl FnOnce(),
) -> Result<PublishedRestore, PublishError> {
    use rustix::fs::{AtFlags, Mode, OFlags, linkat, openat};
    use std::io::Write;

    if !directory
        .metadata()
        .map_err(|_| PublishError::Unavailable)?
        .is_dir()
    {
        return Err(PublishError::Unavailable);
    }
    let manifest_meta = regular_metadata(manifest_file)?;
    if manifest_meta.len() > MAX_MANIFEST_BYTES as u64 {
        return Err(RestoreError::Size.into());
    }
    let mut manifest_bytes = vec![0; manifest_meta.len() as usize];
    read_at_exact(manifest_file, 0, &mut manifest_bytes)?;
    check_end(manifest_file, manifest_meta.len(), &manifest_meta)?;
    let manifest = validate_manifest(&manifest_bytes, expected)?;
    let artifact_meta = regular_metadata(artifact_file)?;
    if artifact_meta.len() > MAX_ARTIFACT_BYTES as u64 {
        return Err(RestoreError::Size.into());
    }
    if artifact_meta.len() != manifest.artifact_len
        || manifest.artifact_digest != expected.artifact_digest
    {
        return Err(RestoreError::ArtifactDigest.into());
    }
    let basename: String = expected
        .artifact_digest
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    // O_TMPFILE creates an unnamed, exclusive inode. Do not set O_EXCL: on
    // Linux that flag would prohibit subsequently linking this inode.
    let fd = openat(
        directory,
        ".",
        OFlags::TMPFILE | OFlags::WRONLY | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    )
    .map_err(|error| match error {
        rustix::io::Errno::OPNOTSUPP | rustix::io::Errno::INVAL => PublishError::Unsupported,
        _ => PublishError::Unavailable,
    })?;
    let mut temp = File::from(fd);
    let mut hasher = crate::olap::Sha256::new();
    let mut buf = [0u8; 8192];
    let mut offset = 0;
    while offset < manifest.artifact_len {
        let n = (manifest.artifact_len - offset).min(buf.len() as u64) as usize;
        read_at_exact(artifact_file, offset, &mut buf[..n])?;
        hasher.update(&buf[..n]);
        temp.write_all(&buf[..n])
            .map_err(|_| PublishError::Unavailable)?;
        offset += n as u64;
    }
    check_end(artifact_file, offset, &artifact_meta)?;
    if hasher.finish_hex() != basename {
        return Err(RestoreError::ArtifactDigest.into());
    }
    temp.sync_all().map_err(|_| PublishError::Unavailable)?;
    before_link();
    match linkat(&temp, "", directory, basename.as_str(), AtFlags::EMPTY_PATH) {
        Ok(()) => {}
        Err(rustix::io::Errno::EXIST) => {
            let fd = openat(
                directory,
                basename.as_str(),
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(existing_open_error)?;
            let existing = File::from(fd);
            verify_existing(&existing, manifest.artifact_len, &basename)?;
            existing
                .sync_all()
                .map_err(|_| PublishError::DurabilityUnknown)?;
        }
        Err(_) => return Err(PublishError::DurabilityUnknown),
    }
    directory
        .sync_all()
        .map_err(|_| PublishError::DurabilityUnknown)?;
    Ok(PublishedRestore {
        basename,
        archive: VerifiedArchive {
            id: expected.candidate_id,
            digest: expected.artifact_digest,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric_core::retention::{Budget, Candidate, Decision, Fidelity, plan};
    use std::{fs, path::PathBuf};

    fn digest(bytes: &[u8]) -> [u8; 32] {
        let hex = crate::olap::sha256_bytes_hex(bytes);
        let mut result = [0; 32];
        for (i, byte) in result.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap();
        }
        result
    }
    fn budget() -> Budget {
        Budget {
            used: 91,
            free: 30,
            quota: 100,
            low: 60,
            high: 90,
            pressure: false,
            audit: 5,
        }
    }
    fn candidate(digest: [u8; 32]) -> Candidate {
        Candidate {
            id: 7,
            digest,
            fidelity: Fidelity::Exact,
            protected: true,
            active_dependency: false,
            bytes: 3,
            priority: 1,
            rewrite_scratch: None,
            archive: None,
        }
    }
    #[test]
    fn publication_is_verified_atomic_and_retryable() {
        use std::io::{Seek, SeekFrom};
        let root = tempfile::tempdir_in(PathBuf::from(
            std::env::var_os("TMPDIR").expect("disk TMPDIR"),
        ))
        .unwrap();
        let output = root.path().join("output");
        fs::create_dir(&output).unwrap();
        let bytes = vec![b'x'; 20_000];
        let artifact_digest = digest(&bytes);
        let manifest = Manifest {
            version: 1,
            authority: "authority".into(),
            origin: "origin".into(),
            candidate_id: 7,
            profile: Profile::Exact,
            artifact_len: bytes.len() as u64,
            artifact_digest,
        };
        let encoded = serde_json::to_vec(&manifest).unwrap();
        let expected = ExpectedRestore {
            authority: "authority".into(),
            origin: "origin".into(),
            candidate_id: 7,
            manifest_digest: digest(&encoded),
            artifact_digest,
        };
        let mp = root.path().join("manifest");
        let ap = root.path().join("artifact");
        fs::write(&mp, &encoded).unwrap();
        fs::write(&ap, vec![b'y'; bytes.len()]).unwrap();
        let mut mf = File::open(&mp).unwrap();
        let mut af = File::open(&ap).unwrap();
        let dir = File::open(&output).unwrap();
        mf.seek(SeekFrom::Start(3)).unwrap();
        af.seek(SeekFrom::Start(5)).unwrap();
        assert!(matches!(
            publish_restore_files(&mf, &af, &dir, &expected),
            Err(PublishError::Verification(RestoreError::ArtifactDigest))
        ));
        assert_eq!(fs::read_dir(&output).unwrap().count(), 0);
        fs::write(&ap, &bytes).unwrap();
        let published = publish_restore_files(&mf, &af, &dir, &expected).unwrap();
        assert_eq!(published.archive.id, 7);
        assert_eq!(published.archive.digest, artifact_digest);
        assert_eq!(fs::read(output.join(&published.basename)).unwrap(), bytes);
        assert_eq!(
            publish_restore_files(&mf, &af, &dir, &expected)
                .unwrap()
                .basename,
            published.basename
        );
        assert_eq!(mf.stream_position().unwrap(), 3);
        assert_eq!(af.stream_position().unwrap(), 5);
        fs::write(output.join(&published.basename), b"other").unwrap();
        assert!(matches!(
            publish_restore_files(&mf, &af, &dir, &expected),
            Err(PublishError::ExistingMismatch)
        ));
        assert_eq!(
            fs::read(output.join(&published.basename)).unwrap(),
            b"other"
        );
        assert_eq!(fs::read_dir(&output).unwrap().count(), 1);
        fs::remove_file(output.join(&published.basename)).unwrap();
        std::os::unix::fs::symlink(&ap, output.join(&published.basename)).unwrap();
        assert!(matches!(
            publish_restore_files(&mf, &af, &dir, &expected),
            Err(PublishError::ExistingMismatch)
        ));
        assert!(
            fs::symlink_metadata(output.join(&published.basename))
                .unwrap()
                .file_type()
                .is_symlink()
        );

        // A write-only FD deterministically fails reading regardless of user capabilities.
        let unreadable = fs::OpenOptions::new().write(true).open(&ap).unwrap();
        assert_eq!(
            verify_existing(&unreadable, bytes.len() as u64, &published.basename),
            Err(PublishError::ExistingUnavailable(ExistingFailure::Read(
                rustix::io::Errno::BADF.raw_os_error()
            )))
        );
        for error in [rustix::io::Errno::ACCESS, rustix::io::Errno::NOENT] {
            assert_eq!(
                existing_open_error(error),
                PublishError::ExistingUnavailable(ExistingFailure::Open(error.raw_os_error()))
            );
        }
    }

    #[test]
    fn killed_before_link_leaves_no_published_restore_and_retry_succeeds() {
        use std::{
            io::Write,
            os::unix::process::ExitStatusExt,
            process::{Child, Command, Stdio},
            time::{Duration, Instant},
        };
        const CHILD_ROOT: &str = "FABRIC_RESTORE_INTERRUPT_CHILD";
        fn fixture(root: &std::path::Path) -> (File, File, File, ExpectedRestore) {
            let bytes = b"bounded restore fixture";
            let artifact_digest = digest(bytes);
            let manifest = Manifest {
                version: 1,
                authority: "authority".into(),
                origin: "origin".into(),
                candidate_id: 7,
                profile: Profile::Exact,
                artifact_len: bytes.len() as u64,
                artifact_digest,
            };
            let encoded = serde_json::to_vec(&manifest).unwrap();
            let expected = ExpectedRestore {
                authority: "authority".into(),
                origin: "origin".into(),
                candidate_id: 7,
                manifest_digest: digest(&encoded),
                artifact_digest,
            };
            (
                File::open(root.join("manifest")).unwrap(),
                File::open(root.join("artifact")).unwrap(),
                File::open(root.join("output")).unwrap(),
                expected,
            )
        }
        if let Some(root) = std::env::var_os(CHILD_ROOT) {
            let root = PathBuf::from(root);
            let (manifest, artifact, dir, expected) = fixture(&root);
            let result = publish_restore_files_inner(&manifest, &artifact, &dir, &expected, || {
                let mut marker = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(root.join("ready"))
                    .unwrap();
                marker
                    .write_all(b"verified and synced before link")
                    .unwrap();
                marker.sync_all().unwrap();
                loop {
                    std::thread::park();
                }
            });
            panic!("child unexpectedly returned: {result:?}");
        }
        struct Guard(Child);
        impl Drop for Guard {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let root = tempfile::tempdir_in(PathBuf::from(
            std::env::var_os("TMPDIR").expect("disk TMPDIR"),
        ))
        .unwrap();
        let artifact = b"bounded restore fixture";
        let artifact_digest = digest(artifact);
        let manifest = Manifest {
            version: 1,
            authority: "authority".into(),
            origin: "origin".into(),
            candidate_id: 7,
            profile: Profile::Exact,
            artifact_len: artifact.len() as u64,
            artifact_digest,
        };
        fs::write(
            root.path().join("manifest"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        fs::write(root.path().join("artifact"), artifact).unwrap();
        fs::create_dir(root.path().join("output")).unwrap();
        fs::write(root.path().join("output/unrelated"), b"existing").unwrap();
        let mut child = Guard(Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "restored_evidence::tests::killed_before_link_leaves_no_published_restore_and_retry_succeeds", "--nocapture"])
            .env(CHILD_ROOT, root.path()).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap());
        let deadline = Instant::now() + Duration::from_secs(15);
        while !root.path().join("ready").exists() {
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "restore child exited before readiness"
            );
            assert!(
                Instant::now() < deadline,
                "restore child readiness deadline"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        child.0.kill().unwrap();
        assert_eq!(child.0.wait().unwrap().signal(), Some(9));
        assert_eq!(fs::read_dir(root.path().join("output")).unwrap().count(), 1);
        assert_eq!(
            fs::read(root.path().join("output/unrelated")).unwrap(),
            b"existing"
        );
        let (manifest, artifact_file, dir, expected) = fixture(root.path());
        let restored = publish_restore_files(&manifest, &artifact_file, &dir, &expected).unwrap();
        assert_eq!(
            fs::read(root.path().join("output").join(&restored.basename)).unwrap(),
            artifact
        );
        assert_eq!(
            restored.archive,
            VerifiedArchive {
                id: 7,
                digest: artifact_digest
            }
        );
        assert_eq!(
            fs::read(root.path().join("output/unrelated")).unwrap(),
            b"existing"
        );
        assert_eq!(
            publish_restore_files(&manifest, &artifact_file, &dir, &expected)
                .unwrap()
                .basename,
            restored.basename
        );
        let destination = root.path().join("output").join(&restored.basename);
        fs::write(&destination, b"foreign existing bytes").unwrap();
        assert!(matches!(
            publish_restore_files(&manifest, &artifact_file, &dir, &expected),
            Err(PublishError::ExistingMismatch)
        ));
        assert_eq!(fs::read(destination).unwrap(), b"foreign existing bytes");
    }

    #[test]
    fn file_restore_checks_exact_bytes_without_moving_offsets() {
        use std::io::{Seek, SeekFrom};
        let root = tempfile::tempdir_in(PathBuf::from(
            std::env::var_os("TMPDIR").expect("disk TMPDIR"),
        ))
        .unwrap();
        let data = vec![b'x'; 20_000];
        let artifact_digest = digest(&data);
        let manifest = Manifest {
            version: 1,
            authority: "authority".into(),
            origin: "origin".into(),
            candidate_id: 7,
            profile: Profile::Exact,
            artifact_len: data.len() as u64,
            artifact_digest,
        };
        let encoded = serde_json::to_vec(&manifest).unwrap();
        let expected = ExpectedRestore {
            authority: "authority".into(),
            origin: "origin".into(),
            candidate_id: 7,
            manifest_digest: digest(&encoded),
            artifact_digest,
        };
        let mp = root.path().join("manifest");
        let ap = root.path().join("artifact");
        fs::write(&mp, &encoded).unwrap();
        fs::write(&ap, &data).unwrap();
        let mut mf = File::open(&mp).unwrap();
        let mut af = File::open(&ap).unwrap();
        mf.seek(SeekFrom::Start(3)).unwrap();
        af.seek(SeekFrom::Start(5)).unwrap();
        assert_eq!(
            verify_restore_files(Some(&mf), Some(&af), &expected),
            verify_restore(Some(&encoded), Some(&data), &expected)
        );
        assert_eq!(mf.stream_position().unwrap(), 3);
        assert_eq!(af.stream_position().unwrap(), 5);
        assert_eq!(
            verify_restore_files(None, Some(&af), &expected),
            Err(RestoreError::MissingManifest)
        );
        assert_eq!(
            verify_restore_files(Some(&mf), None, &expected),
            Err(RestoreError::MissingArtifact)
        );
        let directory = File::open(root.path()).unwrap();
        assert_eq!(
            verify_restore_files(Some(&mf), Some(&directory), &expected),
            Err(RestoreError::NotRegular)
        );
        let write_only = fs::OpenOptions::new().write(true).open(&ap).unwrap();
        assert_eq!(
            verify_restore_files(Some(&mf), Some(&write_only), &expected),
            Err(RestoreError::Unavailable)
        );
        fs::write(&ap, b"short").unwrap();
        assert_eq!(
            verify_restore_files(Some(&mf), Some(&af), &expected),
            Err(RestoreError::ArtifactDigest)
        );
        fs::write(&ap, vec![b'y'; data.len()]).unwrap();
        assert_eq!(
            verify_restore_files(Some(&mf), Some(&af), &expected),
            Err(RestoreError::ArtifactDigest)
        );
        fs::write(&mp, vec![b'x'; MAX_MANIFEST_BYTES + 1]).unwrap();
        assert_eq!(
            verify_restore_files(Some(&mf), Some(&af), &expected),
            Err(RestoreError::Size)
        );
    }

    #[test]
    fn local_restore_binds_identity_and_only_exact_verified_bytes_feed_planner() {
        let root = tempfile::tempdir_in(PathBuf::from(
            std::env::var_os("TMPDIR").expect("disk-backed TMPDIR"),
        ))
        .unwrap();
        let archive = root.path().join("archive");
        let restore = root.path().join("restore");
        let manifest_path = root.path().join("manifest");
        let bytes = b"abc";
        let artifact_digest = digest(bytes);
        assert_eq!(
            crate::olap::sha256_bytes_hex(bytes),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let manifest = Manifest {
            version: 1,
            authority: "authority".into(),
            origin: "origin".into(),
            candidate_id: 7,
            profile: Profile::Exact,
            artifact_len: bytes.len() as u64,
            artifact_digest,
        };
        let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
        let expected = ExpectedRestore {
            authority: "authority".into(),
            origin: "origin".into(),
            candidate_id: 7,
            manifest_digest: digest(&manifest_bytes),
            artifact_digest,
        };
        fs::write(&archive, bytes).unwrap();
        fs::write(&manifest_path, &manifest_bytes).unwrap();
        fs::copy(&archive, &restore).unwrap();
        let captured_manifest = fs::read(&manifest_path).unwrap();
        let restored = fs::read(&restore).unwrap();
        let mut c = candidate(artifact_digest);
        assert_eq!(plan(budget(), &[c]), Ok(Decision::Backpressure));
        c.archive =
            Some(verify_restore(Some(&captured_manifest), Some(&restored), &expected).unwrap());
        assert_eq!(
            plan(budget(), &[c]),
            Ok(Decision::Evict { id: 7, audit: 5 })
        );
        c.active_dependency = true;
        assert_eq!(plan(budget(), &[c]), Ok(Decision::Backpressure));
        c.active_dependency = false;
        fs::remove_file(&restore).unwrap();
        assert_eq!(
            verify_restore(Some(&captured_manifest), None, &expected),
            Err(RestoreError::MissingArtifact)
        );
        c.archive = verify_restore(Some(&captured_manifest), None, &expected).ok();
        assert_eq!(plan(budget(), &[c]), Ok(Decision::Backpressure));
        assert_eq!(
            verify_restore(None, Some(bytes), &expected),
            Err(RestoreError::MissingManifest)
        );
        assert_eq!(
            verify_restore(Some(&captured_manifest), Some(&bytes[..2]), &expected),
            Err(RestoreError::ArtifactDigest)
        );
        assert_eq!(
            verify_restore(Some(&captured_manifest), Some(b"abd"), &expected),
            Err(RestoreError::ArtifactDigest)
        );
        assert_eq!(
            verify_restore(Some(b"tampered"), Some(bytes), &expected),
            Err(RestoreError::ManifestDigest)
        );
        for change in 0..4 {
            let mut wrong = expected.clone();
            match change {
                0 => wrong.authority = "other".into(),
                1 => wrong.origin = "other".into(),
                2 => wrong.candidate_id = 8,
                _ => wrong.artifact_digest = [1; 32],
            }
            assert!(verify_restore(Some(&captured_manifest), Some(bytes), &wrong).is_err());
        }
        let aggregate_artifact = b"aggregate:3";
        let aggregate = Manifest {
            profile: Profile::Aggregate,
            artifact_len: aggregate_artifact.len() as u64,
            artifact_digest: digest(aggregate_artifact),
            ..manifest.clone()
        };
        assert_ne!(aggregate.artifact_digest, artifact_digest);
        let aggregate_bytes = serde_json::to_vec(&aggregate).unwrap();
        let mut aggregate_expected = expected.clone();
        aggregate_expected.manifest_digest = digest(&aggregate_bytes);
        aggregate_expected.artifact_digest = aggregate.artifact_digest;
        assert_eq!(
            verify_restore(
                Some(&aggregate_bytes),
                Some(aggregate_artifact),
                &aggregate_expected
            ),
            Err(RestoreError::UnsupportedProfile)
        );
        let mut noncanonical = captured_manifest.clone();
        noncanonical.push(b' ');
        let mut noncanonical_expected = expected.clone();
        noncanonical_expected.manifest_digest = digest(&noncanonical);
        assert_eq!(
            verify_restore(Some(&noncanonical), Some(bytes), &noncanonical_expected),
            Err(RestoreError::MalformedManifest)
        );
        assert_eq!(
            verify_restore(
                Some(&vec![0; MAX_MANIFEST_BYTES + 1]),
                Some(bytes),
                &expected
            ),
            Err(RestoreError::Size)
        );
        assert_eq!(
            verify_restore(
                Some(&captured_manifest),
                Some(&vec![0; MAX_ARTIFACT_BYTES + 1]),
                &expected
            ),
            Err(RestoreError::Size)
        );
    }
}
