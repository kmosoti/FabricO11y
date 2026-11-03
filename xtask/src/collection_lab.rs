//! Fixtures for executable collection experiments; never production policy.
use fabric::{
    observation::{self, Data, Identity, Level, Observation},
    spindle,
};
use std::{
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

pub fn config(root: &Path) -> spindle::Config {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700)).unwrap();
    spindle::Config {
        socket: root.join("s"),
        storage: root.join("store"),
        quota_bytes: 8 * 1024 * 1024,
        max_connections: 8,
        client_timeout_ms: 200,
        files: vec![],
        journal: None,
        host: None,
    }
}
pub fn observation(sequence: u64) -> Observation {
    Observation {
        version: 2,
        registry: observation::REGISTRY.into(),
        identity: Identity {
            producer: "fault-program".into(),
            generation: "fixed".into(),
            sequence,
        },
        timestamp_ns: sequence,
        resource: "fixture".into(),
        data: Data::Log {
            level: Level::Info,
            message: format!(
                "{sequence}:{}",
                "x".repeat((sequence as usize * 503) % 12000)
            ),
        },
    }
}
/// Owns only the child it created, including on assertion unwinding.
pub struct OwnedChild(pub Child);
impl OwnedChild {
    pub fn kill_wait(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        self.kill_wait();
    }
}

pub fn daemon(binary: &Path, config: &spindle::Config, root: &Path) -> OwnedChild {
    let file = root.join("config.json");
    std::fs::write(&file, serde_json::to_vec(config).unwrap()).unwrap();
    let mut child = OwnedChild(
        Command::new(binary)
            .arg("serve")
            .arg(file)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "daemon exited before readiness"
        );
        if matches!(
            spindle::transport::request(
                &config.socket,
                &spindle::Request::Status,
                Instant::now() + Duration::from_millis(200)
            ),
            Ok(spindle::Reply::Status { .. })
        ) {
            return child;
        }
        assert!(Instant::now() < end, "daemon readiness deadline");
        std::thread::sleep(Duration::from_millis(5));
    }
}
pub fn rss_bytes() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    status
        .lines()
        .find_map(|s| s.strip_prefix("VmRSS:"))
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse::<u64>()
        .unwrap()
        * 1024
}
