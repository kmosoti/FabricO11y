use fabric_server::{Config, NodeGrant, Server};
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

fn scratch_dir() -> tempfile::TempDir {
    let base = std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute() && !p.starts_with("/tmp") && !p.starts_with("/dev/shm"))
        .unwrap_or_else(|| PathBuf::from("/data/tmp"));
    fs::create_dir_all(&base).unwrap();
    tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(base)
        .unwrap()
}

fn write_secret(dir: &Path, name: &str, value: &str, mode: u32) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, value).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
    path
}

#[test]
fn server_open_rejects_unauthenticated_engine_and_forwards_bearer_token() {
    let dir = scratch_dir();
    let reader = write_secret(
        dir.path(),
        "reader.key",
        "reader-secret-credential-0123",
        0o600,
    );
    let node = write_secret(dir.path(), "node.key", "node-secret-credential-4567", 0o600);

    // 1) Engine endpoint without token file and without explicit opt-in fails closed.
    let err = Server::open(Config {
        listen: "127.0.0.1:0".into(),
        storage: dir.path().join("canonical-no-auth"),
        projection: dir.path().join("projection-no-auth"),
        quota_bytes: 4 << 20,
        projection_quota_bytes: 4 << 20,
        reader_credential_file: reader.clone(),
        nodes: vec![NodeGrant {
            origin: "node-a".into(),
            credential_file: node.clone(),
        }],
        node_quota_bytes: Default::default(),
        engine_endpoint: Some("http://127.0.0.1:19116".into()),
        engine_auth_token_file: None,
        allow_unauthenticated_loopback_engine: false,
        max_connections: 4,
    })
    .err()
    .expect("unauthenticated engine_endpoint must fail closed by default");
    assert!(
        err.contains("engine_auth_token_file"),
        "unexpected err: {err}"
    );

    // 2) Engine endpoint with token file attaches Bearer header on /v1/sql requests.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let seen_headers = Arc::new(Mutex::new(Vec::<String>::new()));
    let headers_ref = Arc::clone(&seen_headers);
    let stop_http = Arc::new(AtomicBool::new(false));
    let stop_http_bg = Arc::clone(&stop_http);
    listener.set_nonblocking(true).unwrap();

    let http_server = thread::spawn(move || {
        let mut bound_authority: Option<String> = None;
        while !stop_http_bg.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let _ = stream.set_nonblocking(false);
                    let mut buf = [0u8; 8192];
                    let n = stream.read(&mut buf).unwrap_or(0);
                    let text = String::from_utf8_lossy(&buf[..n]).to_string();
                    if let Some(auth_line) = text
                        .lines()
                        .find(|l| l.to_ascii_lowercase().starts_with("authorization:"))
                    {
                        headers_ref
                            .lock()
                            .unwrap()
                            .push(auth_line.trim().to_owned());
                    }
                    let decoded = {
                        let body_raw = text.split("\r\n\r\n").nth(1).unwrap_or("");
                        let bytes = body_raw.as_bytes();
                        let mut out = String::new();
                        let mut i = 0;
                        while i < bytes.len() {
                            if bytes[i] == b'%'
                                && i + 2 < bytes.len()
                                && let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3])
                                && let Ok(val) = u8::from_str_radix(hex, 16)
                            {
                                out.push(val as char);
                                i += 3;
                                continue;
                            }
                            out.push(if bytes[i] == b'+' {
                                ' '
                            } else {
                                bytes[i] as char
                            });
                            i += 1;
                        }
                        out
                    };
                    let req_upper = decoded.to_ascii_uppercase();
                    let body = if req_upper.contains("INSERT") && decoded.contains("'authority'") {
                        if let Some(rest) = decoded.split("'authority',CAST(X'").nth(1)
                            && let Some(hex) = rest.split('\'').next()
                        {
                            let raw: Vec<u8> = hex
                                .as_bytes()
                                .chunks(2)
                                .filter_map(|c| {
                                    std::str::from_utf8(c)
                                        .ok()
                                        .and_then(|s| u8::from_str_radix(s, 16).ok())
                                })
                                .collect();
                            if let Ok(uuid) = String::from_utf8(raw) {
                                bound_authority = Some(uuid);
                            }
                        }
                        r#"{"code":0,"output":[{"affectedrows":1}]}"#.to_owned()
                    } else if req_upper.contains("CREATE") {
                        r#"{"code":0,"output":[{"affectedrows":0}]}"#.to_owned()
                    } else if decoded.contains("authority_name") {
                        match &bound_authority {
                            Some(auth) => serde_json::json!({
                                "code": 0,
                                "output": [{
                                    "records": {
                                        "schema": { "column_schemas": [{ "name": "authority_name" }] },
                                        "rows": [[auth]]
                                    }
                                }]
                            })
                            .to_string(),
                            None => r#"{"code":0,"output":[{"records":{"schema":{"column_schemas":[{"name":"authority_name"}]},"rows":[]}}]}"#.to_owned(),
                        }
                    } else if decoded.contains(" AS highest") {
                        r#"{"code":0,"output":[{"records":{"schema":{"column_schemas":[{"name":"highest"}]},"rows":[[null]]}}]}"#.to_owned()
                    } else if decoded.contains(" AS n") || req_upper.contains("COUNT(") {
                        r#"{"code":0,"output":[{"records":{"schema":{"column_schemas":[{"name":"n"}]},"rows":[["0"]]}}]}"#.to_owned()
                    } else {
                        r#"{"code":0,"output":[{"records":{"schema":{"column_schemas":[]},"rows":[]}}]}"#.to_owned()
                    };
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(resp.as_bytes());
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(_) => break,
            }
        }
    });

    let engine_token = "greptime-server-bearer-secret-9999";
    let engine_token_file = write_secret(dir.path(), "engine.token", engine_token, 0o600);
    let server = Server::open(Config {
        listen: "127.0.0.1:0".into(),
        storage: dir.path().join("canonical-auth"),
        projection: dir.path().join("projection-auth"),
        quota_bytes: 4 << 20,
        projection_quota_bytes: 4 << 20,
        reader_credential_file: reader,
        nodes: vec![NodeGrant {
            origin: "node-a".into(),
            credential_file: node,
        }],
        node_quota_bytes: Default::default(),
        engine_endpoint: Some(format!("http://{addr}")),
        engine_auth_token_file: Some(engine_token_file),
        allow_unauthenticated_loopback_engine: false,
        max_connections: 4,
    })
    .unwrap();
    drop(server);
    stop_http.store(true, Ordering::Release);
    http_server.join().unwrap();

    let headers = seen_headers.lock().unwrap();
    assert!(
        !headers.is_empty(),
        "server must issue authenticated /v1/sql requests"
    );
    assert!(
        headers
            .iter()
            .all(|h| h.to_ascii_lowercase() == format!("authorization: bearer {engine_token}")),
        "unexpected auth headers: {headers:?}"
    );
}

