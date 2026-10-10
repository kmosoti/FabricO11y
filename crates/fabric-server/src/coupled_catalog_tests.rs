// Q4 additive controls share the original independent fixture/ledger helpers.
#[test]
fn coupled_reuse_hit_publication_reclaim_and_holder_fallback_are_exact() {
    use crate::coupled_catalog::SegmentSources;
    use std::sync::Arc;
    let s = Scratch::new("Q4 cached acquisition gap; original custody expectations");
    let state = s.0.join("state");
    let mut groups = fixture(2_000_000);
    groups.truncate(1);
    groups[0].entries.truncate(1);
    fs::write(s.0.join("expected-before-io.jsonl"), ledger(&groups)).unwrap();
    let mut expected: Vec<_> = ledger(&groups).lines().map(str::to_owned).collect();
    expected.sort();
    let mut log = journal(&state);
    append(&mut log, &groups);
    log.rotate(1).unwrap();
    drop(log);
    verify(&s, &state, &groups, "coupled-valid-query-control");
    let window = Window::new(0, 1u64 << 40).unwrap();
    let node = None;
    let request = || CatalogRequest {
        oldest: None,
        newest: 1,
        table: segment::Table::Logs,
        window: &window,
        node: &node,
        authorized_nodes: None,
        filter: FilterKind::None,
    };
    let mut catalog = ReadCatalog::new(&state);
    catalog.use_descriptor_reuse();
    let first = catalog.view(request()).unwrap();
    let second = catalog.view(request()).unwrap();
    let weak = match (&first.segments, &second.segments) {
        (
            SegmentSources::Reused { snapshot: a, .. },
            SegmentSources::Reused { snapshot: b, .. },
        ) => {
            assert!(Arc::ptr_eq(a, b));
            Arc::downgrade(a)
        }
        _ => panic!("expected actual descriptor-container hit"),
    };
    assert_eq!(custody(&first), expected);
    assert_eq!(custody(&second), expected);
    let sealed = state.join("journal/sealed-00000000000000000001.faj");
    let moved = catalog.view_at_discovery_cut(request(), || {
        segment::build_sealed(&state, 1, &sealed).unwrap();
        pass(&state, u64::MAX);
        assert!(!sealed.exists());
    });
    assert!(
        matches!(moved, Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted),
        "cached split discovery must explicitly retry"
    );
    let current = catalog.view(request()).unwrap();
    assert_eq!(custody(&current), expected);
    assert!(weak.upgrade().is_some());
    // Two retired holders plus two current holders hit the total four-holder cap.
    let current2 = catalog.view(request()).unwrap();
    let overflow = catalog.view(request()).unwrap();
    assert!(matches!(&overflow.segments, SegmentSources::Owned(_)));
    assert_eq!(custody(&overflow), expected);
    drop(first);
    drop(second);
    assert!(
        weak.upgrade().is_none(),
        "retired container must release after last reader"
    );
    drop(current);
    drop(current2);
    drop(overflow);
    verify(&s, &state, &groups, "coupled-exact-query-after-movement");
    let h = History::with_plan(&state, Plan::Walk).with_descriptor_reuse();
    let q = queries().remove(0);
    let answers = pages(&h, q.clone(), 1);
    grade(
        &s,
        &groups,
        &q,
        &answers,
        "coupled-actual-reuse-pages",
        "walk",
        true,
    );
    let mut omitted = answers.clone();
    omitted[0]["rows"].as_array_mut().unwrap().pop();
    grade(
        &s,
        &groups,
        &q,
        &omitted,
        "coupled-omitted-row-control",
        "walk",
        false,
    );
    let mut duplicate = answers.clone();
    let row = duplicate[0]["rows"][0].clone();
    duplicate[0]["rows"].as_array_mut().unwrap().push(row);
    grade(
        &s,
        &groups,
        &q,
        &duplicate,
        "coupled-duplicate-row-control",
        "walk",
        false,
    );
}

#[test]
fn coupled_append_with_unchanged_segment_labels_extends_exactly() {
    use crate::coupled_catalog::SegmentSources;
    use std::sync::Arc;
    let s = Scratch::new("Q4 append without publication; cached descriptor hit");
    let state = s.0.join("state");
    let mut groups = fixture(2_000_000);
    groups.truncate(2);
    for g in &mut groups {
        g.entries.truncate(1);
    }
    let mut log = journal(&state);
    append(&mut log, &groups[..1]);
    let mut catalog = ReadCatalog::new(&state);
    catalog.use_descriptor_reuse();
    let window = Window::new(0, 1u64 << 40).unwrap();
    let node = None;
    let request = |newest| CatalogRequest {
        oldest: None,
        newest,
        table: segment::Table::Logs,
        window: &window,
        node: &node,
        authorized_nodes: None,
        filter: FilterKind::None,
    };
    let before = catalog.view(request(1)).unwrap();
    append(&mut log, &groups[1..]);
    let after = catalog.view(request(2)).unwrap();
    match (&before.segments, &after.segments) {
        (
            SegmentSources::Reused { snapshot: a, .. },
            SegmentSources::Reused { snapshot: b, .. },
        ) => assert!(Arc::ptr_eq(a, b)),
        _ => panic!("expected unchanged-label descriptor reuse"),
    }
    let mut expected: Vec<_> = ledger(&groups).lines().map(str::to_owned).collect();
    expected.sort();
    assert_eq!(custody(&after), expected);
    drop(log);
    let h = History::with_plan(&state, Plan::Walk).with_descriptor_reuse();
    for q in queries() {
        let answers = pages(&h, q.clone(), 2);
        grade(
            &s,
            &groups,
            &q,
            &answers,
            "coupled-appended-exact-pages",
            "walk",
            true,
        );
    }
}
