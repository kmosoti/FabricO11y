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

/// Floating-point cosine similarity; empty, zero, nonfinite and unequal vectors are excluded.
pub fn cosine(a: &[f64], b: &[f64]) -> Option<f64> {
    if a.is_empty() || a.len() != b.len() || a.iter().chain(b).any(|n| !n.is_finite()) {
        return None;
    }
    let scale_a = a.iter().fold(0.0_f64, |scale, x| scale.max(x.abs()));
    let scale_b = b.iter().fold(0.0_f64, |scale, x| scale.max(x.abs()));
    if scale_a == 0.0 || scale_b == 0.0 {
        return None;
    }
    // Positive scaling preserves the angle and keeps each squared component in
    // [0, 1], with at least one unit component in each nonzero vector.
    let (dot, aa, bb) = a
        .iter()
        .zip(b)
        .fold((0.0, 0.0, 0.0), |(dot, aa, bb), (x, y)| {
            let x = x / scale_a;
            let y = y / scale_b;
            (dot + x * y, aa + x * x, bb + y * y)
        });
    let value = dot / (aa.sqrt() * bb.sqrt());
    value.is_finite().then_some(value.clamp(-1.0, 1.0))
}

/// Last-value forecast; requires a nonempty, finite history.
pub fn forecast(values: &[f64]) -> Option<f64> {
    if values.iter().any(|v| !v.is_finite()) {
        return None;
    }
    values.last().copied()
}
