//! Local diagnostic enrollment shares the normal source limits and replay.
use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

fn config(spool: PathBuf, logs: Vec<PathBuf>) -> Config {
    Config {
        spool,
        logs,
        interval_s: 15,
        spool_bytes: 1024 * 1024,
        server: None,
        traces_listen: None,
        max_output_bytes_per_s: None,
    }
}

fn view(logs: Vec<String>) -> RemoteView {
    RemoteView {
        revision: 1,
        paused: false,
        logs,
        metric_interval_s: 30,
    }
}

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root =
            std::env::var_os("FABRIC_SCRATCH_ROOT").expect("resource launcher scratch required");
        let path = PathBuf::from(root).join(format!(
            "pinned-logs-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).expect("clean pinned-log scratch");
    }
}

#[test]
fn pinned_logs_survive_remote_replacement_and_restart() {
    let scratch = Scratch::new();
    let pinned = scratch.0.join("spindle.log");
    std::fs::write(&pinned, "diagnostic fixture\n").unwrap();
    let base = config(scratch.0.join("spool"), vec!["/old-source".into()]);
    let remote_path = scratch.0.join("missing-remote-source");
    let remote = view(vec![remote_path.to_str().unwrap().into()]);
    write_applied(&base.spool, &remote).unwrap();
    for restart in 0..2 {
        let mut node = Spindle::open_with_local_logs(base.clone(), vec![pinned.clone()]).unwrap();
        assert!(node.config.logs.contains(&pinned));
        assert!(node.config.logs.contains(&remote_path));
        assert!(!node.config.logs.contains(&PathBuf::from("/old-source")));
        assert_eq!(node.applied_revision(), 1);
        assert_eq!(node.interval_s(), 30);
        let cycle = node.collect_logs().unwrap().unwrap();
        assert_eq!(cycle.log_records, if restart == 0 { 1 } else { 0 });
        let mut bodies = Vec::new();
        node.journal
            .replay(|batch| {
                if !batch.logs.is_empty() {
                    let request = ExportLogsServiceRequest::decode(batch.logs.as_slice()).unwrap();
                    for group in request.resource_logs {
                        for scope in group.scope_logs {
                            for record in scope.log_records {
                                bodies.push(record.body.unwrap().value.unwrap());
                            }
                        }
                    }
                }
                Ok(())
            })
            .unwrap();
        assert_eq!(
            bodies,
            vec![any_value::Value::StringValue("diagnostic fixture".into())]
        );
    }
}

#[test]
fn pinned_sources_share_total_cap_and_deduplicate() {
    let base = config("/spool".into(), Vec::new());
    let sixteen: Vec<_> = (0..16).map(|n| format!("/source-{n:02}")).collect();
    let pinned = vec![PathBuf::from("/diagnostic")];
    assert!(effective(&base, &view(sixteen.clone()), &pinned).is_err());
    let fifteen = view(sixteen[..15].to_vec());
    let accepted = effective(&base, &fifteen, &pinned).unwrap();
    assert_eq!(accepted.logs.len(), 16);
    let duplicate = vec![PathBuf::from(&sixteen[0]), PathBuf::from(&sixteen[0])];
    let accepted = effective(&base, &view(sixteen), &duplicate).unwrap();
    assert_eq!(accepted.logs.len(), 16);
    assert!(accepted.logs.windows(2).all(|paths| paths[0] < paths[1]));
    assert!(with_local_logs(config("/spool".into(), accepted.logs), &pinned).is_err());
    assert!(with_local_logs(base, &["relative.log".into()]).is_err());
}

#[test]
fn invalid_stored_view_does_not_remove_pins_or_overwrite_record() {
    let scratch = Scratch::new();
    let base = config(scratch.0.join("spool"), vec!["/local-source".into()]);
    let remote = view((0..16).map(|n| format!("/remote-{n}")).collect());
    write_applied(&base.spool, &remote).unwrap();
    let node = Spindle::open_with_local_logs(base.clone(), vec!["/diagnostic".into()]).unwrap();
    assert_eq!(
        node.config.logs,
        vec![PathBuf::from("/diagnostic"), PathBuf::from("/local-source")]
    );
    assert_eq!(node.applied_revision(), 0);
    assert!(node.config_error.is_some());
    assert_eq!(read_applied(&base.spool), Some(remote));
}

#[test]
fn legacy_open_does_not_enroll_diagnostics() {
    let scratch = Scratch::new();
    let base = config(scratch.0.join("spool"), Vec::new());
    let node = Spindle::open(base).unwrap();
    assert!(node.config.logs.is_empty());
    assert!(node.local_logs.is_empty());
    assert!(!scratch.0.join("spool/diagnostics").exists());
}
