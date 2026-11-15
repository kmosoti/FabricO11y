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
fn clean_lexical(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Prefix(p) => out.push(p.as_os_str()),
            Component::RootDir => out.push(Component::RootDir.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !matches!(
                    out.components().next_back(),
                    Some(Component::RootDir) | None
                ) {
                    out.pop();
                }
            }
            Component::Normal(part) => out.push(part),
        }
    }
    out
}

fn is_disallowed_ram_tmp(path: &Path) -> bool {
    path.as_os_str().is_empty()
        || path.starts_with("/tmp")
        || path.starts_with("/dev/shm")
        || path.starts_with("/run/shm")
}

pub fn non_tmp_base_dir() -> PathBuf {
    let probe = |dir: &Path| -> bool {
        if dir.as_os_str().is_empty() || !dir.is_absolute() {
            return false;
        }
        let cleaned = clean_lexical(dir);
        if is_disallowed_ram_tmp(&cleaned) {
            return false;
        }
        let created_dir = || {
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(&cleaned)
                    .is_ok()
            }
            #[cfg(not(unix))]
            {
                fs::create_dir_all(&cleaned).is_ok()
            }
        };
        if !(cleaned.is_dir() || created_dir()) {
            return false;
        }
        if let Ok(canon) = fs::canonicalize(&cleaned)
            && is_disallowed_ram_tmp(&canon)
        {
            return false;
        }
        tempfile::tempfile_in(&cleaned).is_ok()
    };
    if let Some(env_dir) = std::env::var_os("TMPDIR")
        && !env_dir.is_empty()
    {
        let p = PathBuf::from(env_dir);
        if probe(&p) {
            return clean_lexical(&p);
        }
    }
    let data_tmp = PathBuf::from("/data/tmp");
    if probe(&data_tmp) {
        return data_tmp;
    }
    if let Some(home) = std::env::var_os("HOME")
        && !home.is_empty()
    {
        let home_tmp = PathBuf::from(home).join(".cache").join("fabrico11y-tmp");
        if probe(&home_tmp) {
            return home_tmp;
        }
    }
    let var_tmp = PathBuf::from("/var/tmp");
    let _ = fs::create_dir_all(&var_tmp);
    var_tmp
}

pub fn non_tmp_tempdir() -> Result<tempfile::TempDir, String> {
    tempfile::tempdir_in(non_tmp_base_dir()).map_err(|e| e.to_string())
}

pub fn non_tmp_named_tempfile() -> Result<tempfile::NamedTempFile, String> {
    tempfile::NamedTempFile::new_in(non_tmp_base_dir()).map_err(|e| e.to_string())
}

pub fn non_tmp_unlinked_file() -> Result<fs::File, String> {
    tempfile::tempfile_in(non_tmp_base_dir()).map_err(|e| e.to_string())
}

pub fn hash_bytes(bytes: &[u8]) -> Result<String, String> {
    use std::io::Write;
    let mut file = non_tmp_named_tempfile()?;
    file.write_all(bytes).map_err(|e| e.to_string())?;
    hash(file.path())
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
        let root = super::non_tmp_tempdir().unwrap();
        let external = super::non_tmp_tempdir().unwrap();
        std::os::unix::fs::symlink(root.path(), external.path().join("link")).unwrap();
        assert!(
            super::artifact_path(&external.path().join("link/result"), root.path(), false).is_err()
        );
        assert!(super::artifact_path(&external.path().join("result"), root.path(), false).is_ok());
    }
}
