use fabric::{canonical::CanonicalRow, query::*};
use fabric_query::Projection;
fn private_tempdir() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap()
}
use serde_json::Value;
use std::sync::atomic::AtomicBool;
const RAW: &str = include_str!("fixtures/mixed.jsonl");
fn oracle() -> Value {
    serde_json::from_str(include_str!("fixtures/oracle.json")).unwrap()
}
fn records() -> Vec<CanonicalRow> {
    let o = oracle();
    RAW.lines()
        .enumerate()
        .map(|(i, raw)| CanonicalRow {
            canonical_id: i as u64 + 1,
            origin: o["origins"][i].as_str().unwrap().into(),
            source_id: o["sources"][i].as_u64().unwrap(),
            raw: raw.into(),
        })
        .collect()
}
fn request(operation: Operation) -> QueryRequest {
    QueryRequest {
        version: 1,
        query_id: "test".into(),
        authority: None,
        scope: vec!["a".into(), "b".into(), "c".into()],
        snapshot: Snapshot::new(),
        filter: Filter::default(),
        operation,
        budgets: Budgets::default(),
        cursor: None,
    }
}
fn global() -> RunKey {
    RunKey::Global {
        run_id: "11111111-1111-4111-8111-111111111111".into(),
    }
}
fn loaded() -> (tempfile::TempDir, Projection) {
    let d = private_tempdir();
    let mut p = Projection::open(d.path(), 32 * 1024 * 1024).unwrap();
    p.apply(&records()).unwrap();
    (d, p)
}
#[test]
fn exact_wide_sums_cross_process_and_scope_before_aggregation() {
    let (_d, p) = loaded();
    let mut r = request(Operation::RunSummary);
    r.filter.run = Some(global());
    let response = p.query(r.clone()).unwrap();
    assert_eq!(response.rows.len(), 1);
    let typed: Vec<RunSummary> = response.decode_rows().unwrap();
    assert_eq!(typed[0].statistics.sum_ns, Some(36_893_488_147_419_103_231));
    let expected = oracle();
    for (key, val) in expected["global"].as_object().unwrap() {
        assert_eq!(&response.rows[0][key], val, "{key}");
    }
    assert_eq!(response.coverage.observed_gaps.len(), 1);
    r.scope = vec!["a".into()];
    let response = p.query(r).unwrap();
    for (key, val) in expected["origin_a"].as_object().unwrap() {
        assert_eq!(&response.rows[0][key], val, "{key}");
    }
}
#[test]
fn legacy_runs_remain_namespaced() {
    let (_d, p) = loaded();
    let response = p.query(request(Operation::ListRuns)).unwrap();
    assert_eq!(response.rows.len(), 4);
    let legacy: Vec<_> = response
        .rows
        .iter()
        .filter(|r| r["run"]["kind"] == "legacy")
        .collect();
    assert_eq!(legacy.len(), 2);
    assert_ne!(legacy[0]["run"]["origin"], legacy[1]["run"]["origin"]);
}
#[test]
fn snapshot_pages_stable_during_ingest_and_bound_to_scope() {
    let (_d, mut p) = loaded();
    let mut r = request(Operation::Evidence);
    r.budgets.rows = 2;
    let first = p.query(r.clone()).unwrap();
    assert_eq!(first.rows[0]["canonical_id"], "1");
    let mut new = records()[7].clone();
    new.canonical_id = 9;
    new.source_id = 6;
    new.raw = new.raw.replace("\"sequence\":4", "\"sequence\":5");
    p.apply(&[new]).unwrap();
    r.cursor = first.next_cursor;
    let mut all = first.rows;
    loop {
        let response = p.query(r.clone()).unwrap();
        all.extend(response.rows);
        if response.next_cursor.is_none() {
            break;
        }
        r.cursor = response.next_cursor;
    }
    assert_eq!(all.len(), 8);
    assert_eq!(all[7]["canonical_id"], "8");
    let mut wrong = r.clone();
    wrong.scope = vec!["a".into()];
    assert!(p.query(wrong).is_err());
    let mut tampered = r;
    let token = tampered.cursor.as_mut().unwrap();
    token.replace_range(0..1, "f");
    assert!(p.query(tampered).is_err());
}
#[test]
fn corrupt_files_fail_closed_and_checkpoint_survives_reopen() {
    let (d, p) = loaded();
    let progress = p.progress();
    drop(p);
    let p = Projection::open(d.path(), 32 * 1024 * 1024).unwrap();
    assert_eq!(p.progress(), progress);
    assert_eq!(p.query(request(Operation::Evidence)).unwrap().rows.len(), 8);
    assert!(Projection::open(d.path(), 32 * 1024 * 1024).is_err());
    drop(p);
    let file = std::fs::read_dir(d.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|e| e == "parquet"))
        .unwrap();
    std::fs::write(file, b"corrupt").unwrap();
    assert!(Projection::open(d.path(), 32 * 1024 * 1024).is_err());
}
#[test]
fn noncontiguous_and_quota_failures_do_not_advance() {
    let d = private_tempdir();
    let mut p = Projection::open(d.path(), 70 * 1024).unwrap();
    let mut row = records()[0].clone();
    row.source_id = 2;
    assert!(p.apply(&[row]).is_err());
    assert_eq!(p.progress().checkpoint, 0);
    let rows = records();
    let result = p.apply(&rows);
    assert!(result.is_err());
    assert_eq!(p.progress().checkpoint, 0);
    drop(p);
    assert_eq!(
        Projection::open(d.path(), 70 * 1024)
            .unwrap()
            .progress()
            .checkpoint,
        0
    );
}
#[test]
fn budgets_and_cancellation_never_return_partial_totals() {
    let (_d, p) = loaded();
    let mut r = request(Operation::RunSummary);
    r.budgets.scan_rows = 1;
    assert!(p.query(r).unwrap_err().contains("scan budget"));
    let cancelled = AtomicBool::new(true);
    assert!(
        p.query_with_cancel(request(Operation::RunSummary), &cancelled)
            .unwrap_err()
            .contains("cancelled")
    );
    assert!(p.query(request(Operation::RunSummary)).is_ok());
}
#[test]
fn time_filter_and_maximum_timestamp() {
    let (_d, p) = loaded();
    let mut r = request(Operation::Evidence);
    r.filter.start_ns = Some(u64::MAX);
    let response = p.query(r).unwrap();
    assert_eq!(response.rows.len(), 1);
    assert_eq!(response.rows[0]["timestamp_ns"], u64::MAX.to_string());
    let response = p
        .query(request(Operation::Activity { bucket_ns: 100 }))
        .unwrap();
    assert_eq!(
        response.rows.last().unwrap()["bucket_start_ns"],
        "18446744073709551600"
    );
}
#[test]
fn raw_evidence_retains_exact_bytes() {
    let (_d, p) = loaded();
    let response = p.query(request(Operation::Evidence)).unwrap();
    let typed: Vec<Evidence> = response.decode_rows().unwrap();
    assert_eq!(typed.len(), 8);
    for (row, raw) in response.rows.iter().zip(RAW.lines()) {
        assert_eq!(row["raw"].as_str().unwrap(), raw);
    }
}

