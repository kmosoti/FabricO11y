//! Local projection-owner chunk verification; this is not an access ticket.
//!
//! A caller must authenticate and authorize the whole-origin reader before
//! invoking this seam and supply the active canonical authority. No network
//! handler, caller identity, ticket, lifetime, or durable availability is
//! established here. Segments are rebuildable projection artifacts, not custody.
use crate::{MAX_RECEIPT_SEGMENT_BYTES, Projection, err, hex};
use sha2::{Digest, Sha256};
use std::{
    fs,
    fs::File,
    io::Read,
    os::unix::fs::MetadataExt,
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};

pub const MAX_SEGMENT_CHUNK_BYTES: usize = 64 * 1024;
/// Bound manifest scans independently of artifact and output bytes.
pub const MAX_SEGMENT_SCAN: usize = 4096;

/// The digest identifies a manifest entry within the already-authorized origin.
/// It is never interpreted as a file name or path.
pub struct SegmentChunkRequest<'a> {
    pub active_authority: &'a str,
    pub origin: &'a str,
    pub digest: &'a str,
    pub offset: u64,
    pub length: u32,
}

impl Projection {
    /// Verify a bounded complete segment and return only bytes from that same
    /// hashing pass. The deadline and cancellation are checked between reads;
    /// an individual blocking filesystem read cannot be preempted here.
    pub fn read_verified_segment_chunk(
        &self,
        request: &SegmentChunkRequest<'_>,
        cancel: &AtomicBool,
        deadline: Instant,
    ) -> Result<Vec<u8>, String> {
        let budget = || {
            if cancel.load(Ordering::Acquire) || Instant::now() >= deadline {
                Err("projection segment read interrupted".to_owned())
            } else {
                Ok(())
            }
        };
        budget()?;
        if self.poisoned
            || request.active_authority.is_empty()
            || self.manifest.authority.as_deref() != Some(request.active_authority)
            || request.origin.is_empty()
            || request.origin.len() > 256
            || request.digest.len() != 64
            || !request
                .digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("projection segment unavailable".into());
        }
        if self.manifest.segments.len() > MAX_SEGMENT_SCAN {
            return Err("projection segment unavailable".into());
        }
        let mut selected =
            self.manifest.segments.iter().filter(|segment| {
                segment.origin == request.origin && segment.digest == request.digest
            });
        let segment = selected.next().ok_or("projection segment unavailable")?;
        if selected.next().is_some()
            || segment.bytes == 0
            || segment.bytes > MAX_RECEIPT_SEGMENT_BYTES
        {
            return Err("projection segment unavailable".into());
        }
        let length = usize::try_from(request.length).map_err(err)?;
        let end = request
            .offset
            .checked_add(u64::from(request.length))
            .ok_or("invalid segment range")?;
        if length == 0 || length > MAX_SEGMENT_CHUNK_BYTES || end > segment.bytes {
            return Err("invalid segment range".into());
        }
        use rustix::fs::{Mode, OFlags};
        let path = self.dir.join(&segment.file);
        let handle = rustix::fs::open(
            &path,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| "projection segment unavailable")?;
        let mut file = File::from(handle);
        let metadata = file
            .metadata()
            .map_err(|_| "projection segment unavailable")?;
        if !metadata.is_file()
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.len() != segment.bytes
        {
            return Err("projection segment unavailable".into());
        }
        let mut bytes = Vec::with_capacity(length);
        let mut hasher = Sha256::new();
        let mut buffer = [0; 64 * 1024];
        let mut position = 0u64;
        loop {
            budget()?;
            let count = file
                .read(&mut buffer)
                .map_err(|_| "projection segment unavailable")?;
            budget()?;
            if count == 0 {
                break;
            }
            let next = position
                .checked_add(count as u64)
                .ok_or("projection segment unavailable")?;
            if next > segment.bytes {
                return Err("projection segment unavailable".into());
            }
            hasher.update(&buffer[..count]);
            let begin = position.max(request.offset);
            let stop = next.min(end);
            if begin < stop {
                bytes.extend_from_slice(
                    &buffer[(begin - position) as usize..(stop - position) as usize],
                );
            }
            position = next;
        }
        budget()?;
        let current = fs::symlink_metadata(&path).map_err(|_| "projection segment unavailable")?;
        if !current.is_file()
            || current.dev() != metadata.dev()
            || current.ino() != metadata.ino()
            || current.len() != segment.bytes
            || position != segment.bytes
            || bytes.len() != length
            || hex(&hasher.finalize()) != segment.digest
        {
            return Err("projection segment unavailable".into());
        }
        budget()?;
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric::canonical::CanonicalRow;
    use std::{os::unix::fs::PermissionsExt, time::Duration};

