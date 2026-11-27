//! Hardened local Unix domain socket binding and same-UID peer authentication.

use rustix::fs::{Mode, OFlags, fchmod, open};
use std::fs::{self, DirBuilder};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::Path;
use tokio::net::{UnixListener, UnixStream};

/// Creates (if absent) and hardens a dedicated socket parent directory to `0o700`,
/// never modifying shared system roots (`/`, `/tmp`, `/var/tmp`, `/data`, `/data/tmp`).
pub(crate) fn ensure_private_socket_parent(parent: &Path) -> Result<(), String> {
    if !parent.exists() {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)
            .map_err(|e| format!("failed creating socket dir {}: {e}", parent.display()))?;
    }
    if matches!(
        parent.to_str(),
        Some("/" | "/tmp" | "/var/tmp" | "/data" | "/data/tmp" | "/run")
    ) || parent == std::env::temp_dir().as_path()
    {
        return Ok(());
    }
    let meta = fs::symlink_metadata(parent)
        .map_err(|e| format!("failed inspecting socket dir {}: {e}", parent.display()))?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(format!(
            "invalid symlink or non-directory socket parent {}",
            parent.display()
        ));
    }
    if meta.uid() == rustix::process::getuid().as_raw()
        && (meta.permissions().mode() & 0o777) != 0o700
    {
        let fd = open(
            parent,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|e| format!("failed opening socket dir {}: {e}", parent.display()))?;
        fchmod(&fd, Mode::from_raw_mode(0o700))
            .map_err(|e| format!("failed chmod 0700 on socket dir {}: {e}", parent.display()))?;
    }
    Ok(())
}

/// Binds a Unix listener at `sock_path` under an owned `0o700` parent directory and
/// immediately sets `0o600` on the socket inode regardless of ambient umask.
pub(crate) fn bind_private_unix_listener(sock_path: &Path) -> Result<UnixListener, String> {
    if let Some(parent) = sock_path.parent() {
        ensure_private_socket_parent(parent)?;
    }
    if let Ok(meta) = fs::symlink_metadata(sock_path) {
        if meta.file_type().is_symlink() || meta.is_dir() {
            return Err(format!(
                "refusing unsafe socket path {}",
                sock_path.display()
            ));
        }
        let _ = fs::remove_file(sock_path);
    }
    let listener = UnixListener::bind(sock_path)
        .map_err(|e| format!("failed binding {}: {e}", sock_path.display()))?;
    fs::set_permissions(sock_path, fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("failed chmod 0600 on {}: {e}", sock_path.display()))?;
    Ok(listener)
}

/// Verifies `SO_PEERCRED` UID matches the current effective/real UID.
pub(crate) fn is_same_uid_peer(stream: &UnixStream) -> bool {
    let expected = rustix::process::geteuid().as_raw();
    stream.peer_cred().is_ok_and(|cred| cred.uid() == expected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn private_unix_socket_enforces_0700_0600_under_permissive_umask() {
        if std::env::var_os("FABRIC_COLLECT_UMASK_SUBPROCESS").is_none() {
            let exe = std::env::current_exe().expect("test binary path");
            let status = std::process::Command::new("sh")
                .args([
                    "-c",
                    "umask 000 && exec \"$1\" --exact \"socket_security::tests::private_unix_socket_enforces_0700_0600_under_permissive_umask\"",
                    "sh",
                    &exe.to_string_lossy(),
                ])
                .env("FABRIC_COLLECT_UMASK_SUBPROCESS", "1")
                .status()
                .expect("spawn umask 000 child");
            assert!(status.success(), "umask 000 child test failed");
            return;
        }

        let base = std::env::var_os("TMPDIR")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute() && !p.starts_with("/tmp") && !p.starts_with("/dev/shm"))
            .unwrap_or_else(|| PathBuf::from("/data/tmp"));
        fs::create_dir_all(&base).unwrap();
        let dir = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(base)
            .unwrap();
        let sock_dir = dir.path().join("run");
        let sock = sock_dir.join("daemon.sock");
        let listener = bind_private_unix_listener(&sock).unwrap();

        let dir_mode = fs::symlink_metadata(&sock_dir)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        let sock_mode = fs::symlink_metadata(&sock).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700);
        assert_eq!(sock_mode, 0o600);

        let connect_fut = UnixStream::connect(&sock);
        let accept_fut = listener.accept();
        let (client, server) = tokio::join!(connect_fut, accept_fut);
        let _client = client.unwrap();
        let (srv_stream, _) = server.unwrap();
        assert!(is_same_uid_peer(&srv_stream));
    }
}