#[test]
fn conflicting_v3_context_is_rejected_before_checkpoint() {
    let d = private_tempdir();
    let mut p = Projection::open(d.path(), 32 * 1024 * 1024).unwrap();
    let row = CanonicalRow {
        canonical_id: 1,
        origin: "a".into(),
        source_id: 1,
        raw: include_str!("fixtures/invalid-context-mismatch.json")
            .trim_end()
            .into(),
    };
    assert!(p.apply(&[row]).is_err());
    assert_eq!(p.progress().checkpoint, 0);
}

#[test]
fn service_tool_and_missing_duration_statistics() {
    let (_d, p) = loaded();
    let tools = p.query(request(Operation::ToolSummary)).unwrap();
    let tools: Vec<ToolSummary> = tools.decode_rows().unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].statistics.duration_count, 2);
    assert_eq!(tools[0].statistics.sum_ns, Some(18_446_744_073_709_551_616));
    assert_eq!(tools[0].statistics.errors, 1);
    assert_eq!(tools[0].statistics.cancelled, 1);
    let services = p.query(request(Operation::ServiceSummary)).unwrap();
    let services: Vec<ServiceSummary> = services.decode_rows().unwrap();
    assert_eq!(services[0].service, "api");
    assert_eq!(services[0].statistics.sum_ns, Some(7));
    let mut r = request(Operation::RunSummary);
    r.filter.start_ns = Some(150);
    r.filter.end_ns = Some(160);
    let gaps = p.query(r).unwrap();
    let gaps: Vec<RunSummary> = gaps.decode_rows().unwrap();
    assert_eq!(gaps[0].statistics.count, 1);
    assert_eq!(gaps[0].statistics.duration_count, 0);
    assert_eq!(gaps[0].statistics.sum_ns, None);
    assert_eq!(gaps[0].statistics.mean_ns, None);
}

