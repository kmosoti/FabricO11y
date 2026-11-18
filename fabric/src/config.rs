//! Bounded service configuration readers. TOML uses a versioned document.

use rustix::fs::{Mode, OFlags, open};
use serde::de::DeserializeOwned;
use std::{
    ffi::OsStr,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

pub const MAX_BYTES: usize = 64 * 1024;
pub const VERSION: i64 = 1;

/// Load an explicitly selected configuration. A `.toml` filename selects
/// versioned TOML. Other filenames retain the legacy JSON interpretation.
pub fn load<T: DeserializeOwned>(path: &Path) -> Result<T, String> {
    load_optional(path)?.ok_or_else(|| format!("configuration file missing: {}", path.display()))
}

/// Report true absence separately from an invalid or unreadable file.
pub fn load_optional<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, String> {
    let fd = match open(
        path,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(error) => return Err(format!("open configuration {}: {error}", path.display())),
    };
    let input = File::from(fd);
    let metadata = input
        .metadata()
        .map_err(|error| format!("inspect configuration {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!(
            "configuration must be a regular file: {}",
            path.display()
        ));
    }
    if metadata.len() > MAX_BYTES as u64 {
        return Err(format!("configuration exceeds 64 KiB: {}", path.display()));
    }
    let mut bytes = Vec::new();
    input
        .take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("read configuration {}: {error}", path.display()))?;
    decode(path, &bytes).map(Some)
}

/// Prefer the versioned TOML document, falling back to JSON only if it
/// is absent. Invalid TOML does not silently activate stale JSON.
pub fn discover<T: DeserializeOwned>(
    dir: &Path,
    stem: &str,
) -> Result<Option<(PathBuf, T)>, String> {
    let primary = dir.join(format!("{stem}.toml"));
    if let Some(value) = load_optional(&primary)? {
        return Ok(Some((primary, value)));
    }
    let legacy = dir.join(format!("{stem}.json"));
    Ok(load_optional(&legacy)?.map(|value| (legacy, value)))
}

fn decode<T: DeserializeOwned>(path: &Path, bytes: &[u8]) -> Result<T, String> {
    if bytes.len() > MAX_BYTES {
        return Err(format!("configuration exceeds 64 KiB: {}", path.display()));
    }
    if path.extension() == Some(OsStr::new("toml")) {
        let invalid = || format!("invalid TOML configuration: {}", path.display());
        let raw = std::str::from_utf8(bytes).map_err(|_| invalid())?;
        let mut document: toml::Table = toml::from_str(raw).map_err(|_| invalid())?;
        if document.remove("version").and_then(|v| v.as_integer()) != Some(VERSION) {
            return Err(format!(
                "TOML configuration requires version = {VERSION}: {}",
                path.display()
            ));
        }
        // Serde and TOML diagnostics may quote user input, including unrecognized
        // credential fields. Configuration errors expose the path only.
        document.try_into().map_err(|_| invalid())
    } else {
        let invalid = || format!("invalid JSON configuration: {}", path.display());
        crate::observation::check_json(bytes, MAX_BYTES).map_err(|_| invalid())?;
        serde_json::from_slice(bytes).map_err(|_| invalid())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Sample {
        path: PathBuf,
    }

    #[test]
    fn explicit_toml_version_and_legacy_json() {
        let toml = decode::<Sample>(
            Path::new("config.toml"),
            b"version = 1\npath = '/data/config'",
        )
        .unwrap();
        let json =
            decode::<Sample>(Path::new("config.json"), br#"{"path":"/data/config"}"#).unwrap();
        assert_eq!(toml.path, json.path);
        assert!(
            decode::<Sample>(
                Path::new("config.toml"),
                b"version = 2\npath = '/data/config'"
            )
            .is_err()
        );
        assert!(decode::<Sample>(Path::new("config.toml"), b"path = '/data/config'").is_err());
    }

    #[test]
    fn spindle_examples_use_executable_schema() {
        let config_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../config");
        let standalone: crate::spindle::Config = load(&config_dir.join("spindle.toml")).unwrap();
        assert!(standalone.forwarding.is_none());
        standalone.validate().unwrap();
        let fleet: crate::spindle::Config = load(&config_dir.join("spindle-fleet.toml")).unwrap();
        assert!(!fleet.forwarding.unwrap().reclaim_confirmed);
    }

    #[test]
    fn discovery_only_falls_back_when_toml_is_absent() {
        let directory = tempfile::Builder::new()
            .tempdir_in(std::env::var_os("TMPDIR").unwrap_or_else(|| "/var/tmp".into()))
            .unwrap();
        let legacy = directory.path().join("service.json");
        let current = directory.path().join("service.toml");
        std::fs::write(&legacy, br#"{"path":"/data/legacy"}"#).unwrap();
        let (path, settings) = discover::<Sample>(directory.path(), "service")
            .unwrap()
            .unwrap();
        assert_eq!(path, legacy);
        assert_eq!(settings.path, Path::new("/data/legacy"));
        std::fs::write(&current, "version = 1\npath = '/data/current'").unwrap();
        let (path, settings) = discover::<Sample>(directory.path(), "service")
            .unwrap()
            .unwrap();
        assert_eq!(path, current);
        assert_eq!(settings.path, Path::new("/data/current"));
        std::fs::write(&current, "version = 2").unwrap();
        assert!(discover::<Sample>(directory.path(), "service").is_err());
    }

    #[test]
    fn invalid_or_oversize_input_does_not_echo_values() {
        let raw = b"version = 1\npath = '/data/config'\ncredential = 'sensitive-input'";
        let error = decode::<Sample>(Path::new("config.toml"), raw).unwrap_err();
        assert!(!error.contains("sensitive-input"));
        assert!(decode::<Sample>(Path::new("config.toml"), &vec![b' '; MAX_BYTES + 1]).is_err());
        assert!(load::<Sample>(Path::new("/dev/null")).is_err());
    }
}
