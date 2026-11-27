use fabric_core::{CliEnvelope, DoctorReport};
use std::{
    fs,
    os::unix::{fs::DirBuilderExt, net::UnixListener},
    path::PathBuf,
    process::Command,
};

struct Scratch(PathBuf);

impl Scratch {
    fn new(prefer_tmpdir: bool) -> Self {
        let name = format!("d-{}", std::process::id());
        let outer = prefer_tmpdir
            .then(|| std::env::var_os("TMPDIR"))
            .flatten()
            .and_then(|path| fabric_store::validate_non_tmp_path_on_disk(&PathBuf::from(path)).ok())
            .filter(|path| {
                path.is_dir()
                    && path
                        .join(&name)
                        .join("daemon.sock")
                        .as_os_str()
                        .as_encoded_bytes()
                        .len()
                        < 108
            });
        let base = outer.unwrap_or_else(|| {
            let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
            fabric_store::validate_non_tmp_path_on_disk(base.parent().unwrap()).unwrap();
            fs::create_dir_all(&base).unwrap();
            fabric_store::validate_non_tmp_path_on_disk(&base).unwrap()
        });
        let path = base.join(name);
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn offline_doctor_emits_typed_versioned_diagnostic() {
    const CHILD: &str = "FABRIC_DOCTOR_SCHEMA_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let sentinel = Scratch::new(true);
        let listener = UnixListener::bind(sentinel.0.join("daemon.sock")).unwrap();
        listener.set_nonblocking(true).unwrap();
        let config = sentinel.0.join("invalid.toml");
        fs::write(&config, "invalid = [").unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "offline_doctor_emits_typed_versioned_diagnostic"])
            .env(CHILD, "1")
            .env_remove("TMPDIR")
            .env("FABRIC_RUNTIME_DIR", &sentinel.0)
            .env("FABRIC_CONFIG_FILE", config)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        return;
    }

    assert!(std::env::var_os("TMPDIR").is_none());
    assert!(std::env::var_os("FABRIC_RUNTIME_DIR").is_some());
    assert!(std::env::var_os("FABRIC_CONFIG_FILE").is_some());
    let base = Scratch::new(false);
    let output = Command::new(env!("CARGO_BIN_EXE_fabric"))
        .args(["--json", "doctor"])
        .env("HOME", &base.0)
        .env("FABRIC_DATA_DIR", base.0.join("data"))
        .env("FABRIC_TMP_DIR", &base.0)
        .env("FABRIC_RUNTIME_DIR", &base.0)
        .env("XDG_CONFIG_HOME", base.0.join("config"))
        .env_remove("FABRIC_CONFIG_FILE")
        .env_remove("FABRIC_SERVER_ENDPOINT")
        .env_remove("FABRIC_READER_CREDENTIAL_FILE")
        .env_remove("SPINDLE_SOCKET")
        .env_remove("XDG_RUNTIME_DIR")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope: CliEnvelope<DoctorReport> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope.schema_version, "fabric.cli.v1");
    assert_eq!(envelope.command, "doctor");
    assert!(!envelope.data.daemon_running);
    assert!(!envelope.data.checks.is_empty());
}