    fn fixture() -> (tempfile::TempDir, Projection, String, String, Vec<u8>) {
        let root = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(std::env::var_os("TMPDIR").unwrap())
            .unwrap();
        let mut projection = Projection::open(root.path(), 32 * 1024 * 1024).unwrap();
        projection.bind_authority("authority").unwrap();
        let raw = include_str!("../tests/fixtures/mixed.jsonl")
            .lines()
            .next()
            .unwrap();
        projection
            .apply(&[CanonicalRow {
                canonical_id: 1,
                origin: "alpha".into(),
                source_id: 1,
                raw: raw.into(),
            }])
            .unwrap();
        let segment = &projection.manifest.segments[0];
        let name = segment.file.clone();
        let digest = segment.digest.clone();
        let bytes = fs::read(root.path().join(&name)).unwrap();
        (root, projection, name, digest, bytes)
    }

    fn request(digest: &str, offset: u64, length: u32) -> SegmentChunkRequest<'_> {
        SegmentChunkRequest {
            active_authority: "authority",
            origin: "alpha",
            digest,
            offset,
            length,
        }
    }

    fn read(projection: &Projection, request: &SegmentChunkRequest<'_>) -> Result<Vec<u8>, String> {
        projection.read_verified_segment_chunk(
            request,
            &AtomicBool::new(false),
            Instant::now() + Duration::from_secs(10),
        )
    }

    #[test]
    fn actual_projection_segment_resumes_from_verified_chunks() {
        let (_root, projection, _name, digest, original) = fixture();
        let split = (original.len() / 2).max(1);
        assert!(split < original.len());
        let mut combined = read(&projection, &request(&digest, 0, split as u32)).unwrap();
        combined.extend(
            read(
                &projection,
                &request(&digest, split as u64, (original.len() - split) as u32),
            )
            .unwrap(),
        );
        assert_eq!(combined, original);
    }

    #[test]
    fn scope_range_budget_and_ambiguous_membership_fail_closed() {
        let (_root, mut projection, _name, digest, original) = fixture();
        let mut q = request(&digest, 0, 1);
        q.active_authority = "other";
        assert!(read(&projection, &q).is_err());
        q.active_authority = "authority";
        q.origin = "other";
        assert!(read(&projection, &q).is_err());
        q.origin = "alpha";
        q.digest = "bad";
        assert!(read(&projection, &q).is_err());
        q.digest = &digest;
        for (offset, length) in [
            (u64::MAX, 1),
            (original.len() as u64, 1),
            (0, 0),
            (0, MAX_SEGMENT_CHUNK_BYTES as u32 + 1),
        ] {
            q.offset = offset;
            q.length = length;
            assert!(read(&projection, &q).is_err());
        }
        q.offset = 0;
        q.length = 1;
        assert!(
            projection
                .read_verified_segment_chunk(
                    &q,
                    &AtomicBool::new(true),
                    Instant::now() + Duration::from_secs(1)
                )
                .is_err()
        );
        assert!(
            projection
                .read_verified_segment_chunk(&q, &AtomicBool::new(false), Instant::now())
                .is_err()
        );
        projection
            .manifest
            .segments
            .push(projection.manifest.segments[0].clone());
        assert!(read(&projection, &q).is_err());
        projection.manifest.segments.pop();
        projection.manifest.segments[0].bytes = MAX_RECEIPT_SEGMENT_BYTES + 1;
        assert!(read(&projection, &q).is_err());
        projection.manifest.segments[0].bytes = original.len() as u64;
        let copy = projection.manifest.segments[0].clone();
        projection
            .manifest
            .segments
            .resize(MAX_SEGMENT_SCAN + 1, copy);
        assert!(read(&projection, &q).is_err());
    }

    #[test]
    fn corrupt_replaced_and_missing_segment_never_returns_chunk() {
        let (root, projection, name, digest, original) = fixture();
        let path = root.path().join(&name);
        let q = request(&digest, 0, 1);
        let mut corrupt = original.clone();
        corrupt[0] ^= 1;
        fs::write(&path, &corrupt).unwrap();
        assert!(read(&projection, &q).is_err());
        fs::write(&path, &original[..original.len() - 1]).unwrap();
        assert!(read(&projection, &q).is_err());
        fs::remove_file(&path).unwrap();
        assert!(read(&projection, &q).is_err());
        let target = root.path().join("substitute");
        fs::write(&target, &original).unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(read(&projection, &q).is_err());
    }
}
