#![forbid(unsafe_code)]

use fabric_client::{
    FabricBlame, FabricClient, FabricCustody, FabricDoctor, FabricDrift, FabricEnvelope,
    FabricLineage, FabricMemory, FabricMetricQuery, FabricQueryClient, FabricSnapshot, custody_lag,
    format_bytes_iec, mem_used_pct, parse_envelope,
};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TEST_SEQ: AtomicU64 = AtomicU64::new(0);

struct NonTmpScratch(PathBuf);

impl NonTmpScratch {
    fn new(prefix: &str) -> Self {
        let base = non_tmp_base();
        let seq = TEST_SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = base.join(format!("{prefix}-{}-{seq}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create disk-backed test scratch");
        assert!(
            !dir.starts_with("/tmp") && !dir.starts_with("/dev/shm"),
            "test scratch must stay off RAM-backed tmpfs: {}",
            dir.display()
        );
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for NonTmpScratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn non_tmp_base() -> PathBuf {
    if let Some(raw) = std::env::var_os("TMPDIR") {
        let p = PathBuf::from(raw);
        if p.is_absolute()
            && !p.starts_with("/tmp")
            && !p.starts_with("/dev/shm")
            && std::fs::create_dir_all(&p).is_ok()
        {
            return p;
        }
    }
    for candidate in ["/data/tmp", "/var/tmp"] {
        let p = PathBuf::from(candidate);
        if std::fs::create_dir_all(&p).is_ok() {
            return p;
        }
    }
    panic!("no non-tmpfs scratch base directory available");
}

fn write_exec(path: &Path, body: &str) {
    std::fs::write(path, body).expect("write stub");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod stub");
}

#[test]
fn schema_deserializes_live_and_nullable_fabric_cli_v1_fields() {
    assert_eq!(format_bytes_iec(512), "512 B");
    assert_eq!(format_bytes_iec(4096), "4 KiB");
    assert_eq!(format_bytes_iec(16 * 1024 * 1024), "16 MiB");
    assert!((mem_used_pct(&FabricMemory::default())).abs() < 1e-6);

    let snap: FabricEnvelope<FabricSnapshot> = parse_envelope(
        br#"{
            "schema_version": "fabric.cli.v1",
            "generated_at_unix_ms": 1795113125304,
            "command": "snapshot",
            "data": {
                "timestamp_unix_ms": 1795113123300,
                "hostname": "example-node",
                "psi": { "cpu_some": { "avg10": 1.25, "avg60": 0.9, "avg300": 0.5, "total_usec": 100 } },
                "memory": {
                    "mem_total_bytes": 1000,
                    "mem_available_bytes": 600,
                    "zram": { "device": "zram0", "same_pages": 9, "pages_compacted": 4, "compression_ratio": 3.0 }
                },
                "agents": [
                    { "session_id": "s1", "agent_name": "worker", "slice": "agent.slice", "unit": null, "pids": [12], "rss_bytes": 4096 }
                ],
                "gpus": [{ "card": "card0", "vendor": "0x10de", "name": "gpu0" }],
                "self_health": { "pid": 10, "rss_bytes": 1024, "open_fds": 5, "fabrico11y_slice_confined": true }
            }
        }"#,
    )
    .expect("valid snapshot envelope with nullable agent unit");
    assert_eq!(snap.data.hostname, "example-node");
    assert_eq!(snap.data.agents[0].unit, "");
    assert_eq!(snap.data.gpus[0].name.as_deref(), Some("gpu0"));
    assert!((mem_used_pct(&snap.data.memory) - 40.0).abs() < 1e-6);

    let custody: FabricEnvelope<FabricCustody> = parse_envelope(
        br#"{
            "schema_version": "fabric.cli.v1",
            "command": "custody",
            "data": {
                "spindle_socket": null,
                "spindle_online": false,
                "highest_record_id": 50,
                "canonical_through": 12
            }
        }"#,
    )
    .expect("custody with null spindle_socket");
    assert_eq!(custody.data.spindle_socket, "");
    assert_eq!(custody_lag(&custody.data), 38);

    let bad = br#"{"schema_version":"fabric.cli.v2","command":"snapshot","data":{"hostname":"h"}}"#;
    let err = parse_envelope::<FabricSnapshot>(bad).unwrap_err();
    assert!(err.contains("schema_version"), "unexpected error: {err}");

    let _blame: FabricEnvelope<FabricBlame> = parse_envelope(
        br#"{"schema_version":"fabric.cli.v1","command":"blame","data":{"resource":"memory","entries":[{"rank":1,"node_id":"pid:1","label":"x","target_path":"/data/tmp/foo","lineage_chain":["a"]}]}}"#,
    )
    .unwrap();
    let _drift: FabricEnvelope<FabricDrift> = parse_envelope(
        br#"{"schema_version":"fabric.cli.v1","command":"drift","data":{"total_findings":1,"warn_count":1,"findings":[{"pillar":"lineage","severity":"warn","target":"u","summary":"s","metric_value":1.0,"threshold_value":0.0}]}}"#,
    )
    .unwrap();
    let _doc: FabricEnvelope<FabricDoctor> = parse_envelope(
        br#"{"schema_version":"fabric.cli.v1","command":"doctor","data":{"healthy":true,"daemon_running":true,"cgroup_path":"/fabrico11y.slice","tmp_dir":"/data/tmp","checks":[{"name":"rss_budget","passed":true,"actual":"59 MiB","threshold":"< 100 MiB","detail":"ok"}]}}"#,
    )
    .unwrap();
    let _query: FabricEnvelope<FabricMetricQuery> = parse_envelope(
        br#"{"schema_version":"fabric.cli.v1","command":"query","data":{"domain":"metrics","row_count":1,"rows":[{"host":"h","name":"host.mem.available_bytes","timestamp_ms":10,"value":42.0,"pid":null,"labels_json":"{}"}]}}"#,
    )
    .unwrap();
    let lin: FabricEnvelope<FabricLineage> = parse_envelope(
        br#"{"schema_version":"fabric.cli.v1","command":"lineage","data":{"target":"app.slice","matched_roots":["slice:app.slice"],"direction":"downstream","max_depth":2,"nodes":[{"id":"slice:app.slice","kind":"slice","label":"app.slice","metadata":{"severity":"warn"}}],"edges":[{"src":"slice:app.slice","dst":"unit:app.scope","kind":"controls","weight":1024.0,"note":"unit"}]}}"#,
    )
    .unwrap();
    assert_eq!(lin.data.nodes.len(), 1);
}

#[tokio::test]
async fn client_preserves_partial_warnings_and_missing_binary_state() {
    let missing = FabricClient::with_bin(None, None);
    assert!(!missing.is_available());
    assert!(missing.bin_path().is_none());
    let offline = missing.collect().await;
    assert!(!offline.online);
    assert!(offline.error.is_some());

    let scratch = NonTmpScratch::new("fabric-client-test");
    let stub = scratch.path().join("fabric-stub.sh");
    write_exec(
        &stub,
        r#"#!/bin/sh
case "$1" in
  snapshot) echo '{"schema_version":"fabric.cli.v1","command":"snapshot","data":{"timestamp_unix_ms":100,"hostname":"stub-node","memory":{"mem_total_bytes":1000,"mem_available_bytes":600}}}' ;;
  blame) echo "blame subsystem unavailable" >&2; exit 2 ;;
  drift) echo '{"schema_version":"fabric.cli.v99","command":"drift","data":{}}' ;;
  doctor) echo '{"schema_version":"fabric.cli.v1","command":"doctor","data":{"healthy":true,"daemon_running":true,"pid":42,"rss_bytes":1024,"open_fds":8,"tmp_files_created":0,"fabrico11y_slice_confined":true}}' ;;
  query) echo "parquet scan failed" >&2; exit 3 ;;
  lineage) echo '{"schema_version":"fabric.cli.v1","command":"lineage","data":{"target":"app.slice","matched_roots":["slice:app.slice"],"direction":"downstream","max_depth":2,"nodes":[],"edges":[]}}' ;;
  *) echo "stub error" >&2; exit 1 ;;
