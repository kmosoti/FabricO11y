//! Real shared Sources lifetime controls; metadata does not lease Segment files.
use super::*;
use crate::sealer::{self, Retention};
use crate::store::{CommitMode, Entry, Group, Store};
use crate::tail::ManifestHandle;
use fabric_frame::envelope::Batch;
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, any_value};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use prost::Message;
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};

static NEXT: AtomicU64 = AtomicU64::new(0);
const SEED: u64 = 2703163393;
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let root = PathBuf::from(
            std::env::var("FABRIC_SCRATCH_ROOT").expect("contained data-drive scratch required"),
        );
        let path = root.join(format!(
            "catalog-lifetime-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("origin.json"), json!({"seed":SEED,"fixture":"real shared Sources metadata lifetime; files remain subject to retention"}).to_string()).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!(
                "catalog lifetime failure fixture retained: {}",
                self.0.display()
            );
        } else {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
}

fn publish(root: &Path, label: u64) {
    let logs = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            scope_logs: vec![ScopeLogs {
                log_records: (0..8)
                    .map(|index| LogRecord {
                        observed_time_unix_nano: SEED + label * 8 + index,
                        body: Some(AnyValue {
                            value: Some(any_value::Value::StringValue(format!(
                                "needle-λ-{label}-{index}"
                            ))),
                        }),
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
    .encode_to_vec();
    let batch = Batch {
        version: 1,
        node_id: vec![7; 16],
        generation: 1,
        sequence: label,
        logs,
        ..Default::default()
    }
    .encode_to_vec();
    let group = Group {
        group_sequence: label,
        entries: vec![Entry {
            label: "node-λ".into(),
            batch,
            received_unix_nano: SEED + label,
        }],
    };
    segment::build(root, label, &[group]).unwrap();
}

fn view(history: &History, newest: u64) -> Sources {
    let query: Query = serde_json::from_value(
        json!({"kind":"logs","from_ns":0,"to_ns":u64::MAX,"contains":"needle","limit":2}),
    )
    .unwrap();
    history
        .sources_walk(
            None,
            newest,
            &query,
            &Window {
                from_ns: 0,
                to_ns: u64::MAX,
            },
            &None,
            None,
        )
        .unwrap()
}

fn weak_manifest(view: &Sources, label: u64) -> Weak<segment::Manifest> {
    match &view
        .segments
        .iter()
        .find(|(_, m)| m.journal_label == label)
        .unwrap()
        .1
    {
        ManifestHandle::Shared(m) => Arc::downgrade(m),
        ManifestHandle::Owned(_) => panic!("expected shared Sources handle"),
    }
}

fn unique_manifests(views: &[&Sources]) -> usize {
    views
        .iter()
        .flat_map(|v| &v.segments)
        .map(|(_, m)| match m {
            ManifestHandle::Shared(m) => Arc::as_ptr(m) as usize,
            ManifestHandle::Owned(_) => panic!("owned handle cannot count as a shared identity"),
        })
        .collect::<HashSet<_>>()
        .len()
}

#[test]
fn held_sources_pin_unique_metadata_until_release_but_never_pin_retained_files() {
    let scratch = Scratch::new();
    let (intake, commit) = Store::open(&scratch.0, 64 * 1024 * 1024, CommitMode::INDIVIDUAL)
        .unwrap()
        .spawn_joinable()
        .unwrap();
    publish(&scratch.0, 1);
    publish(&scratch.0, 2);
    let history = History::with_plan(&scratch.0, Plan::Walk).with_shared_catalog();
    let first = view(&history, 2);
    let second = view(&history, 2);
    assert_eq!(unique_manifests(&[&first, &second]), 2);
    let weak = weak_manifest(&first, 1);
    assert!(Weak::ptr_eq(&weak, &weak_manifest(&second, 1)));
    let first_index = first
        .segments
        .iter()
        .position(|(_, m)| m.journal_label == 1)
        .unwrap();
    let second_index = second
        .segments
        .iter()
        .position(|(_, m)| m.journal_label == 1)
        .unwrap();
    let filter = Arc::downgrade(
        first.filters[first_index]
            .as_ref()
            .expect("verified real filter"),
    );
    assert!(Arc::ptr_eq(
        first.filters[first_index].as_ref().unwrap(),
        second.filters[second_index].as_ref().unwrap()
    ));
    // Negative control: same-content detached metadata must count as a new owner.
    let detached = Arc::new(weak.upgrade().unwrap().as_ref().clone());
    assert!(!Weak::ptr_eq(&weak, &Arc::downgrade(&detached)));
    assert_ne!(Arc::as_ptr(&detached), weak.as_ptr());
    drop(detached);
    for label in 3..=6 {
        publish(&scratch.0, label);
    }
    let newest = view(&history, 6);
    assert_eq!(unique_manifests(&[&first, &second, &newest]), 6);
    // Actual retention removes every real Segment despite paused metadata views.
    sealer::pass(
        &scratch.0,
        &intake,
        Retention {
            max_age_s: u64::MAX,
            max_bytes: 0,
        },
        1,
    )
    .unwrap();
    assert!(segment::list(&scratch.0).unwrap().is_empty());
    history.refresh_catalog(6).unwrap();
    assert!(view(&history, 6).segments.is_empty());
    assert!(weak.upgrade().is_some());
    assert!(filter.upgrade().is_some());
    assert!(!first.segments[first_index].0.exists());
    assert!(
        segment::scan_logs(
            &first.segments[first_index].0,
            &first.segments[first_index].1,
            0,
            u64::MAX,
            |_| {}
        )
        .is_err()
    );
    drop(newest);
    drop(first);
    assert!(
        weak.upgrade().is_some(),
        "second paused view must retain its owner"
    );
    drop(second);
    assert!(
        weak.upgrade().is_none(),
        "retired metadata must release after final view"
    );
    assert!(
        filter.upgrade().is_none(),
        "retired verified filter must release after final view"
    );
    drop(intake);
    commit.join().unwrap();
    println!(
        "{}",
        json!({"control":"real_retention_sources_lifetime","seed":SEED,
        "published_segments":6,"unique_initial_owners":2,"unique_after_publication":6,
        "same_content_distinct_owner_rejected":true,"files_removed_with_views_held":true,
        "manifest_and_filter_released":true})
    );
}

#[test]
fn aborting_a_paused_real_query_view_releases_its_metadata() {
    let scratch = Scratch::new();
    let (intake, commit) = Store::open(&scratch.0, 64 * 1024 * 1024, CommitMode::INDIVIDUAL)
        .unwrap()
        .spawn_joinable()
        .unwrap();
    publish(&scratch.0, 1);
    let history = History::with_plan(&scratch.0, Plan::Walk).with_shared_catalog();
    let sources = view(&history, 1);
    let weak = weak_manifest(&sources, 1);
    let index = sources
        .segments
        .iter()
        .position(|(_, m)| m.journal_label == 1)
        .unwrap();
    let filter = Arc::downgrade(sources.filters[index].as_ref().unwrap());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    runtime.block_on(async {
        let (entered, paused) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            entered.send(()).unwrap();
            std::future::pending::<()>().await;
            drop(sources); // Keeps actual Sources inside the paused future.
        });
        paused.await.unwrap();
        sealer::pass(
            &scratch.0,
            &intake,
            Retention {
                max_age_s: u64::MAX,
                max_bytes: 0,
            },
            1,
        )
        .unwrap();
        history.refresh_catalog(1).unwrap();
        assert!(
            weak.upgrade().is_some(),
            "paused future must still own its Sources"
        );
        assert!(filter.upgrade().is_some());
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(
            weak.upgrade().is_none(),
            "aborted future must release retired metadata"
        );
        assert!(filter.upgrade().is_none());
    });
    drop(intake);
    commit.join().unwrap();
    println!(
        "{}",
        json!({"control":"actual_tokio_sources_cancellation","seed":SEED,
        "held_before_abort":true,"manifest_and_filter_released_after_join":true})
    );
}

include!("coupled_catalog_lifetime_tests.rs");
