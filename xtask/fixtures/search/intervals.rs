pub fn solve(data: &[u64], _queries: &[u64], _k: usize) -> Vec<u64> {
    let mut intervals: Vec<_> = data.as_chunks::<2>().0.iter().map(|p| (p[0], p[1])).collect();
    intervals.sort_unstable();
    let mut result: Vec<u64> = Vec::with_capacity(data.len());
    for (lo,hi) in intervals {
        let n = result.len();
        if n != 0 && lo <= result[n-1] { result[n-1] = result[n-1].max(hi); }
        else { result.push(lo); result.push(hi); }
    }
    result
}
