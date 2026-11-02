use serde::{Serialize, de::DeserializeOwned};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

pub fn hash(path: &Path) -> Result<String, String> {
    let bytes = Command::new("sha256sum")
        .arg("--")
        .arg(path)
        .output()
        .map_err(|e| e.to_string())?;
    if !bytes.status.success() {
        return Err(String::from_utf8_lossy(&bytes.stderr).into());
    }
    Ok(String::from_utf8_lossy(&bytes.stdout)
        .split_whitespace()
        .next()
        .ok_or("missing digest")?
        .to_owned())
}
pub fn read<T: DeserializeOwned>(path: &Path) -> Result<T, String> {
    serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}
pub fn write<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.write_all(&serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}
/// Resolve the parent before enforcing the artifact boundary, including symlinks.
pub fn artifact_path(path: &Path, root: &Path, allow_local: bool) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("artifact paths must be absolute".into());
    }
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let parent = path
        .parent()
        .ok_or("artifact has no parent")?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let resolved = parent.join(path.file_name().ok_or("artifact has no name")?);
    if resolved.starts_with(&root) && !(allow_local && resolved.starts_with(root.join(".local"))) {
        return Err("artifacts must be outside tracked source".into());
    }
    Ok(resolved)
}

pub fn tools() -> PathBuf {
    std::env::var_os("FABRIC_PROOF_TOOLS")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
                .join(".local/share/fabrico11y/proof-tools")
        })
}
pub fn binary(variable: &str, relative: &str) -> PathBuf {
    std::env::var_os(variable)
        .map(PathBuf::from)
        .unwrap_or_else(|| tools().join(relative))
}

#[cfg(test)]
mod tests {
    #[test]
    fn artifact_boundary_resolves_parent_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(root.path(), external.path().join("link")).unwrap();
        assert!(
            super::artifact_path(&external.path().join("link/result"), root.path(), false).is_err()
        );
        assert!(super::artifact_path(&external.path().join("result"), root.path(), false).is_ok());
    }
}
