//! Sans-I/O path safety policy enforcing zero `/tmp` or `/dev/shm` usage.

use std::fmt;
use std::path::{Component, Path, PathBuf};

/// Default converged telemetry data directory on `/data`.
pub const DEFAULT_DATA_DIR: &str = "/data/fabrico11y";

/// Default non-tmpfs temporary scratch directory on `/data`.
pub const DEFAULT_TMP_DIR: &str = "/data/tmp";

const FORBIDDEN_RAM_PREFIXES: &[&str] = &["/tmp", "/dev/shm", "/run/shm"];

/// Error raised when a configured path violates non-tmpfs storage policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathPolicyError {
    NotAbsolute(String),
    ForbiddenRamMount(String),
}

impl fmt::Display for PathPolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAbsolute(path) => write!(f, "path must be absolute: {path}"),
            Self::ForbiddenRamMount(path) => {
                write!(
                    f,
                    "RAM-backed path is prohibited by self-preservation policy: {path}"
                )
            }
        }
    }
}

impl std::error::Error for PathPolicyError {}

/// Lexically normalizes an absolute path without accessing the filesystem.
#[must_use]
pub fn normalize_lexical_path(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let mut parts: Vec<std::ffi::OsString> = Vec::new();
    for component in path.components() {
        match component {
            Component::RootDir | Component::CurDir => {}
            Component::ParentDir => {
                let _ = parts.pop();
            }
            Component::Normal(seg) => parts.push(seg.to_os_string()),
            Component::Prefix(_) => return None,
        }
    }
    let mut out = PathBuf::from("/");
    for part in parts {
        out.push(part);
    }
    Some(out)
}

/// Rejects relative paths and paths residing under `/tmp`, `/dev/shm`, or `/run/shm`.
pub fn validate_non_tmp_path(path: &Path) -> Result<PathBuf, PathPolicyError> {
    let normalized = normalize_lexical_path(path)
        .ok_or_else(|| PathPolicyError::NotAbsolute(path.display().to_string()))?;
    for forbidden in FORBIDDEN_RAM_PREFIXES {
        let forbidden_path = Path::new(forbidden);
        if normalized == forbidden_path || normalized.starts_with(forbidden_path) {
            return Err(PathPolicyError::ForbiddenRamMount(
                normalized.display().to_string(),
            ));
        }
    }
    Ok(normalized)
}

/// Deterministic validated layout beneath `FABRIC_DATA_DIR`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageLayout {
    data_dir: PathBuf,
}

impl StorageLayout {
    /// Creates a validated storage layout, refusing `/tmp` and `/dev/shm` paths.
    pub fn new(data_dir: impl AsRef<Path>) -> Result<Self, PathPolicyError> {
        let normalized = validate_non_tmp_path(data_dir.as_ref())?;
        Ok(Self {
            data_dir: normalized,
        })
    }

    #[must_use]
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    #[must_use]
    pub fn state_dir(&self) -> PathBuf {
        self.data_dir.join("state")
    }

    #[must_use]
    pub fn catalog_redb(&self) -> PathBuf {
        self.state_dir().join("catalog.redb")
    }

    #[must_use]
    pub fn live_state_json(&self) -> PathBuf {
        self.state_dir().join("live_state.json")
    }

    #[must_use]
    pub fn staging_dir(&self) -> PathBuf {
        self.state_dir().join("staging")
    }

    #[must_use]
    pub fn spill_dir(&self) -> PathBuf {
        self.state_dir().join("spill")
    }

    #[must_use]
    pub fn olap_dir(&self) -> PathBuf {
        self.data_dir.join("olap")
    }

    #[must_use]
    pub fn olap_domain_dir(&self, domain: &str) -> PathBuf {
        self.olap_dir().join(domain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_tmp_and_shm_even_with_traversal() {
        assert!(validate_non_tmp_path(Path::new("/tmp")).is_err());
        assert!(validate_non_tmp_path(Path::new("/tmp/fabric-rust-1100")).is_err());
        assert!(validate_non_tmp_path(Path::new("/data/../tmp/foo")).is_err());
        assert!(validate_non_tmp_path(Path::new("/dev/shm/ring")).is_err());
        assert!(validate_non_tmp_path(Path::new("relative/dir")).is_err());
    }

    #[test]
    fn accepts_data_paths_and_computes_subdirectories() {
        let layout = StorageLayout::new("/data/fabrico11y/../fabrico11y").unwrap();
        assert_eq!(layout.data_dir(), Path::new("/data/fabrico11y"));
        assert_eq!(
            layout.catalog_redb(),
            Path::new("/data/fabrico11y/state/catalog.redb")
        );
        assert_eq!(
            layout.olap_domain_dir("metrics"),
            Path::new("/data/fabrico11y/olap/metrics")
        );
    }
}
