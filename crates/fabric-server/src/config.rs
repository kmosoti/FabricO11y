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
    /// Lines of `<sha256 hex of token> <label>`; phase 3 replaces this with enrollment.
    pub node_credentials: PathBuf,
    pub journal_bytes: u64,
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
                    | "node_credentials"
                    | "journal_bytes"
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
        let config = Config {
            listen: required("listen")?
                .parse()
                .map_err(|_| invalid("invalid listen address"))?,
            tls_cert: path("tls_cert")?,
            tls_key: path("tls_key")?,
            state_dir: path("state_dir")?,
            node_credentials: path("node_credentials")?,
            journal_bytes: match values.get("journal_bytes") {
                Some(v) => v.parse().map_err(|_| invalid("invalid journal_bytes"))?,
                None => DEFAULT_JOURNAL_BYTES,
            },
        };
        if config.journal_bytes < 1024 * 1024 {
            return Err(invalid("journal_bytes below 1 MiB"));
        }
        Ok(config)
    }
}

/// Credentials: SHA-256 of a bearer token (lowercase hex) to a node label.
pub fn load_credentials(path: &Path) -> io::Result<HashMap<[u8; 32], String>> {
    let text = read_bounded(path)?;
    let mut map = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (hash, label) = line
            .split_once(' ')
            .ok_or_else(|| invalid("credential line must be '<sha256 hex> <label>'"))?;
        let label = label.trim();
        if hash.len() != 64
            || !hash
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            || label.is_empty()
            || label.len() > 64
            || !label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
        {
            return Err(invalid("invalid credential line"));
        }
        let mut digest = [0_u8; 32];
        for (i, chunk) in hash.as_bytes().chunks(2).enumerate() {
            digest[i] = u8::from_str_radix(std::str::from_utf8(chunk).unwrap(), 16).unwrap();
        }
        if map.insert(digest, label.to_owned()).is_some()
            || map.values().filter(|l| *l == label).count() > 1
        {
            return Err(invalid("duplicate credential or label"));
        }
    }
    Ok(map)
}