#[test]
fn credential_loader_rejects_symlinks_and_permissive_modes() {
    let dir = scratch_dir();
    let real = write_secret(
        dir.path(),
        "real.key",
        "private-credential-0123456789",
        0o600,
    );
    let sym = dir.path().join("sym.key");
    symlink(&real, &sym).unwrap();
    let node = write_secret(
        dir.path(),
        "node.key",
        "node-credential-0123456789ab",
        0o600,
    );

    let sym_err = Server::open(Config {
        listen: "127.0.0.1:0".into(),
        storage: dir.path().join("canonical-sym"),
        projection: dir.path().join("projection-sym"),
        quota_bytes: 4 << 20,
        projection_quota_bytes: 4 << 20,
        reader_credential_file: sym,
        nodes: vec![NodeGrant {
            origin: "node-a".into(),
            credential_file: node.clone(),
        }],
        node_quota_bytes: Default::default(),
        engine_endpoint: None,
        engine_auth_token_file: None,
        allow_unauthenticated_loopback_engine: false,
        max_connections: 4,
    })
    .err()
    .expect("symlinked credential must fail via O_NOFOLLOW");
    assert!(sym_err.contains("failed opening credential"), "{sym_err}");

    let world = write_secret(
        dir.path(),
        "world.key",
        "private-credential-0123456789",
        0o644,
    );
    let mode_err = Server::open(Config {
        listen: "127.0.0.1:0".into(),
        storage: dir.path().join("canonical-mode"),
        projection: dir.path().join("projection-mode"),
        quota_bytes: 4 << 20,
        projection_quota_bytes: 4 << 20,
        reader_credential_file: world,
        nodes: vec![NodeGrant {
            origin: "node-a".into(),
            credential_file: node,
        }],
        node_quota_bytes: Default::default(),
        engine_endpoint: None,
        engine_auth_token_file: None,
        allow_unauthenticated_loopback_engine: false,
        max_connections: 4,
    })
    .err()
    .expect("group/world readable credential must fail fstat check");
    assert!(mode_err.contains("private owned"), "{mode_err}");
}