#[test]
fn manifest_corruption_and_unpublished_files_are_not_skipped() {
    let (d, p) = loaded();
    let expected = p.progress();
    drop(p);
    std::fs::write(
        d.path().join("segment-unpublished.parquet"),
        b"interrupted publication",
    )
    .unwrap();
    std::fs::write(d.path().join("staging-interrupted"), b"partial").unwrap();
    let p = Projection::open(d.path(), 32 * 1024 * 1024).unwrap();
    assert_eq!(p.progress(), expected);
    assert!(!d.path().join("segment-unpublished.parquet").exists());
    assert!(!d.path().join("staging-interrupted").exists());
    drop(p);
    let path = d.path().join("manifest.json");
    let mut bytes = std::fs::read(&path).unwrap();
    let i = bytes.iter().position(|b| *b == b'8').unwrap();
    bytes[i] = b'7';
    std::fs::write(path, bytes).unwrap();
    assert!(Projection::open(d.path(), 32 * 1024 * 1024).is_err());
}

#[test]
fn coverage_scope_and_exact_output_budget() {
    let (_d, p) = loaded();
    let mut r = request(Operation::Evidence);
    r.scope = vec!["a".into()];
    r.budgets.output_bytes = 2048;
    let response = p.query(r).unwrap();
    assert_eq!(response.coverage.projection.frontiers.len(), 1);
    assert!(response.coverage.projection.frontiers.contains_key("a"));
    assert_eq!(response.coverage.canonical_frontier.len(), 1);
    let bytes = serde_json::to_vec(&response).unwrap().len();
    assert_eq!(bytes, response.coverage.consumed.output_bytes as usize);
    assert!(bytes <= 2048);
    assert!(response.coverage.truncated);
    assert!(!response.coverage.complete);
}

#[test]
fn canonical_authority_binds_replay_queries_and_cursors() {
    let d = private_tempdir();
    let mut p = Projection::open(d.path(), 32 * 1024 * 1024).unwrap();
    p.bind_authority("authority-a").unwrap();
    p.apply(&records()).unwrap();
    let mut r = request(Operation::Evidence);
    r.authority = Some("authority-a".into());
    r.budgets.rows = 1;
    let page = p.query(r.clone()).unwrap();
    assert_eq!(page.coverage.canonical_authority, "authority-a");
    r.cursor = page.next_cursor;
    let mut wrong = r.clone();
    wrong.authority = Some("authority-b".into());
    assert!(p.query(wrong).is_err());
    assert!(p.bind_authority("authority-b").is_err());
    drop(p);
    let mut p = Projection::open(d.path(), 32 * 1024 * 1024).unwrap();
    p.bind_authority("authority-a").unwrap();
    assert!(p.query(r).is_ok());
    assert!(p.bind_authority("authority-b").is_err());
}

#[test]
fn oversized_sparse_manifest_is_rejected_before_reading_payload() {
    let dir = private_tempdir();
    let manifest = dir.path().join("manifest.json");
    std::fs::File::create(&manifest)
        .unwrap()
        .set_len(1024 * 1024 * 1024)
        .unwrap();
    let started = std::time::Instant::now();
    let error = match Projection::open(dir.path(), 2 * 1024 * 1024 * 1024) {
        Ok(_) => panic!("oversized manifest accepted"),
        Err(error) => error,
    };
    assert!(error.contains("manifest byte limit"), "{error}");
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
    assert_eq!(
        std::fs::metadata(manifest).unwrap().len(),
        1024 * 1024 * 1024
    );
}

