pub fn solve(data: &[u64], queries: &[u64], _k: usize) -> Vec<u64> {
    let set: std::collections::HashSet<_> = data.iter().copied().collect();
    queries.iter().map(|q| u64::from(set.contains(q))).collect()
}
