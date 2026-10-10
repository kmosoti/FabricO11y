use fabric_o11y::spindle::runtime::{Config, inspect};
use fabric_o11y::spindle::sender::{agent, read_token};
use std::io::{self, Read};
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage:
  fabricctl inspect <NODE_CONFIG>
  fabricctl access <ACCESS_CONFIG> node list|add|config|pause|resume|revoke ...
  fabricctl access <ACCESS_CONFIG> query '<QUERY_JSON>'
  fabricctl admin <ADMIN_CONFIG> node list
  fabricctl admin <ADMIN_CONFIG> node add <NAME> [--log PATH]... [--interval SECONDS]
  fabricctl admin <ADMIN_CONFIG> node config <NAME> [--log PATH]... [--interval SECONDS]
  fabricctl admin <ADMIN_CONFIG> node pause|resume|revoke <NAME>
  fabricctl admin <ADMIN_CONFIG> query '<QUERY_JSON>'";

fn hex_id(id: &[u8; 16]) -> String {
    id.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

/// Mode-specific HTTPS client settings; credentials are never selected by fallback.
struct Admin {
    url: String,
    ca: PathBuf,
    token_file: PathBuf,
}

fn parse_client(text: &str, scoped: bool) -> io::Result<Admin> {
    let token_key = if scoped {
        "workload_token_file"
    } else {
        "admin_token_file"
    };
    let (mut url, mut ca, mut token_file) = (None, None, None);
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| invalid("invalid client config line"))?;
        let value = value.trim().to_owned();
        match key.trim() {
            "server_url" if url.is_none() => url = Some(value),
            "server_ca" if ca.is_none() => ca = Some(PathBuf::from(value)),
            key if key == token_key && token_file.is_none() => {
                token_file = Some(PathBuf::from(value))
            }
            _ => return Err(invalid("unknown or duplicate client config key")),
        }
    }
    match (url, ca, token_file) {
        (Some(url), Some(ca), Some(token_file)) => {
            validate_origin(&url)?;
            if !ca.is_absolute() || !token_file.is_absolute() {
                return Err(invalid("server_ca and token file must be absolute paths"));
            }
            Ok(Admin {
                url,
                ca,
                token_file,
            })
        }
        _ => Err(invalid(
            "client config needs server_url=https://..., server_ca and its mode-specific token file",
        )),
    }
}

fn validate_origin(url: &str) -> io::Result<()> {
    let uri: ureq::http::Uri = url.parse().map_err(|_| invalid("invalid server_url"))?;
    let authority = uri
        .authority()
        .ok_or_else(|| invalid("server_url needs a host"))?;
    if uri.scheme_str() != Some("https")
        || authority.host().is_empty()
        || url.len() > 240
        || authority.as_str().contains('@')
        || url
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '\\' | '#' | '?' | '%'))
        || url != format!("https://{authority}")
    {
        return Err(invalid(
            "server_url must be an HTTPS origin without path, credentials, query or fragment",
        ));
    }
    Ok(())
}

fn load_client(path: &str, scoped: bool) -> io::Result<Admin> {
    let mut text = String::new();
    std::fs::File::open(path)?
        .take(64 * 1024 + 1)
        .read_to_string(&mut text)?;
    if text.len() > 64 * 1024 {
        return Err(invalid("client config exceeds 64 KiB"));
    }
    parse_client(&text, scoped)
}

fn workload_token(path: &std::path::Path) -> io::Result<String> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.permissions().mode() & 0o077 != 0 {
        return Err(invalid(
            "workload token must be a regular file with owner-only permissions (0600)",
        ));
    }
    let mut token = String::new();
    (&mut file).take(4097).read_to_string(&mut token)?;
    if token.len() > 4096 {
        return Err(invalid("workload token file exceeds 4 KiB"));
    }
    let token = token.trim();
    if token.len() != 64 || !token.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(invalid("invalid workload token file"));
    }
    Ok(token.to_owned())
}

fn node_name(name: &str) -> io::Result<()> {
    if name.is_empty()
        || name.len() > 64
        || matches!(name, "." | "..")
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err(invalid(
            "node name must be 1-64 of [A-Za-z0-9._-], excluding dot path segments",
        ));
    }
    Ok(())
}

/// `--log PATH` (repeatable) and `--interval SECONDS` into a desired configuration.
fn desired(args: &[String]) -> io::Result<serde_json::Value> {
    let mut logs = Vec::new();
    let mut interval = 15_u64;
    let mut rest = args.iter();
    while let Some(flag) = rest.next() {
        let value = rest.next().ok_or_else(|| invalid("flag needs a value"))?;
        match flag.as_str() {
            "--log" => logs.push(value.clone()),
            "--interval" => interval = value.parse().map_err(|_| invalid("invalid --interval"))?,
            _ => return Err(invalid("unknown flag")),
        }
    }
    Ok(serde_json::json!({"logs": logs, "metric_interval_s": interval}))
}

