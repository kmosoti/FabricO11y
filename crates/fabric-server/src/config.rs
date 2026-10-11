//! Server configuration: one small key=value file, validated before use.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read};
use std::net::SocketAddr;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

const MAX_CONFIG_BYTES: u64 = 64 * 1024;
/// Bounded journal workspace, separate from sealed telemetry retention.
pub const DEFAULT_JOURNAL_BYTES: u64 = 20 * 1024 * 1024 * 1024;
/// Release retention default: 100 GB decimal of sealed telemetry.
pub const DEFAULT_RETENTION_BYTES: u64 = 100_000_000_000;

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

#[derive(Clone, Debug)]
pub struct Config {
    pub listen: SocketAddr,
    pub tls_cert: PathBuf,
    pub tls_key: PathBuf,
    pub state_dir: PathBuf,
    /// The legacy administrator's bearer token; only `serve-legacy` reads it.
    pub admin_token_file: Option<PathBuf>,
    pub console_dir: Option<PathBuf>,
    pub access_origin: Option<String>,
    pub access_rp_id: Option<String>,
    pub journal_bytes: u64,
    /// Size at which the active journal file is sealed for segmenting.
    pub journal_file_bytes: u64,
    /// Retention: at most this age and at most this many segment bytes.
    pub retention_s: u64,
    pub retention_bytes: u64,
    /// How history queries read their sources (`query_plan=scan|walk`, default scan);
    /// both plans return the same answers (ADR-0024).
    pub query_plan: crate::query::Plan,
    /// Segments built at once (`seal_workers`, 1 to 16; default half the CPUs, at
    /// most 4). Journal files are still reclaimed oldest first (ADR-0025).
    pub seal_workers: usize,
}

/// Production CLI options; the embedded serving primitive does not spawn processes.
#[derive(Clone, Debug, Default)]
pub struct SpindleSettings {
    pub url: Option<String>,
    pub ca: Option<PathBuf>,
    pub executable: Option<PathBuf>,
}

fn read_bounded(file: File) -> io::Result<String> {
    let mut bytes = Vec::new();
    file.take(MAX_CONFIG_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_CONFIG_BYTES {
        return Err(invalid("file exceeds 64 KiB"));
    }
    String::from_utf8(bytes).map_err(|_| invalid("file is not UTF-8"))
}

/// A credential file's trimmed text. The file must be regular, reached without
/// a symlink and readable only by its owner, or also by its group when root
/// owns it (the packaged `0640 root:fabricolly`).
pub fn read_secret(path: &Path) -> io::Result<String> {
    let file = File::options()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    let root_group = meta.uid() == 0 && meta.mode() & 0o037 == 0;
    if !meta.is_file() || (meta.mode() & 0o077 != 0 && !root_group) {
        return Err(invalid(
            "credential files must be regular, not symlinks, and readable only by their owner (0600) or, if root-owned, their group (0640)",
        ));
    }
    Ok(read_bounded(file)?.trim().to_owned())
}

impl Config {
    pub fn load(path: impl AsRef<Path>) -> io::Result<Self> {
        Self::load_with_spindle(path).map(|(server, _)| server)
    }

    pub fn load_with_spindle(path: impl AsRef<Path>) -> io::Result<(Self, SpindleSettings)> {
        let text = read_bounded(File::open(path.as_ref())?)?;
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
                    | "console_dir"
                    | "access_origin"
                    | "access_rp_id"
                    | "journal_bytes"
                    | "journal_file_bytes"
                    | "retention_s"
                    | "retention_bytes"
                    | "query_plan"
                    | "seal_workers"
                    | "self_spindle_url"
                    | "self_spindle_ca"
                    | "self_spindle_executable"
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
            admin_token_file: values
                .get("admin_token_file")
                .map(|_| path("admin_token_file"))
                .transpose()?,
            console_dir: values
                .get("console_dir")
                .map(|_| path("console_dir"))
                .transpose()?,
            access_origin: values.get("access_origin").map(|v| v.to_string()),
            access_rp_id: values.get("access_rp_id").map(|v| v.to_string()),
            journal_bytes: number("journal_bytes", DEFAULT_JOURNAL_BYTES)?,
            journal_file_bytes: number("journal_file_bytes", 64 * 1024 * 1024)?,
            retention_s: number("retention_s", 24 * 3600)?,
            retention_bytes: number("retention_bytes", DEFAULT_RETENTION_BYTES)?,
            seal_workers: match values.get("seal_workers") {
                None => crate::sealer::default_workers(),
                Some(v) => match v.parse::<usize>() {
                    Ok(n) if (1..=16).contains(&n) => n,
                    _ => return Err(invalid("seal_workers must be 1 to 16")),
                },
            },
            query_plan: match values.get("query_plan").copied() {
                None | Some("scan") => crate::query::Plan::Scan,
                Some("walk") => crate::query::Plan::Walk,
                Some(_) => return Err(invalid("query_plan must be scan or walk")),
            },
        };
        if config.access_origin.is_some() != config.access_rp_id.is_some()
            || (config.console_dir.is_some() && config.access_origin.is_none())
            || config.access_origin.as_ref().is_some_and(|v| v.is_empty())
            || config.access_rp_id.as_ref().is_some_and(|v| v.is_empty())
        {
            return Err(invalid(
                "access_origin and access_rp_id must be paired; console_dir requires access",
            ));
        }
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
        let spindle = SpindleSettings {
            url: values.get("self_spindle_url").map(|s| s.to_string()),
            ca: values
                .get("self_spindle_ca")
                .map(|_| path("self_spindle_ca"))
                .transpose()?,
            executable: values
                .get("self_spindle_executable")
                .map(|_| path("self_spindle_executable"))
                .transpose()?,
        };
        Ok((config, spindle))
    }
}

/// The legacy administrator token: printable ASCII, at least 32 bytes.
pub fn load_admin_token(path: Option<&Path>) -> io::Result<String> {
    let path = path.ok_or_else(|| invalid("serve-legacy requires admin_token_file"))?;
    let token = read_secret(path)?;
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
        let good = "listen=127.0.0.1:7443\ntls_cert=/c.pem\ntls_key=/k.pem\nstate_dir=/s\n";
        std::fs::write(&path, good).unwrap();
        let config = Config::load(&path).unwrap();
        assert_eq!(config.admin_token_file, None);
        std::fs::write(&path, format!("{good}admin_token_file=/a\n")).unwrap();
        let legacy = Config::load(&path).unwrap();
        assert_eq!(legacy.admin_token_file, Some(PathBuf::from("/a")));
        assert_eq!(config.journal_bytes, DEFAULT_JOURNAL_BYTES);
        assert_eq!(config.retention_bytes, 100_000_000_000);
        assert_eq!(config.query_plan, crate::query::Plan::Scan);
        std::fs::write(&path, format!("{good}query_plan=walk\n")).unwrap();
        assert_eq!(
            Config::load(&path).unwrap().query_plan,
            crate::query::Plan::Walk
        );
        for bad in [
            format!("{good}node_credentials=/x\n"),
            format!("{good}listen=127.0.0.1:1\n"),
            format!("{good}admin_token_file=relative\n"),
            good.replace("state_dir=/s", "state_dir=relative"),
            format!("{good}query_plan=index\n"),
            format!("{good}seal_workers=0\n"),
            format!("{good}seal_workers=17\n"),
            format!("{good}console_dir=/console\n"),
            format!("{good}access_origin=https://example.com\n"),
            format!("{good}access_rp_id=example.com\n"),
        ] {
            std::fs::write(&path, bad).unwrap();
            assert!(Config::load(&path).is_err());
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
