pub fn solve(data: &[u64], _: &[u64], k: usize) -> Vec<u64> {
    let mut output = data.to_vec();
    output.sort_unstable_by(|a, b| b.cmp(a));
    output.truncate(k);
    output
}
