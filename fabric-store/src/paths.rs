//! Disk-aware path validation resolving POSIX symlinks component-by-component.

use fabric_core::paths::{check_forbidden_ram_prefix, normalize_lexical_path};
use fabric_core::{PathPolicyError, StorageLayout};
use std::collections::VecDeque;
use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};

const MAX_SYMLINK_HOPS: usize = 40;

fn build_abs_path(parts: &[OsString]) -> PathBuf {
    let mut out = PathBuf::from("/");
    for part in parts {
        out.push(part);
    }
    out
}

/// Validates and resolves `path` on disk, returning the symlink-resolved path.
///
/// Rejects relative paths, paths lexically under `/tmp`, `/dev/shm`, or `/run/shm`,
/// and any live, relative, dangling, or `..`-traversed symlink targeting them.
pub fn validate_non_tmp_path_on_disk(path: &Path) -> Result<PathBuf, PathPolicyError> {
    let normalized = normalize_lexical_path(path)
        .ok_or_else(|| PathPolicyError::NotAbsolute(path.display().to_string()))?;
    check_forbidden_ram_prefix(&normalized)?;

    let mut resolved_parts: Vec<OsString> = Vec::new();
    let mut remaining: VecDeque<OsString> = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_os_string()),
            Component::ParentDir => Some(OsString::from("..")),
            Component::RootDir | Component::CurDir | Component::Prefix(_) => None,
        })
        .collect();
    let mut hops = 0usize;

    while let Some(seg) = remaining.pop_front() {
        if seg == "." {
            continue;
        }
        if seg == ".." {
            let _ = resolved_parts.pop();
            continue;
        }
        resolved_parts.push(seg);
        let candidate = build_abs_path(&resolved_parts);
        check_forbidden_ram_prefix(&candidate)?;
        if let Ok(meta) = fs::symlink_metadata(&candidate)
            && meta.file_type().is_symlink()
        {
            hops += 1;
            if hops > MAX_SYMLINK_HOPS {
                return Err(PathPolicyError::ForbiddenRamMount(format!(
                    "symlink loop exceeded {MAX_SYMLINK_HOPS} hops: {}",
                    path.display()
                )));
            }
            let target = fs::read_link(&candidate)
                .map_err(|_| PathPolicyError::ForbiddenRamMount(candidate.display().to_string()))?;
            let _ = resolved_parts.pop();
            if target.is_absolute() {
                resolved_parts.clear();
            }
            let target_parts: Vec<OsString> = target
                .components()
                .filter_map(|c| match c {
                    Component::Normal(s) => Some(s.to_os_string()),
                    Component::ParentDir => Some(OsString::from("..")),
                    Component::RootDir | Component::CurDir | Component::Prefix(_) => None,
                })
                .collect();
            for part in target_parts.into_iter().rev() {
                remaining.push_front(part);
            }
        }
    }

    let resolved = build_abs_path(&resolved_parts);
    check_forbidden_ram_prefix(&resolved)?;
    Ok(resolved)
}

/// Creates a `StorageLayout` whose `data_dir` is resolved across symlinks on disk.
pub fn validated_storage_layout(data_dir: impl AsRef<Path>) -> Result<StorageLayout, String> {
    let resolved = validate_non_tmp_path_on_disk(data_dir.as_ref()).map_err(|e| e.to_string())?;
    StorageLayout::new(resolved).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric_core::DEFAULT_TMP_DIR;
    use tempfile::TempDir;

    fn non_tmp_dir() -> TempDir {
        if let Some(env_dir) = std::env::var_os("TMPDIR") {
            let p = PathBuf::from(env_dir);
            if !p.starts_with("/tmp")
                && !p.starts_with("/dev/shm")
                && !p.starts_with("/run/shm")
                && fs::create_dir_all(&p).is_ok()
                && let Ok(td) = tempfile::tempdir_in(&p)
            {
                return td;
            }
        }
        let fallback = Path::new(DEFAULT_TMP_DIR);
        let _ = fs::create_dir_all(fallback);
        if let Ok(td) = tempfile::tempdir_in(fallback) {
            return td;
        }
        let vt = Path::new("/var/tmp");
        tempfile::tempdir_in(vt).expect("non-tmp tempdir")
    }

    #[test]
    fn rejects_live_dangling_and_dotdot_symlinks_and_returns_resolved_path() {
        let root = non_tmp_dir();
        let canonical_root = fs::canonicalize(root.path()).expect("canonical test root");

        let live_link = root.path().join("live-to-tmp");
        std::os::unix::fs::symlink("/tmp", &live_link).expect("create live symlink");
        assert!(validate_non_tmp_path_on_disk(&live_link).is_err());
        assert!(validate_non_tmp_path_on_disk(&live_link.join("nested/child")).is_err());

        let dangling_link = root.path().join("dangling-to-tmp");
        std::os::unix::fs::symlink("/tmp/nonexistent-fabric-target", &dangling_link)
            .expect("create dangling symlink");
        assert!(validate_non_tmp_path_on_disk(&dangling_link).is_err());
        assert!(validate_non_tmp_path_on_disk(&dangling_link.join("sub")).is_err());
        assert!(validate_non_tmp_path_on_disk(&dangling_link.join("..")).is_err());

        // Safe symlink + `..` traversal must resolve to `/safe/out`, NOT lexical `/out`,
        // even when `/out` is a separate symlink pointing to `/tmp`.
        let safe_child = root.path().join("safe/child");
        let safe_out = canonical_root.join("safe/out");
        fs::create_dir_all(&safe_child).expect("create safe/child");
        fs::create_dir_all(&safe_out).expect("create safe/out");

        let link = root.path().join("link");
        std::os::unix::fs::symlink(&safe_child, &link).expect("create link -> safe/child");
        let poisoned_lexical_out = root.path().join("out");
        std::os::unix::fs::symlink("/tmp/spill-bypass", &poisoned_lexical_out)
            .expect("create out -> /tmp/spill-bypass");

        let resolved =
            validate_non_tmp_path_on_disk(&link.join("../out")).expect("must resolve via safe");
        assert_eq!(resolved, safe_out);

        let layout = validated_storage_layout(link.join("../out")).expect("layout");
        assert_eq!(layout.data_dir(), safe_out.as_path());
    }
}