fn remote(config_path: &str, words: &[String], scoped: bool) -> io::Result<bool> {
    let admin = load_client(config_path, scoped)?;
    let token = if scoped {
        workload_token(&admin.token_file)?
    } else {
        read_token(&admin.token_file)?
    };
    let client = agent(&admin.ca)?;
    let auth = format!("Bearer {token}");
    let namespace = if scoped { "console" } else { "admin" };
    let url = |tail: &str| format!("{}/v1/{namespace}/nodes{tail}", admin.url);
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    let result = match words.as_slice() {
        ["node", "list"] => client
            .get(&url(""))
            .config()
            .max_redirects(0)
            .build()
            .header("authorization", &auth)
            .header("x-fabric-client-version", "1")
            .call(),
        ["node", "add", name, flags @ ..] => {
            node_name(name)?;
            let flags: Vec<String> = flags.iter().map(|s| s.to_string()).collect();
            let mut body = desired(&flags)?;
            body["name"] = serde_json::Value::String(name.to_string());
            client
                .post(&url(""))
                .config()
                .max_redirects(0)
                .build()
                .header("authorization", &auth)
                .header("x-fabric-client-version", "1")
                .header("content-type", "application/json")
                .send(body.to_string())
        }
        ["node", "config", name, flags @ ..] => {
            node_name(name)?;
            let flags: Vec<String> = flags.iter().map(|s| s.to_string()).collect();
            client
                .put(&url(&format!("/{name}/config")))
                .config()
                .max_redirects(0)
                .build()
                .header("authorization", &auth)
                .header("x-fabric-client-version", "1")
                .header("content-type", "application/json")
                .send(desired(&flags)?.to_string())
        }
        ["node", action @ ("pause" | "resume" | "revoke"), name] => {
            node_name(name)?;
            client
                .post(&url(&format!("/{name}/{action}")))
                .config()
                .max_redirects(0)
                .build()
                .header("authorization", &auth)
                .header("x-fabric-client-version", "1")
                .send_empty()
        }
        ["query", body] => {
            serde_json::from_str::<serde_json::Value>(body)
                .map_err(|e| invalid(&format!("query is not JSON: {e}")))?;
            client
                .post(&format!("{}/v1/{namespace}/query", admin.url))
                .config()
                .max_redirects(0)
                .build()
                .header("authorization", &auth)
                .header("x-fabric-client-version", "1")
                .header("content-type", "application/json")
                .send(body.to_string())
        }
        _ => return Err(invalid(USAGE)),
    };
    let mut response = result.map_err(|e| io::Error::other(format!("transport: {e}")))?;
    let status = response.status().as_u16();
    let body = response
        .body_mut()
        .with_config()
        .limit(16 * 1024 * 1024)
        .read_to_string()
        .map_err(|e| io::Error::other(format!("response: {e}")))?;
    println!("{body}");
    if !(200..300).contains(&status) {
        eprintln!("fabricctl: server answered HTTP {status}");
        return Ok(false);
    }
    Ok(true)
}

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [mode, config_path] if mode == "inspect" => {
            let result = Config::load(config_path).and_then(|config| inspect(&config));
            match result {
                Ok(report) => {
                    println!(
                        "node_id={} generation={} batches={} metric_points={} log_records={} gaps={} otlp_payload_bytes={} committed_bytes={} file_bytes={} coverage_unknown={} recovery_required={} interrupted_append={} next_sequence={} acked_through={} log_backlog_bytes={}",
                        hex_id(&report.node_id),
                        report.generation,
                        report.batches,
                        report.metric_points,
                        report.log_records,
                        report.gaps,
                        report.otlp_payload_bytes,
                        report.committed_bytes,
                        report.file_bytes,
                        report.coverage_unknown,
                        report.recovery_required,
                        report.interrupted_append,
                        report.next_sequence,
                        report.acked_through,
                        report.log_backlog_bytes
                    );
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("fabricctl: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        [mode, config_path, words @ ..] if mode == "admin" || mode == "access" => {
            match remote(config_path, words, mode == "access") {
                Ok(true) => ExitCode::SUCCESS,
                Ok(false) => ExitCode::FAILURE,
                Err(error) => {
                    eprintln!("fabricctl: {error}");
                    ExitCode::from(2)
                }
            }
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt, symlink};

    #[test]
    fn scoped_config_never_accepts_master_key_or_ambiguous_origin() {
        let valid = "server_url=https://telemetry.example.com:7443\nserver_ca=/etc/fabrico11y/ca.pem\nworkload_token_file=/home/operator/workload-token\n";
        assert!(parse_client(valid, true).is_ok());
        assert!(
            parse_client(
                &valid.replace("workload_token_file", "admin_token_file"),
                true
            )
            .is_err()
        );
        assert!(parse_client(valid, false).is_err());
        assert!(parse_client(&format!("{valid}server_url=https://other.example\n"), true).is_err());
        assert!(
            parse_client(
                &valid.replace("/etc/fabrico11y/ca.pem", "relative.pem"),
                true
            )
            .is_err()
        );
        for origin in [
            "http://example.com",
            "https://user@example.com",
            "https://example.com/",
            "https://example.com/path",
            "https://example.com?x",
            "https://example.com#x",
            "https://example.com%2fother",
            "https://example.com\\other",
        ] {
            assert!(validate_origin(origin).is_err(), "{origin}");
        }
        assert!(validate_origin("https://[::1]:7443").is_ok());
    }

    #[test]
    fn node_argument_cannot_change_request_path() {
        assert!(node_name("source-1.prod").is_ok());
        for name in ["", ".", "..", "a/b", "a?x", "a#x", "a%2fb", "a\\b"] {
            assert!(node_name(name).is_err(), "{name}");
        }
    }

    #[test]
    fn workload_file_rejects_symlink_permissions_and_truncated_oversize() {
        let root = PathBuf::from(
            std::env::var_os("FABRIC_SCRATCH_ROOT").expect("resource launcher scratch"),
        );
        let dir = root.join(format!("fabricctl-token-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                std::fs::remove_dir_all(&self.0).unwrap();
            }
        }
        let _cleanup = Cleanup(dir.clone());
        let path = dir.join("token");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        use std::io::Write;
        file.write_all("ab".repeat(32).as_bytes()).unwrap();
        assert!(workload_token(&path).is_ok());
        let link = dir.join("link");
        symlink(&path, &link).unwrap();
        assert!(workload_token(&link).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(workload_token(&path).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        file.write_all(&vec![b' '; 5000]).unwrap();
        assert!(workload_token(&path).is_err());
    }
}
