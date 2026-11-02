use crate::collection::Observation;

/// Select matching evidence in input order, up to `limit` records.
pub fn filter<'a>(records: &'a [Observation], kind: &str, limit: usize) -> Vec<&'a Observation> {
    records
        .iter()
        .filter(|r| r.kind == kind)
        .take(limit)
        .collect()
}

pub fn lexical<'a>(records: &'a [Observation], term: &str, limit: usize) -> Vec<&'a Observation> {
    if term.is_empty() {
        return Vec::new();
    }
    records
        .iter()
        .filter(|r| r.text.contains(term))
        .take(limit)
        .collect()
}

/// Exact cosine similarity; zero vectors and unequal dimensions are excluded.
pub fn cosine(a: &[f64], b: &[f64]) -> Option<f64> {
    if a.is_empty() || a.len() != b.len() || a.iter().chain(b).any(|n| !n.is_finite()) {
        return None;
    }
    let dot: f64 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let aa: f64 = a.iter().map(|x| x * x).sum();
    let bb: f64 = b.iter().map(|x| x * x).sum();
    let value = dot / (aa.sqrt() * bb.sqrt());
    value.is_finite().then_some(value)
}

/// Last-value forecast; requires a nonempty, finite history.
pub fn forecast(values: &[f64]) -> Option<f64> {
    if values.iter().any(|v| !v.is_finite()) {
        return None;
    }
    values.last().copied()
}
