//! Local companion enrollment must preserve durable administrative decisions.

use fabric_server::config::Config;
use fabric_server::control::{Control, DesiredConfig, SELF_SPINDLE_NAME, Status, sha256_hex};
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::var_os("FABRIC_SCRATCH_ROOT")
            .expect("run tests through the resource launcher");
        let path = PathBuf::from(root).join(format!(
            "self-identity-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove owned identity test scratch");
    }
}

fn desired() -> DesiredConfig {
    DesiredConfig {
        logs: vec!["/var/lib/fabric-server/operational/server.log".into()],
        metric_interval_s: 15,
    }
}

#[test]
fn self_enrollment_publishes_only_the_hash_and_survives_restart() {
    let scratch = Scratch::new();
    let token = "a1".repeat(32);
    let mut control = Control::open(&scratch.0).unwrap();
    control.ensure_self_spindle(&token, desired()).unwrap();
    assert_eq!(
        control.authenticate(&token).as_deref(),
        Some(SELF_SPINDLE_NAME)
    );
    let inventory = control.inventory();
    assert_eq!(inventory.len(), 1);
    let record = &inventory[0].0;
    assert_eq!(record.name, SELF_SPINDLE_NAME);
    assert_eq!(record.token_sha256, sha256_hex(token.as_bytes()));
    assert_eq!(record.status, Status::Active);
    assert_eq!(record.revision, 1);
    assert_eq!(record.desired, desired());
    let persisted = fs::read(scratch.0.join("control.json")).unwrap();
    assert!(!String::from_utf8_lossy(&persisted).contains(&token));
    drop(control);

    let mut reopened = Control::open(&scratch.0).unwrap();
    reopened.ensure_self_spindle(&token, desired()).unwrap();
    assert_eq!(
        reopened.authenticate(&token).as_deref(),
        Some(SELF_SPINDLE_NAME)
    );
    assert_eq!(fs::read(scratch.0.join("control.json")).unwrap(), persisted);
    assert_eq!(reopened.inventory()[0].0.revision, 1);
}

#[test]
fn matching_restart_preserves_admin_configuration_and_pause() {
    let scratch = Scratch::new();
    let token = "b2".repeat(32);
    let mut control = Control::open(&scratch.0).unwrap();
    control.ensure_self_spindle(&token, desired()).unwrap();
    let changed = DesiredConfig {
        logs: vec!["/var/log/selected.log".into()],
        metric_interval_s: 30,
    };
    control
        .set_config(SELF_SPINDLE_NAME, changed.clone())
        .unwrap();
    control
        .set_status(SELF_SPINDLE_NAME, Status::Paused)
        .unwrap();
    let persisted = fs::read(scratch.0.join("control.json")).unwrap();
    drop(control);

    let mut reopened = Control::open(&scratch.0).unwrap();
    reopened.ensure_self_spindle(&token, desired()).unwrap();
    let record = &reopened.inventory()[0].0;
    assert_eq!(record.status, Status::Paused);
    assert_eq!(record.desired, changed);
    assert_eq!(record.revision, 3);
    assert_eq!(fs::read(scratch.0.join("control.json")).unwrap(), persisted);
}

#[test]
fn wrong_or_revoked_credentials_leave_existing_identity_unchanged() {
    let scratch = Scratch::new();
    let token = "c3".repeat(32);
    let wrong = "d4".repeat(32);
    let mut control = Control::open(&scratch.0).unwrap();
    control.ensure_self_spindle(&token, desired()).unwrap();
    let persisted = fs::read(scratch.0.join("control.json")).unwrap();
    assert_eq!(
        control
            .ensure_self_spindle(&wrong, desired())
            .unwrap_err()
            .kind(),
        io::ErrorKind::PermissionDenied
    );
    assert_eq!(fs::read(scratch.0.join("control.json")).unwrap(), persisted);
    assert_eq!(
        control.authenticate(&token).as_deref(),
        Some(SELF_SPINDLE_NAME)
    );
    assert_eq!(control.authenticate(&wrong), None);
    assert_eq!(control.inventory()[0].0.revision, 1);

    control
        .set_status(SELF_SPINDLE_NAME, Status::Revoked)
        .unwrap();
    let revoked = fs::read(scratch.0.join("control.json")).unwrap();
    drop(control);
    let mut reopened = Control::open(&scratch.0).unwrap();
    assert_eq!(
        reopened
            .ensure_self_spindle(&token, desired())
            .unwrap_err()
            .kind(),
        io::ErrorKind::PermissionDenied
    );
    assert_eq!(reopened.authenticate(&token), None);
    assert_eq!(reopened.inventory()[0].0.status, Status::Revoked);
    assert_eq!(reopened.inventory()[0].0.revision, 2);
    assert_eq!(fs::read(scratch.0.join("control.json")).unwrap(), revoked);
}