esac
"#,
    );

    let client = FabricClient::with_bin(Some(stub), None);
    assert!(client.is_available());
    let bundle = client.collect().await;
    assert!(bundle.online);
    assert!(bundle.error.is_none());
    assert_eq!(bundle.source_desc, "fabric@stub-node");
    assert!(
        bundle
            .warnings
            .iter()
            .any(|w| w.contains("blame: blame subsystem unavailable"))
    );
    assert!(
        bundle
            .warnings
            .iter()
            .any(|w| w.contains("drift:") && w.contains("schema_version"))
    );
    assert!(bundle.doctor.is_some_and(|d| d.healthy));

    let fast = client.collect_fast().await;
    assert!(fast.online);
    assert!(fast.blame_memory.is_none());

    let hist_err = client.try_query_mem_history().await.unwrap_err();
    assert!(hist_err.contains("parquet scan failed"));
    assert!(client.query_mem_history().await.is_empty());
    assert!(client.collect_blame("invalid_res").await.is_err());
    assert!(client.collect_lineage("  ").await.is_err());
    let lin = client
        .collect_lineage("app.slice")
        .await
        .expect("lineage ok");
    assert_eq!(lin.target, "app.slice");
    let doc = client.collect_doctor().await.expect("doctor ok");
    assert_eq!(doc.pid, Some(42));
}

