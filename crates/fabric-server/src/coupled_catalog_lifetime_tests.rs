// Q4 runs the same actual lifetime schedules with descriptor reuse.
#[test]
fn coupled_held_sources_pin_metadata_without_file_lease() {
    let scratch = Scratch::new();
    let (intake, commit) = Store::open(&scratch.0, 64 * 1024 * 1024, CommitMode::INDIVIDUAL)
        .unwrap()
        .spawn_joinable()
        .unwrap();
    publish(&scratch.0, 1);
    publish(&scratch.0, 2);
    let history = History::with_plan(&scratch.0, Plan::Walk).with_descriptor_reuse();
    let mut page_query =
        json!({"kind":"logs","from_ns":0,"to_ns":u64::MAX,"contains":"needle","limit":2});
    let page = history
        .run(&serde_json::from_value(page_query.clone()).unwrap(), 2)
        .unwrap();
    assert!(!page["next_page"].is_null());
    page_query["page"] = page["next_page"].clone();
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
    assert!(
        matches!(
            history.run(&serde_json::from_value(page_query).unwrap(), 6),
            Err(QueryError::Gone)
        ),
        "retired descriptor ownership must not resurrect an expired page"
    );
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
fn coupled_aborting_real_view_releases_retired_snapshot() {
    let scratch = Scratch::new();
    let (intake, commit) = Store::open(&scratch.0, 64 * 1024 * 1024, CommitMode::INDIVIDUAL)
        .unwrap()
        .spawn_joinable()
        .unwrap();
    publish(&scratch.0, 1);
    let history = History::with_plan(&scratch.0, Plan::Walk).with_descriptor_reuse();
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