#[test]
fn token_already_assigned_to_another_identity_is_refused() {
    let scratch = Scratch::new();
    let mut control = Control::open(&scratch.0).unwrap();
    let (_, token) = control.enroll("edge-existing", desired()).unwrap();
    let persisted = fs::read(scratch.0.join("control.json")).unwrap();
    assert!(control.ensure_self_spindle(&token, desired()).is_err());
    assert_eq!(
        control.authenticate(&token).as_deref(),
        Some("edge-existing")
    );
    assert_eq!(control.inventory().len(), 1);
    assert_eq!(fs::read(scratch.0.join("control.json")).unwrap(), persisted);
}

#[test]
fn revoked_other_identity_cannot_reuse_its_credential_under_the_self_name() {
    // Regression origin: static review found bootstrap checked only the live
    // authentication index, which excludes terminally revoked credentials.
    // The fix must check all durable identity hashes before enrollment.
    let scratch = Scratch::new();
    let mut control = Control::open(&scratch.0).unwrap();
    let (_, token) = control.enroll("edge-revoked", desired()).unwrap();
    control.set_status("edge-revoked", Status::Revoked).unwrap();
    let persisted = fs::read(scratch.0.join("control.json")).unwrap();
    drop(control);

    let mut reopened = Control::open(&scratch.0).unwrap();
    assert!(reopened.ensure_self_spindle(&token, desired()).is_err());
    assert_eq!(reopened.authenticate(&token), None);
    assert_eq!(reopened.inventory().len(), 1);
    assert_eq!(reopened.inventory()[0].0.name, "edge-revoked");
    assert_eq!(reopened.inventory()[0].0.status, Status::Revoked);
    assert_eq!(fs::read(scratch.0.join("control.json")).unwrap(), persisted);
}

#[test]
fn invalid_bootstrap_inputs_do_not_publish_an_identity() {
    let scratch = Scratch::new();
    let mut control = Control::open(&scratch.0).unwrap();
    for token in [String::new(), "a".repeat(63), "g".repeat(64)] {
        assert!(control.ensure_self_spindle(&token, desired()).is_err());
    }
    let invalid = DesiredConfig {
        metric_interval_s: 0,
        ..desired()
    };
    assert!(
        control
            .ensure_self_spindle(&"e5".repeat(32), invalid)
            .is_err()
    );
    assert!(control.inventory().is_empty());
    assert!(!scratch.0.join("control.json").exists());
}

const SERVER_CONFIG: &str = "listen=127.0.0.1:7443\ntls_cert=/cert.pem\ntls_key=/key.pem\nstate_dir=/state\nadmin_token_file=/admin\n";

#[test]
fn companion_config_keys_are_optional_and_absolute_paths_round_trip() {
    let scratch = Scratch::new();
    let path = scratch.0.join("server.conf");
    fs::write(&path, SERVER_CONFIG).unwrap();
    let (_, absent) = Config::load_with_spindle(&path).unwrap();
    assert!(absent.url.is_none());
    assert!(absent.ca.is_none());
    assert!(absent.executable.is_none());

    fs::write(&path, format!("{SERVER_CONFIG}self_spindle_url=https://localhost:7443\nself_spindle_ca=/etc/fabric/ca.pem\nself_spindle_executable=/usr/bin/fabric-node\n")).unwrap();
    let (server, settings) = Config::load_with_spindle(&path).unwrap();
    assert_eq!(server.state_dir, PathBuf::from("/state"));
    assert_eq!(settings.url.as_deref(), Some("https://localhost:7443"));
    assert_eq!(settings.ca, Some(PathBuf::from("/etc/fabric/ca.pem")));
    assert_eq!(
        settings.executable,
        Some(PathBuf::from("/usr/bin/fabric-node"))
    );
    assert_eq!(Config::load(&path).unwrap().listen, server.listen);
}

#[test]
fn companion_paths_and_duplicate_keys_are_validated() {
    let scratch = Scratch::new();
    let path = scratch.0.join("server.conf");
    for key in ["self_spindle_ca", "self_spindle_executable"] {
        for value in [
            "relative/path".to_owned(),
            String::new(),
            format!("/{}", "x".repeat(240)),
        ] {
            fs::write(&path, format!("{SERVER_CONFIG}{key}={value}\n")).unwrap();
            assert!(
                Config::load_with_spindle(&path).is_err(),
                "accepted {key}={value}"
            );
        }
        fs::write(
            &path,
            format!("{SERVER_CONFIG}{key}=/first\n{key}=/second\n"),
        )
        .unwrap();
        assert!(Config::load_with_spindle(&path).is_err());
        fs::write(&path, format!("{SERVER_CONFIG}{key}=/only\n")).unwrap();
        assert!(Config::load_with_spindle(&path).is_ok());
    }
    fs::write(&path, format!("{SERVER_CONFIG}self_spindle_url=https://localhost:7443\nself_spindle_url=https://localhost:7444\n")).unwrap();
    assert!(Config::load_with_spindle(&path).is_err());
}
