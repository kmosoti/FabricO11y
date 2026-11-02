pub fn solve(data: &[u64], queries: &[u64], _: usize) -> Vec<u64> {
    queries.iter().map(|q| u64::from(data.contains(q))).collect()
}
