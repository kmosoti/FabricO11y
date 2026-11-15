use std::{
    fs,
    net::TcpListener,
    os::unix::fs::{PermissionsExt, symlink},
    path::PathBuf,
};
use xtask::collection::{Layout, StackInstallOptions, install_stack};

fn stack_fixture() -> (tempfile::TempDir, Layout, PathBuf, PathBuf, PathBuf) {
    let temp = xtask::evidence::non_tmp_tempdir().unwrap();
    let layout = Layout {
        home: temp.path().join("home"),
        runtime: temp.path().join("run"),
        config: temp.path().join("config"),
        state: temp.path().join("state"),
    };
    let bins = temp.path().join("bin");
    fs::create_dir(&bins).unwrap();
    for name in [
        "fabric",
        "spindle",
        "fabric-server",
        "fabric-adapter",
        "fabric-query",
        "fabric-mcp",
    ] {
        let path = bins.join(name);
        fs::write(&path, b"#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let data_dir = temp.path().join("data");
    let tmp_dir = temp.path().join("tmp");
    fs::create_dir(&data_dir).unwrap();
    fs::set_permissions(&data_dir, fs::Permissions::from_mode(0o755)).unwrap();
    for public_sub in ["state", "olap"] {
        let sub = data_dir.join(public_sub);
        fs::create_dir(&sub).unwrap();
        fs::set_permissions(&sub, fs::Permissions::from_mode(0o755)).unwrap();
    }
    fs::create_dir(&tmp_dir).unwrap();
    fs::set_permissions(&tmp_dir, fs::Permissions::from_mode(0o700)).unwrap();
    (temp, layout, bins, data_dir, tmp_dir)
}

#[test]
fn stack_install_couples_credentials_origin_authority_and_is_idempotent() {
    let (_temp, layout, bins, data_dir, tmp_dir) = stack_fixture();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let options = StackInstallOptions {
        data_dir: data_dir.clone(),
        tmp_dir,
        listen: format!("127.0.0.1:{port}"),
    };

    install_stack(&layout, &bins, &options).unwrap();
    install_stack(&layout, &bins, &options).unwrap();

    let cfg_dir = layout.config.join("fabrico11y");
    let server_cfg_path = cfg_dir.join("server.json");
    let spindle_cfg_path = cfg_dir.join("spindle.json");
    let node_tok_path = cfg_dir.join("spindle-node.token");
    let reader_tok_path = cfg_dir.join("fabric-reader.token");

    for private in [
        &server_cfg_path,
        &spindle_cfg_path,
        &node_tok_path,
        &reader_tok_path,
    ] {
        assert_eq!(
            fs::metadata(private).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    let node_tok = fs::read_to_string(&node_tok_path).unwrap();
    let reader_tok = fs::read_to_string(&reader_tok_path).unwrap();
    assert_ne!(node_tok.trim(), reader_tok.trim());
    assert_eq!(node_tok.trim().len(), 64);
    assert_eq!(reader_tok.trim().len(), 64);

    let server_cfg: fabric_server::Config =
        serde_json::from_slice(&fs::read(&server_cfg_path).unwrap()).unwrap();
    let spindle_cfg: fabric::spindle::Config =
        serde_json::from_slice(&fs::read(&spindle_cfg_path).unwrap()).unwrap();

    spindle_cfg.validate().unwrap();
    assert_eq!(server_cfg.listen, options.listen);
    assert_eq!(server_cfg.nodes.len(), 1);
    assert_eq!(server_cfg.nodes[0].credential_file, node_tok_path);
    assert_eq!(server_cfg.reader_credential_file, reader_tok_path);

    let fwd = spindle_cfg.forwarding.as_ref().unwrap();
    assert_eq!(fwd.endpoint, options.listen);
    assert_eq!(fwd.credential_file, node_tok_path);
    assert!(!fwd.reclaim_confirmed);

    assert_eq!(
        fs::metadata(&data_dir).unwrap().permissions().mode() & 0o777,
        0o755
    );
    for private_sub in ["spindle", "server", "server/canonical", "server/projection"] {
        assert_eq!(
            fs::metadata(data_dir.join(private_sub))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }

    let (origin, authority) = {
        let live_store =
            fabric::spindle::custody::Store::open(&spindle_cfg.storage, spindle_cfg.quota_bytes)
                .unwrap();
        let live_ledger =
            fabric_server::ledger::Ledger::open(&server_cfg.storage, server_cfg.quota_bytes)
                .unwrap();
        // Repeated install-stack while services hold active owner.lock flocks remains idempotent.
        install_stack(&layout, &bins, &options).unwrap();
        (
            live_store.origin().to_owned(),
            live_ledger.authority.clone(),
        )
    };
    assert_eq!(server_cfg.nodes[0].origin, origin);
    assert_eq!(fwd.authority, authority);

    // Server opens cleanly with coupled credentials, projection authority binding, and node grants.
    let server = fabric_server::Server::open(server_cfg).unwrap();
    drop(server);

    let unit_dir = layout.config.join("systemd/user");
    let slice_text = fs::read_to_string(unit_dir.join("fabrico11y.slice")).unwrap();
    assert!(!slice_text.contains("[Install]"));
    assert!(!slice_text.contains("CPUAccounting"));
    for unit_name in [
        "fabric-server.service",
        "spindle.service",
        "fabric-daemon.service",
    ] {
        let unit_path = unit_dir.join(unit_name);
        assert_eq!(
            fs::metadata(&unit_path).unwrap().permissions().mode() & 0o777,
            0o644
        );
        let body = fs::read_to_string(&unit_path).unwrap();
        assert!(body.contains("Slice=fabrico11y.slice"));
        assert!(body.contains("KillMode=control-group"));
        assert!(body.contains("NoNewPrivileges=yes"));
        assert!(!body.contains("After=default.target"));
        assert!(!body.contains("PrivateTmp"));
        assert!(!body.contains("ProtectSystem=strict"));
        if unit_name != "fabric-daemon.service" {
            assert!(body.contains("Type=notify"));
            assert!(body.contains("NotifyAccess=main"));
        }
    }
    let daemon_body = fs::read_to_string(unit_dir.join("fabric-daemon.service")).unwrap();
    assert!(daemon_body.contains("--spindle-socket"));
    assert!(daemon_body.contains("--data-dir"));
    assert!(daemon_body.contains("--tmp-dir"));

    fs::write(&server_cfg_path, b"{}").unwrap();
    let err = install_stack(&layout, &bins, &options).unwrap_err();
    assert!(err.contains("installation conflict"), "unexpected: {err}");
}

#[test]
fn stack_install_preflights_conflicts_before_mutating_state() {
    let (_temp, layout, bins, data_dir, tmp_dir) = stack_fixture();
    let cfg_dir = layout.config.join("fabrico11y");
    fs::create_dir_all(&cfg_dir).unwrap();
    let spindle_cfg_path = cfg_dir.join("spindle.json");
    fs::write(
        &spindle_cfg_path,
        b"{\"socket\":\"/x\",\"storage\":\"/y\",\"quota_bytes\":1}\n",
    )
    .unwrap();
    fs::set_permissions(&spindle_cfg_path, fs::Permissions::from_mode(0o600)).unwrap();

    let err = install_stack(
        &layout,
        &bins,
        &StackInstallOptions {
            data_dir: data_dir.clone(),
            tmp_dir,
            listen: "127.0.0.1:17440".into(),
        },
    )
    .unwrap_err();
    assert!(err.contains("installation conflict"), "unexpected: {err}");
    assert!(!data_dir.join("spindle/custody.sqlite").exists());
    assert!(!data_dir.join("server/canonical/canonical.sqlite").exists());
    assert!(!cfg_dir.join("spindle-node.token").exists());

    // Corrupt existing canonical storage must fail preflight without mutating Spindle custody.
    let (_temp2, layout2, bins2, data_dir2, tmp_dir2) = stack_fixture();
    let corrupt_dir = data_dir2.join("server/canonical");
    fs::create_dir_all(&corrupt_dir).unwrap();
    fs::set_permissions(data_dir2.join("server"), fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(&corrupt_dir, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(corrupt_dir.join("canonical.sqlite"), b"not-sqlite").unwrap();
    let err = install_stack(
        &layout2,
        &bins2,
        &StackInstallOptions {
            data_dir: data_dir2.clone(),
            tmp_dir: tmp_dir2.clone(),
            listen: "127.0.0.1:17440".into(),
        },
    )
    .unwrap_err();
    assert!(err.contains("installation conflict"), "unexpected: {err}");
    assert!(!data_dir2.join("spindle/custody.sqlite").exists());
    assert!(!data_dir2.join("spindle/owner.lock").exists());
    assert!(
        !layout2
            .config
            .join("fabrico11y/spindle-node.token")
            .exists()
    );

    // Removing canonical.sqlite after initial coupled setup must fail preflight cleanly.
    let (_temp3, layout3, bins3, data_dir3, tmp_dir3) = stack_fixture();
    let opts3 = StackInstallOptions {
        data_dir: data_dir3.clone(),
        tmp_dir: tmp_dir3,
        listen: "127.0.0.1:17440".into(),
    };
    install_stack(&layout3, &bins3, &opts3).unwrap();
    let canonical_db = data_dir3.join("server/canonical/canonical.sqlite");
    fs::remove_file(&canonical_db).unwrap();
    let err = install_stack(&layout3, &bins3, &opts3).unwrap_err();
    assert!(err.contains("installation conflict"), "unexpected: {err}");
    assert!(!canonical_db.exists());

    // Readable SQLite canonical database with incompatible registries descriptor must fail preflight without mutating Spindle custody.
    let (_temp4, layout4, bins4, data_dir4, tmp_dir4) = stack_fixture();
    let server_parent4 = data_dir4.join("server");
    fs::create_dir(&server_parent4).unwrap();
    fs::set_permissions(&server_parent4, fs::Permissions::from_mode(0o700)).unwrap();
    let canonical_dir4 = server_parent4.join("canonical");
    let ledger4 = fabric_server::ledger::Ledger::open(&canonical_dir4, 256 * 1024 * 1024).unwrap();
    drop(ledger4);
    let conn4 = rusqlite::Connection::open(canonical_dir4.join("canonical.sqlite")).unwrap();
    conn4
        .execute(
            "UPDATE registries SET descriptor = '{\"version\":99}' WHERE name = 'fabrico11y/3'",
            [],
        )
        .unwrap();
    drop(conn4);
    let err = install_stack(
        &layout4,
        &bins4,
        &StackInstallOptions {
            data_dir: data_dir4.clone(),
            tmp_dir: tmp_dir4,
            listen: "127.0.0.1:17440".into(),
        },
    )
    .unwrap_err();
    assert!(
        err.contains("installation conflict") && err.contains("registry descriptor conflict"),
        "unexpected: {err}"
    );
    assert!(!data_dir4.join("spindle/custody.sqlite").exists());
    assert!(!data_dir4.join("spindle/owner.lock").exists());
    assert!(
        !layout4
            .config
            .join("fabrico11y/spindle-node.token")
            .exists()
    );

    // Uppercase persisted lease_key rejected by Store::unhex must fail preflight without opening Store or mutating files.
    let (_temp5, layout5, bins5, data_dir5, tmp_dir5) = stack_fixture();
    let spindle_dir5 = data_dir5.join("spindle");
    let store5 = fabric::spindle::custody::Store::open(&spindle_dir5, 64 * 1024 * 1024).unwrap();
    drop(store5);
    let conn5 = rusqlite::Connection::open(spindle_dir5.join("custody.sqlite")).unwrap();
    conn5
        .execute(
            "UPDATE custody_state SET value = ?1 WHERE key = 'lease_key'",
            ["AB".repeat(32)],
        )
        .unwrap();
    drop(conn5);
    fs::remove_file(spindle_dir5.join("owner.lock")).unwrap();
    let err = install_stack(
        &layout5,
        &bins5,
        &StackInstallOptions {
            data_dir: data_dir5.clone(),
            tmp_dir: tmp_dir5,
            listen: "127.0.0.1:17440".into(),
        },
    )
    .unwrap_err();
    assert!(
        err.contains("installation conflict") && err.contains("lease key"),
        "unexpected: {err}"
    );
    assert!(!data_dir5.join("spindle/owner.lock").exists());
    assert!(!data_dir5.join("server/canonical/canonical.sqlite").exists());
    assert!(
        !layout5
            .config
            .join("fabrico11y/spindle-node.token")
            .exists()
    );
}

#[test]
fn stack_install_rejects_tmp_paths_non_loopback_listen_and_symlinks() {
    let (temp, layout, bins, data_dir, tmp_dir) = stack_fixture();

    let err = install_stack(
        &layout,
        &bins,
        &StackInstallOptions {
            data_dir: PathBuf::from("/tmp/forbidden-fabric-data"),
            tmp_dir: tmp_dir.clone(),
            listen: "127.0.0.1:17440".into(),
        },
    )
    .unwrap_err();
    assert!(!err.is_empty());

    let err = install_stack(
        &layout,
        &bins,
        &StackInstallOptions {
            data_dir: data_dir.clone(),
            tmp_dir: PathBuf::from("/tmp/forbidden-fabric-tmp"),
            listen: "127.0.0.1:17440".into(),
        },
    )
    .unwrap_err();
    assert!(!err.is_empty());

    let mut bad_runtime_layout = layout.clone();
    bad_runtime_layout.runtime = PathBuf::from("/tmp/forbidden-runtime");
    let err = install_stack(
        &bad_runtime_layout,
        &bins,
        &StackInstallOptions {
            data_dir: data_dir.clone(),
            tmp_dir: tmp_dir.clone(),
            listen: "127.0.0.1:17440".into(),
        },
    )
    .unwrap_err();
    assert!(!err.is_empty());

    let err = install_stack(
        &layout,
        &bins,
        &StackInstallOptions {
            data_dir: data_dir.clone(),
            tmp_dir: tmp_dir.clone(),
            listen: "0.0.0.0:17440".into(),
        },
    )
    .unwrap_err();
    assert!(err.contains("loopback"), "unexpected: {err}");

    let err = install_stack(
        &layout,
        &bins,
        &StackInstallOptions {
            data_dir: data_dir.clone(),
            tmp_dir: tmp_dir.clone(),
            listen: "127.0.0.1:0".into(),
        },
    )
    .unwrap_err();
    assert!(err.contains("non-zero"), "unexpected: {err}");

    fs::create_dir(&layout.home).unwrap();
    symlink(temp.path(), &layout.runtime).unwrap();
    let err = install_stack(
        &layout,
        &bins,
        &StackInstallOptions {
            data_dir: data_dir.clone(),
            tmp_dir: tmp_dir.clone(),
            listen: "127.0.0.1:17440".into(),
        },
    )
    .unwrap_err();
    assert!(err.contains("symbolic link"), "unexpected: {err}");
    fs::remove_file(&layout.runtime).unwrap();

    let symlink_data = temp.path().join("data-symlink");
    symlink(&data_dir, &symlink_data).unwrap();
    let err = install_stack(
        &layout,
        &bins,
        &StackInstallOptions {
            data_dir: symlink_data,
            tmp_dir: tmp_dir.clone(),
            listen: "127.0.0.1:17440".into(),
        },
    )
    .unwrap_err();
    assert!(err.contains("symbolic link"), "unexpected: {err}");

    for bad_mode in [0o775, 0o777] {
        fs::set_permissions(&data_dir, fs::Permissions::from_mode(bad_mode)).unwrap();
        let err = install_stack(
            &layout,
            &bins,
            &StackInstallOptions {
                data_dir: data_dir.clone(),
                tmp_dir: tmp_dir.clone(),
                listen: "127.0.0.1:17440".into(),
            },
        )
        .unwrap_err();
        assert!(
            err.contains("group/world-writable") || err.contains("expected owned"),
            "unexpected for mode {bad_mode:o}: {err}"
        );
    }
    fs::set_permissions(&data_dir, fs::Permissions::from_mode(0o755)).unwrap();

    let writable_parent = temp.path().join("writable-parent");
    fs::create_dir(&writable_parent).unwrap();
    let nested_tmp = writable_parent.join("tmp");
    fs::create_dir(&nested_tmp).unwrap();
    fs::set_permissions(&nested_tmp, fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(&writable_parent, fs::Permissions::from_mode(0o777)).unwrap();
    let err = install_stack(
        &layout,
        &bins,
        &StackInstallOptions {
            data_dir: data_dir.clone(),
            tmp_dir: nested_tmp,
            listen: "127.0.0.1:17440".into(),
        },
    )
    .unwrap_err();
    assert!(err.contains("group/world-writable"), "unexpected: {err}");
    fs::set_permissions(&writable_parent, fs::Permissions::from_mode(0o755)).unwrap();
}
