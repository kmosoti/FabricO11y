pub fn solve(data: &[u64], _: &[u64], _: usize) -> Vec<u64> {
    let mut intervals: Vec<_> = data.as_chunks::<2>().0.to_vec();
    loop {
        let mut found = None;
        'search: for i in 0..intervals.len() {
            for j in i + 1..intervals.len() {
                let a = intervals[i];
                let b = intervals[j];
                if a[0] <= b[1] && b[0] <= a[1] {
                    found = Some((i, j));
                    break 'search;
                }
            }
        }
        let Some((i, j)) = found else { break; };
        let b = intervals.remove(j);
        intervals[i] = [intervals[i][0].min(b[0]), intervals[i][1].max(b[1])];
    }
    intervals.sort_unstable();
    intervals.into_iter().flatten().collect()
}
