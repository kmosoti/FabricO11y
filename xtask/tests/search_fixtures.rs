mod membership {
    include!("../fixtures/search/membership.rs");
}
mod topk {
    include!("../fixtures/search/topk.rs");
}
mod intervals {
    include!("../fixtures/search/intervals.rs");
}
mod starter_membership {
    include!("../fixtures/search/starters/membership.rs");
}
mod starter_topk {
    include!("../fixtures/search/starters/topk.rs");
}
mod starter_intervals {
    include!("../fixtures/search/starters/intervals.rs");
}
#[test]
fn baselines_and_starters_obey_boundary_contracts() {
    for f in [membership::solve, starter_membership::solve] {
        assert_eq!(f(&[0, u64::MAX, 0], &[u64::MAX, 1, 0, 1], 0), [1, 0, 1, 0]);
        assert_eq!(f(&[], &[0], 0), [0]);
    }
    for f in [topk::solve, starter_topk::solve] {
        assert_eq!(
            f(&[0, u64::MAX, u64::MAX, 1], &[], 3),
            [u64::MAX, u64::MAX, 1]
        );
        assert_eq!(f(&[0, 1], &[], usize::MAX), [1, 0]);
        assert!(f(&[1], &[], 0).is_empty());
    }
    for f in [intervals::solve, starter_intervals::solve] {
        assert_eq!(
            f(&[u64::MAX, u64::MAX, 2, 3, 0, 2, 4, 5], &[], 0),
            [0, 3, 4, 5, u64::MAX, u64::MAX]
        );
        assert_eq!(f(&[3, 8, 1, 3, 8, 9, 2, 2], &[], 0), [1, 9]);
        assert!(f(&[], &[], 0).is_empty());
    }
}
