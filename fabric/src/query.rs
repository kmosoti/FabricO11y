//! Versioned investigation contracts and the native credential-bound client.
pub use fabric_query_contract::*;

/// Credential-bound client used unchanged by CLI and MCP. Credentials never enter
/// requests, cursors, query output or Debug formatting.
#[derive(Clone)]
pub struct Client {
    endpoint: String,
    credential: String,
}
/// Opens `path` once with `O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC`, validates ownership,
/// regular-file type, optional `0o077` privacy mask, and byte ceiling on `fstat`,
/// and reads directly from that opened descriptor (zero pathname TOCTOU).
pub fn read_credential_fd(
    path: &std::path::Path,
    require_private_mode: bool,
    max_bytes: u64,
) -> Result<String, String> {
    use rustix::fs::{Mode, OFlags};
    use std::{fs::File, io::Read, os::unix::fs::MetadataExt};
    let fd = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| {
        if error == rustix::io::Errno::LOOP {
            "credential file is symlinked".to_owned()
        } else {
            format!("credential file error: {error}")
        }
    })?;
    let file = File::from(fd);
    let metadata = file
        .metadata()
        .map_err(|error| format!("credential file error: {error}"))?;
    if !metadata.is_file()
        || metadata.uid() != rustix::process::getuid().as_raw()
        || (require_private_mode && metadata.mode() & 0o077 != 0)
        || metadata.len() == 0
        || metadata.len() > max_bytes
    {
        return Err(format!(
            "credential file must be owned, {}regular, nonempty and <={max_bytes} bytes",
            if require_private_mode {
                "private, "
            } else {
                ""
            }
        ));
    }
    let mut value = String::new();
    file.take(max_bytes.saturating_add(1))
        .read_to_string(&mut value)
        .map_err(|error| format!("credential file read error: {error}"))?;
    if value.len() as u64 > max_bytes {
        return Err(format!("credential file exceeds {max_bytes} bytes"));
    }
    let trimmed = value.trim().to_owned();
    if trimmed.is_empty() {
        return Err("credential file is empty".into());
    }
    Ok(trimmed)
}

/// Reads the private credential file used by canonical readers.
pub fn read_credential_file(path: &std::path::Path) -> Result<String, String> {
    validate_credential(read_credential_fd(path, true, 256)?)
}

fn validate_credential(value: String) -> Result<String, String> {
    if value.len() < 16 || value.len() > 256 || !value.bytes().all(|b| b.is_ascii_graphic()) {
        return Err("invalid credential format".into());
    }
    Ok(value)
}

/// Connection settings shared by the CLI and MCP host. A private file avoids
/// putting the credential itself in host configuration or command arguments.
pub fn connection_from_env() -> Result<(String, String), String> {
    let endpoint = std::env::var("FABRIC_ENDPOINT").map_err(|_| "FABRIC_ENDPOINT required")?;
    let credential = match (
        std::env::var_os("FABRIC_CREDENTIAL_FILE"),
        std::env::var_os("FABRIC_CREDENTIAL"),
    ) {
        (Some(path), None) => read_credential_file(std::path::Path::new(&path))?,
        (None, Some(value)) => validate_credential(
            value
                .into_string()
                .map_err(|_| "invalid credential encoding")?,
        )?,
        (Some(_), Some(_)) => {
            return Err("choose FABRIC_CREDENTIAL_FILE or FABRIC_CREDENTIAL".into());
        }
        (None, None) => return Err("FABRIC_CREDENTIAL_FILE required".into()),
    };
    Ok((endpoint, credential))
}
impl Client {
    pub fn new(endpoint: impl Into<String>, credential: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            credential: credential.into(),
        }
    }
    pub fn exchange(
        &self,
        request: &crate::canonical::Request,
        deadline: std::time::Instant,
    ) -> Result<crate::canonical::Response, String> {
        crate::canonical::exchange(&self.endpoint, &self.credential, request, deadline)
    }
    pub fn query(
        &self,
        request: QueryRequest,
        deadline: std::time::Instant,
    ) -> Result<QueryResponse, String> {
        request.validate()?;
        match self.exchange(&crate::canonical::Request::Query { request }, deadline)? {
            crate::canonical::Response::Query { response } => Ok(response),
            crate::canonical::Response::Error { error } => Err(error),
            _ => Err("unexpected query response".into()),
        }
    }
    pub fn plan(
        &self,
        request: QueryRequest,
        deadline: std::time::Instant,
    ) -> Result<QueryPlan, String> {
        request.validate()?;
        match self.exchange(&crate::canonical::Request::Plan { request }, deadline)? {
            crate::canonical::Response::Plan { plan } => Ok(plan),
            crate::canonical::Response::Error { error } => Err(error),
            _ => Err("unexpected plan response".into()),
        }
    }
}
