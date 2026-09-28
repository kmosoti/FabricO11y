use fabric_o11y::spindle::runtime::{Config, inspect};
use fabric_o11y::spindle::sender::{agent, read_token};
use std::io::{self, Read};
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage:
  fabricctl inspect <NODE_CONFIG>
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

/// Admin client settings: `server_url`, `server_ca`, `admin_token_file`.
struct Admin {
    url: String,
    ca: PathBuf,
    token_file: PathBuf,
}

fn load_admin(path: &str) -> io::Result<Admin> {
    let mut text = String::new();
    std::fs::File::open(path)?
        .take(64 * 1024)
        .read_to_string(&mut text)?;
    let (mut url, mut ca, mut token_file) = (None, None, None);
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| invalid("invalid admin config line"))?;
        let value = value.trim().to_owned();
        match key.trim() {
            "server_url" if url.is_none() => url = Some(value),
            "server_ca" if ca.is_none() => ca = Some(PathBuf::from(value)),
            "admin_token_file" if token_file.is_none() => token_file = Some(PathBuf::from(value)),
            _ => return Err(invalid("unknown or duplicate admin config key")),
        }
    }
    match (url, ca, token_file) {
        (Some(url), Some(ca), Some(token_file)) if url.starts_with("https://") => Ok(Admin {
            url,
            ca,
            token_file,
        }),
        _ => Err(invalid(
            "admin config needs server_url=https://..., server_ca and admin_token_file",
        )),
    }
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

fn admin(config_path: &str, words: &[String]) -> io::Result<bool> {
    let admin = load_admin(config_path)?;
    let token = read_token(&admin.token_file)?;
    let client = agent(&admin.ca)?;
    let auth = format!("Bearer {token}");
    let url = |tail: &str| format!("{}/v1/admin/nodes{tail}", admin.url);
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    let result = match words.as_slice() {
        ["node", "list"] => client.get(&url("")).header("authorization", &auth).call(),
        ["node", "add", name, flags @ ..] => {
            let flags: Vec<String> = flags.iter().map(|s| s.to_string()).collect();
            let mut body = desired(&flags)?;
            body["name"] = serde_json::Value::String(name.to_string());
            client
                .post(&url(""))
                .header("authorization", &auth)
                .header("content-type", "application/json")
                .send(body.to_string())
        }
        ["node", "config", name, flags @ ..] => {
            let flags: Vec<String> = flags.iter().map(|s| s.to_string()).collect();
            client
                .put(&url(&format!("/{name}/config")))
                .header("authorization", &auth)
                .header("content-type", "application/json")
                .send(desired(&flags)?.to_string())
        }
        ["node", action @ ("pause" | "resume" | "revoke"), name] => client
            .post(&url(&format!("/{name}/{action}")))
            .header("authorization", &auth)
            .send_empty(),
        ["query", body] => {
            serde_json::from_str::<serde_json::Value>(body)
                .map_err(|e| invalid(&format!("query is not JSON: {e}")))?;
            client
                .post(&format!("{}/v1/admin/query", admin.url))
                .header("authorization", &auth)
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
        .unwrap_or_default();
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
        [mode, config_path, words @ ..] if mode == "admin" => match admin(config_path, words) {
            Ok(true) => ExitCode::SUCCESS,
            Ok(false) => ExitCode::FAILURE,
            Err(error) => {
                eprintln!("fabricctl: {error}");
                ExitCode::from(2)
            }
        },
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}
