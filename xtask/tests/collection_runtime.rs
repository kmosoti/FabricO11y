use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
};
use xtask::collection::{Layout, install};
fn fixture() -> (tempfile::TempDir, Layout, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let layout = Layout {
        home: temp.path().join("home"),
        runtime: temp.path().join("run"),
        config: temp.path().join("config"),
        state: temp.path().join("state"),
    };
    let bins = temp.path().join("bin");
    fs::create_dir(&bins).unwrap();
    for name in ["spindle", "fabric-adapter"] {
        let path = bins.join(name);
        fs::write(&path, b"fixture executable").unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    (temp, layout, bins)
}
#[test]
fn installation_is_private_repeatable_and_preserves_conflicts() {
    let (_temp, layout, bins) = fixture();
    install(&layout, &bins).unwrap();
    install(&layout, &bins).unwrap();
    let path = layout.config.join("fabrico11y/spindle.json");
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let config: fabric::spindle::Config =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(config.socket, layout.socket());
    assert!(config.files.is_empty() && config.host.is_none() && config.journal.is_none());
    fs::create_dir(&config.storage).unwrap();
    fs::write(config.storage.join("user-data"), b"preserve").unwrap();
    fs::write(&path, b"custom configuration").unwrap();
    assert!(
        install(&layout, &bins)
            .unwrap_err()
            .contains("installation conflict")
    );
    assert_eq!(fs::read(&path).unwrap(), b"custom configuration");
    assert_eq!(
        fs::read(config.storage.join("user-data")).unwrap(),
        b"preserve"
    );
}
#[test]
fn installation_refuses_symlink_and_open_runtime() {
    let (temp, layout, bins) = fixture();
    fs::create_dir(&layout.home).unwrap();
    symlink(temp.path(), &layout.runtime).unwrap();
    assert!(
        install(&layout, &bins)
            .unwrap_err()
            .contains("symbolic link")
    );
    fs::remove_file(&layout.runtime).unwrap();
    fs::create_dir(&layout.runtime).unwrap();
    fs::set_permissions(&layout.runtime, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(install(&layout, &bins).unwrap_err().contains("private"));
}
