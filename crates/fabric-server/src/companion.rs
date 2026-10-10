//! Production process composition. No wire or durable Batch path lives here.
use fabric_server::{
    config::{Config, SpindleSettings},
    control::{Control, DesiredConfig, SELF_SPINDLE_NAME},
};
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::os::unix::{
    fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    process::CommandExt,
};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

fn private_file(path: &Path, create: bool) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(create)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let m = file.metadata()?;
    // SAFETY: geteuid has no pointer preconditions.
    if !m.is_file()
        || m.nlink() != 1
        || m.mode() & 0o077 != 0
        || m.uid() != unsafe { libc::geteuid() }
    {
        return Err(invalid(
            "local Spindle state must be private regular files owned by the service user",
        ));
    }
    Ok(file)
}

fn atomic_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let staged = path.with_extension("new");
    let mut file = private_file(&staged, true)?;
    file.set_len(0)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(staged, path)?;
    File::open(path.parent().unwrap())?.sync_all()
}

/// Holding this owner prevents two production supervisors from provisioning the
/// same state. File::try_lock is released by RAII and the descriptor is CLOEXEC.
pub struct LocalSpindle {
    _lease: File,
    directory: PathBuf,
    executable: PathBuf,
    settings: SpindleSettings,
    certificate: PathBuf,
    server_log: PathBuf,
}

impl LocalSpindle {
    pub fn prepare(
        config: &Config,
        settings: SpindleSettings,
        server_log: PathBuf,
    ) -> io::Result<Self> {
        fs::create_dir_all(&config.state_dir)?;
        let lease = private_file(&config.state_dir.join("server-process.lock"), true)?;
        lease.try_lock().map_err(|e| {
            io::Error::other(format!(
                "server state already owned or cannot be locked: {e}"
            ))
        })?;
        let directory = config.state_dir.join("self-spindle");
        match DirBuilder::new().mode(0o700).create(&directory) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
        let m = fs::symlink_metadata(&directory)?;
        // SAFETY: geteuid has no pointer preconditions.
        if !m.is_dir() || m.mode() & 0o077 != 0 || m.uid() != unsafe { libc::geteuid() } {
            return Err(invalid(
                "local Spindle directory must be private and owned by this user",
            ));
        }
        let executable = match &settings.executable {
            Some(p) => p.clone(),
            None => std::env::current_exe()?
                .parent()
                .ok_or_else(|| invalid("server executable has no parent"))?
                .join("fabric-node"),
        };
        let m = fs::metadata(&executable).map_err(|e| io::Error::new(e.kind(),
            "dedicated fabric-node executable unavailable; build/install both workspace binaries or set self_spindle_executable"))?;
        if !m.is_file() || m.mode() & 0o111 == 0 {
            return Err(invalid("dedicated Spindle executable is not executable"));
        }
        let mut control = Control::open(&config.state_dir)?;
        let token_path = directory.join("token");
        let token = match private_file(&token_path, false) {
            Ok(file) => {
                let mut value = String::new();
                file.take(257).read_to_string(&mut value)?;
                value
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if control
                    .inventory()
                    .iter()
                    .any(|(r, _)| r.name == SELF_SPINDLE_NAME)
                {
                    return Err(invalid(
                        "enrolled local Spindle credential is missing; restore it instead of replacing its identity",
                    ));
                }
                let mut bytes = [0u8; 32];
                File::open("/dev/urandom")?.read_exact(&mut bytes)?;
                let value: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
                // Publish the secret before enrolling its hash. Startup after
                // either boundary repeats the same identity, never a new token.
                atomic_private(&token_path, value.as_bytes())?;
                value
            }
            Err(e) => return Err(e),
        };
        control.ensure_self_spindle(
            &token,
            DesiredConfig {
                logs: vec![server_log.to_string_lossy().into_owned()],
                metric_interval_s: 15,
            },
        )?;
        Ok(Self {
            _lease: lease,
            directory,
            executable,
            settings,
            certificate: config.tls_cert.clone(),
            server_log,
        })
    }

    pub fn start(&self, listener: SocketAddr) -> io::Result<Companion> {
        let url = destination(listener, self.settings.url.as_deref())?;
        let ca = self.settings.ca.as_ref().unwrap_or(&self.certificate);
        let text = format!(
            "spool_dir={}\nmetric_interval_s=15\nspool_bytes=67108864\nmax_output_bytes_per_s=65536\nserver_url={url}\nserver_ca={}\ntoken_file={}\n",
            self.directory.join("spool").display(),
            ca.display(),
            self.directory.join("token").display()
        );
        let path = self.directory.join("node.conf");
        atomic_private(&path, text.as_bytes())?;
        let mut command = Command::new(&self.executable);
        command
            .arg("run")
            .arg(&path)
            .arg("--server-log")
            .arg(&self.server_log)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        let parent = std::process::id() as libc::pid_t;
        // SAFETY: pre_exec uses only async-signal-safe syscalls; no allocation
        // or lock on the successful path. Close the fork/parent-death race.
        unsafe {
            command.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                    return Err(io::Error::last_os_error());
                }
                if libc::getppid() != parent {
                    return Err(io::Error::from_raw_os_error(libc::ESRCH));
                }
                Ok(())
            });
        }
        Ok(Companion(command.spawn()?))
    }
}

fn destination(listener: SocketAddr, configured: Option<&str>) -> io::Result<String> {
    let local = SocketAddr::new(
        match listener.ip() {
            IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
            ip => ip,
        },
        listener.port(),
    );
    let Some(url) = configured else {
        return Ok(format!("https://{local}"));
    };
    let host = url
        .strip_prefix("https://")
        .ok_or_else(|| invalid("self_spindle_url requires verified HTTPS"))?;
    let target = if let Some(port) = host.strip_prefix("localhost:") {
        SocketAddr::new(
            local.ip(),
            port.parse()
                .map_err(|_| invalid("invalid local Spindle port"))?,
        )
    } else {
        host.parse::<SocketAddr>()
            .map_err(|_| invalid("self_spindle_url needs a local IP literal or localhost"))?
    };
    if target != local || (host.starts_with("localhost:") && !local.ip().is_loopback()) {
        return Err(invalid(
            "self_spindle_url must address this server's local listener",
        ));
    }
    Ok(url.into())
}

/// Drop is a last-resort reap; the async supervisor first offers SIGTERM and
/// keeps HTTP available while the child finishes its bounded current cycle.
pub struct Companion(pub Child);
impl Companion {
    pub fn terminate(&mut self) -> io::Result<()> {
        // The child is unreaped, so its PID cannot have been reused.
        if self.0.try_wait()?.is_none() {
            // SAFETY: kill uses the owned child's PID and a fixed signal.
            if unsafe { libc::kill(self.0.id() as libc::pid_t, libc::SIGTERM) } != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }
}
impl Drop for Companion {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_companion_cannot_silently_forward_to_another_server() {
        let listener = "0.0.0.0:7443".parse().unwrap();
        assert_eq!(
            destination(listener, None).unwrap(),
            "https://127.0.0.1:7443"
        );
        assert!(destination(listener, Some("https://localhost:7443")).is_ok());
        for url in [
            "http://127.0.0.1:7443",
            "https://127.0.0.1:7444",
            "https://10.1.1.1:7443",
            "https://example.com:7443",
        ] {
            assert!(destination(listener, Some(url)).is_err(), "{url}");
        }
    }
}