#[tokio::test]
async fn query_client_enforces_credential_file_strips_env_and_redacts_errors() {
    let scratch = NonTmpScratch::new("fabric-query-client-test");
    let dir = scratch.path();

    let missing_cred = dir.join("missing.token");
    let qstub = dir.join("fabric-query-stub.sh");
    write_exec(
        &qstub,
        r#"#!/bin/sh
if [ -n "${FABRIC_CREDENTIAL:-}" ]; then
  echo "FABRIC_CREDENTIAL must not be set in child env" >&2
  exit 1
fi
TOK="$(tr -d '\r\n' < "$FABRIC_CREDENTIAL_FILE")"
case "$1" in
  progress)
    if [ "$FABRIC_ENDPOINT" = "127.0.0.1:17440" ] && [ "$TOK" = "tok-never-echoed-secret-01234567" ]; then
      echo '{"result":"progress","authority":"auth-a","frontiers":{"origin-1":"42"}}'
    else
      echo "auth failure with token $TOK" >&2
      exit 1
    fi
    ;;
  evidence)
    echo "{\"result\":\"found\",\"row\":{\"canonical_id\":\"$2\",\"origin\":\"origin-1\",\"source_id\":\"1\",\"raw\":\"{}\"}}"
    ;;
  cancel)
    echo "{\"result\":\"cancelled\",\"query_id\":\"$2\"}"
    ;;
  *)
    exit 1
    ;;
esac
"#,
    );

    let missing_client =
        FabricQueryClient::with_bin(Some(qstub.clone()), "127.0.0.1:17440", &missing_cred);
    assert!(missing_client.progress().await.is_err());

    let empty_cred = dir.join("empty.token");
    std::fs::write(&empty_cred, "").expect("write empty");
    let empty_client =
        FabricQueryClient::with_bin(Some(qstub.clone()), "127.0.0.1:17440", &empty_cred);
    assert!(empty_client.progress().await.is_err());

    let cred = dir.join("cred.token");
    std::fs::write(&cred, "tok-never-echoed-secret-01234567\n").expect("write cred");
    std::fs::set_permissions(&cred, std::fs::Permissions::from_mode(0o600)).expect("chmod cred");

    let link_cred = dir.join("link.token");
    symlink(&cred, &link_cred).expect("create symlink");
    let sym_client =
        FabricQueryClient::with_bin(Some(qstub.clone()), "127.0.0.1:17440", &link_cred);
    let sym_err = sym_client.progress().await.unwrap_err();
    assert!(sym_err.contains("symlinked"));

    let qclient = FabricQueryClient::with_bin(Some(qstub.clone()), "127.0.0.1:17440", &cred);
    let progress = qclient.progress().await.expect("progress ok");
    assert_eq!(progress["frontiers"]["origin-1"], "42");
    let evidence = qclient.evidence(9).await.expect("evidence ok");
    assert_eq!(evidence["row"]["canonical_id"], "9");
    let cancelled = qclient.cancel("q-1").await.expect("cancel ok");
    assert_eq!(cancelled["query_id"], "q-1");

    let debug_str = format!("{qclient:?}");
    assert!(!debug_str.contains("tok-never-echoed-secret-01234567"));

    let bad_q = FabricQueryClient::with_bin(Some(qstub), "[::1]:19999", &cred);
    let qerr = bad_q.progress().await.unwrap_err();
    assert!(
        !qerr.contains("tok-never-echoed-secret-01234567"),
        "token leaked in error: {qerr}"
    );
    assert!(qerr.contains("[REDACTED]"));
}
