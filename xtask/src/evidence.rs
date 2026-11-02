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