#[test]
fn oversized_apply_is_rejected_without_decoding_or_advancing() {
    let dir = private_tempdir();
    let mut p = Projection::open(dir.path(), 32 * 1024 * 1024).unwrap();
    let row = CanonicalRow {
        canonical_id: 1,
        origin: "a".into(),
        source_id: 1,
        raw: "{}".into(),
    };
    let too_many = vec![row.clone(); 8193];
    assert!(p.apply(&too_many).unwrap_err().contains("row limit"));
    assert_eq!(p.progress().checkpoint, 0);
    let mut large = row;
    large.raw = " ".repeat(1025);
    let too_large = vec![large; 8192];
    assert!(p.apply(&too_large).unwrap_err().contains("raw byte limit"));
    assert_eq!(p.progress().checkpoint, 0);
    assert_eq!(too_large[0].raw.len(), 1025);
    p.apply(&records()[..1]).unwrap();
    assert_eq!(p.progress().checkpoint, 1);
}

#[test]
fn existing_public_directory_is_rejected_without_chmod() {
    use std::os::unix::fs::PermissionsExt;
    let dir = private_tempdir();
    let public = dir.path().join("public");
    std::fs::create_dir(&public).unwrap();
    std::fs::set_permissions(&public, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(Projection::open(&public, 32 * 1024 * 1024).is_err());
    assert_eq!(
        std::fs::metadata(&public).unwrap().permissions().mode() & 0o777,
        0o755
    );
    assert!(!public.join("projection.lock").exists());
}

#[test]
fn symlink_directory_and_state_files_are_rejected() {
    use std::os::unix::fs::symlink;
    let base = private_tempdir();
    let target = base.path().join("actual");
    let p = Projection::open(&target, 32 * 1024 * 1024).unwrap();
    drop(p);
    let alias = base.path().join("alias");
    symlink(&target, &alias).unwrap();
    assert!(Projection::open(&alias, 32 * 1024 * 1024).is_err());
    let manifest = target.join("manifest.json");
    let manifest_target = target.join("retained-manifest.json");
    std::fs::rename(&manifest, &manifest_target).unwrap();
    symlink(&manifest_target, &manifest).unwrap();
    assert!(Projection::open(&target, 32 * 1024 * 1024).is_err());
    std::fs::remove_file(&manifest).unwrap();
    std::fs::rename(&manifest_target, &manifest).unwrap();
    let lock = target.join("projection.lock");
    std::fs::remove_file(&lock).unwrap();
    let lock_target = target.join("other-lock");
    std::fs::write(&lock_target, []).unwrap();
    symlink(&lock_target, &lock).unwrap();
    assert!(Projection::open(&target, 32 * 1024 * 1024).is_err());
    std::fs::remove_file(&lock).unwrap();
    std::fs::remove_file(&manifest).unwrap();
    std::fs::create_dir(&manifest).unwrap();
    assert!(Projection::open(&target, 32 * 1024 * 1024).is_err());
}

#[test]
fn symlink_segment_cannot_escape_manifest_file_validation() {
    use std::os::unix::fs::symlink;
    let (d, p) = loaded();
    drop(p);
    let segment = std::fs::read_dir(d.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|p| {
            p.extension()
                .is_some_and(|extension| extension == "parquet")
        })
        .unwrap();
    let target = d.path().join("retained-data");
    std::fs::rename(&segment, &target).unwrap();
    symlink(&target, &segment).unwrap();
    assert!(Projection::open(d.path(), 32 * 1024 * 1024).is_err());
}

#[test]
fn fifo_manifest_is_rejected_before_a_blocking_open() {
    let dir = private_tempdir();
    let path = dir.path().join("manifest.json");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .unwrap()
            .success()
    );
    let location = dir.path().to_path_buf();
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let _ = tx.send(Projection::open(location, 32 * 1024 * 1024).map(|_| ()));
    });
    let result = rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("special-file rejection must precede blocking open");
    assert!(result.unwrap_err().contains("regular nonsymlink"));
    worker.join().unwrap();
}

