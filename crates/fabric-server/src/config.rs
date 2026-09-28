//! Server configuration: one small key=value file, validated before use.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

const MAX_CONFIG_BYTES: u64 = 64 * 1024;
/// Default journal ceiling: the retention floor of 20 GiB from the contract.
pub const DEFAULT_JOURNAL_BYTES: u64 = 20 * 1024 * 1024 * 1024;

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

#[derive(Clone, Debug)]
pub struct Config {
    pub listen: SocketAddr,
    pub tls_cert: PathBuf,
    pub tls_key: PathBuf,
    pub state_dir: PathBuf,
    /// Root-owned file holding the administrator's bearer token.
    pub admin_token_file: PathBuf,
    pub journal_bytes: u64,
    /// Size at which the active journal file is sealed for segmenting.
    pub journal_file_bytes: u64,
    /// Retention: at most this age and at most this many segment bytes.
    pub retention_s: u64,
    pub retention_bytes: u64,
}

fn read_bounded(path: &Path) -> io::Result<String> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_CONFIG_BYTES {
        return Err(invalid("file exceeds 64 KiB"));
    }
    String::from_utf8(bytes).map_err(|_| invalid("file is not UTF-8"))
}

impl Config {
    pub fn load(path: impl AsRef<Path>) -> io::Result<Self> {
        let text = read_bounded(path.as_ref())?;
        let mut values: HashMap<&str, &str> = HashMap::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (key, value) = line
                .split_once('=')
                .ok_or_else(|| invalid("invalid server config line"))?;
            let key = key.trim();
            if !matches!(
                key,
                "listen"
                    | "tls_cert"
                    | "tls_key"
                    | "state_dir"
                    | "admin_token_file"
                    | "journal_bytes"
                    | "journal_file_bytes"
                    | "retention_s"
                    | "retention_bytes"
            ) {
                return Err(invalid("unknown server config key"));
            }
            if values.insert(key, value.trim()).is_some() {
                return Err(invalid("duplicate server config key"));
            }
        }
        let required = |key: &str| {
            values
                .get(key)
                .copied()
                .ok_or_else(|| invalid("missing required server config key"))
        };
        let path = |key: &str| -> io::Result<PathBuf> {
            let value = PathBuf::from(required(key)?);
            if !value.is_absolute() || value.as_os_str().len() > 240 {
                return Err(invalid("server config paths must be absolute and short"));
            }
            Ok(value)
        };
        let number = |key: &str, default: u64| -> io::Result<u64> {
            match values.get(key) {
                Some(v) => v
                    .parse()
                    .map_err(|_| invalid("invalid number in server config")),
                None => Ok(default),
            }
        };
        let config = Config {
            listen: required("listen")?
                .parse()
                .map_err(|_| invalid("invalid listen address"))?,
            tls_cert: path("tls_cert")?,
            tls_key: path("tls_key")?,
            state_dir: path("state_dir")?,
            admin_token_file: path("admin_token_file")?,
            journal_bytes: number("journal_bytes", DEFAULT_JOURNAL_BYTES)?,
            journal_file_bytes: number("journal_file_bytes", 64 * 1024 * 1024)?,
            retention_s: number("retention_s", 24 * 3600)?,
            retention_bytes: number("retention_bytes", 20 * 1024 * 1024 * 1024)?,
        };
        if config.journal_file_bytes < 64 * 1024
            || config.journal_file_bytes > config.journal_bytes
            || config.retention_s == 0
        {
            return Err(invalid(
                "journal_file_bytes or retention outside allowed range",
            ));
        }
        if config.journal_bytes < 1024 * 1024 {
            return Err(invalid("journal_bytes below 1 MiB"));
        }
        Ok(config)
    }
}

/// The administrator token: printable ASCII, at least 32 bytes.
pub fn load_admin_token(path: &Path) -> io::Result<String> {
    let token = read_bounded(path)?.trim().to_owned();
    if token.len() < 32 || token.len() > 256 || !token.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(invalid("admin token must be 32-256 printable ASCII bytes"));
    }
    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_complete_file_loads_and_unknown_or_duplicate_keys_are_refused() {
        let dir = std::env::temp_dir().join(format!("fabric-server-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("server.conf");
        let good = "listen=127.0.0.1:7443\ntls_cert=/c.pem\ntls_key=/k.pem\nstate_dir=/s\nadmin_token_file=/a\n";
        std::fs::write(&path, good).unwrap();
        let config = Config::load(&path).unwrap();
        assert_eq!(config.admin_token_file, PathBuf::from("/a"));
        assert_eq!(config.journal_bytes, DEFAULT_JOURNAL_BYTES);
        for bad in [
            format!("{good}node_credentials=/x\n"),
            format!("{good}listen=127.0.0.1:1\n"),
            good.replace("admin_token_file=/a\n", ""),
            good.replace("state_dir=/s", "state_dir=relative"),
        ] {
            std::fs::write(&path, bad).unwrap();
            assert!(Config::load(&path).is_err());
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
