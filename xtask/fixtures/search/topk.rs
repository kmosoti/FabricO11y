pub fn solve(data: &[u64], _queries: &[u64], k: usize) -> Vec<u64> {
    let k = k.min(data.len());
    if k == 0 { return Vec::new(); }
    let mut values = data.to_vec();
    if k < values.len() { values.select_nth_unstable_by(k, |a,b| b.cmp(a)); values.truncate(k); }
    values.sort_unstable_by(|a,b| b.cmp(a)); values
}