#[test]
fn manifest_nesting_is_bounded_before_deserialization() {
    let dir = private_tempdir();
    let deep = "[".repeat(17) + &"]".repeat(17);
    std::fs::write(dir.path().join("manifest.json"), deep).unwrap();
    let error = match Projection::open(dir.path(), 32 * 1024 * 1024) {
        Ok(_) => panic!("deep manifest accepted"),
        Err(error) => error,
    };
    assert!(error.contains("nesting limit"), "{error}");
}

#[test]
fn drop_releases_projection_lock_across_concurrent_subprocess_spawns() {
    use std::sync::{Arc, atomic::Ordering};

    let dir = private_tempdir();
    let stop = Arc::new(AtomicBool::new(false));
    let spawner_stop = Arc::clone(&stop);
    let spawner = std::thread::spawn(move || {
        while !spawner_stop.load(Ordering::Relaxed) {
            assert!(
                std::process::Command::new("true")
                    .status()
                    .unwrap()
                    .success()
            );
        }
    });
    for _ in 0..32 {
        let p = Projection::open(dir.path(), 32 * 1024 * 1024).unwrap();
        drop(p);
    }
    stop.store(true, Ordering::Relaxed);
    spawner.join().unwrap();
}

#[test]
fn pre_rename_apply_failure_does_not_poison_projection() {
    let dir = private_tempdir();
    let mut p = Projection::open(dir.path(), 32 * 1024 * 1024).unwrap();
    let collision = dir.path().join("staging-segment-1-0.parquet");
    std::fs::write(&collision, b"pre-existing collision fixture").unwrap();

    assert!(p.apply(&records()[..1]).is_err());
    assert!(
        collision.is_file(),
        "pre-existing staging collision file must not be removed"
    );
    assert_eq!(p.progress().checkpoint, 0);

    std::fs::remove_file(&collision).unwrap();
    let progress = p
        .apply(&records()[..1])
        .expect("projection must recover cleanly without reopen after pre-rename error");
    assert_eq!(progress.checkpoint, 1);
}

#[test]
fn origin_quota_caps_noisy_origin_and_preserves_fair_share_for_peers() {
    let dir = private_tempdir();
    let base = records();
    // Cap each omitted origin at 16 KiB while keeping plenty of global quota (32 MiB).
    let mut p = Projection::open_with_origin_quotas(
        dir.path(),
        32 * 1024 * 1024,
        Default::default(),
        Some(16 * 1024),
    )
    .unwrap();

    let mut cid = 0u64;
    let mut noisy_seq = 0u64;
    let noisy_row = |cid: &mut u64, seq: &mut u64| {
        *cid += 1;
        *seq += 1;
        CanonicalRow {
            canonical_id: *cid,
            origin: "noisy-origin".into(),
            source_id: *seq,
            raw: base[0].raw.clone(),
        }
    };

    // First segment from noisy-origin succeeds and consumes part of its 4096-byte origin budget.
    let first = noisy_row(&mut cid, &mut noisy_seq);
    p.apply(&[first]).unwrap();

    // Additional segments for noisy-origin hit the per-origin cap well before global 32 MiB.
    let mut hit_cap = false;
    for _ in 0..8 {
        let row = noisy_row(&mut cid, &mut noisy_seq);
        if let Err(err) = p.apply(&[row]) {
            assert!(
                err.contains("origin quota") || err.contains("projection quota"),
                "unexpected quota error: {err}"
            );
            hit_cap = true;
            // Roll back cid counter since failed apply didn't advance checkpoint.
            cid -= 1;
            break;
        }
    }
    assert!(hit_cap, "noisy-origin must hit fair-share origin cap");

    // Quiet peer origin still has its own fair share available.
    let quiet = CanonicalRow {
        canonical_id: cid + 1,
        origin: "quiet-origin".into(),
        source_id: 1,
        raw: base[0].raw.clone(),
    };
    let progress = p
        .apply(&[quiet])
        .expect("quiet peer origin must remain unblocked by noisy-origin quota exhaustion");
    assert_eq!(progress.frontiers.get("quiet-origin"), Some(&1));
}
