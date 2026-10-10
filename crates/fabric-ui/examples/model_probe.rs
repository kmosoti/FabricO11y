use fabric_ui::model::{ChartPoint, EnvelopeBucket, Sample, TimeWindow, chart_envelope};
use std::{hint::black_box, mem::size_of, time::Instant};

fn raw(samples: &[Sample], window: TimeWindow) -> Vec<ChartPoint> {
    samples
        .iter()
        .filter_map(|s| {
            s.value.map(|value| ChartPoint {
                time_ns: s.time_ns,
                x: (s.time_ns - window.start_ns) as f64 / (window.end_ns - window.start_ns) as f64,
                value,
            })
        })
        .collect()
}

// Previous per-sample floor formula retained as the ablation reference.
fn division_envelope(samples: &[Sample], window: TimeWindow) -> Vec<EnvelopeBucket> {
    let span = window.end_ns - window.start_ns;
    let mut previous = window.start_ns;
    for sample in samples {
        assert!(sample.time_ns >= previous && sample.time_ns <= window.end_ns);
        assert!(sample.value.is_none_or(f64::is_finite));
        previous = sample.time_ns;
    }
    let mut output: Vec<_> = (0..256)
        .map(|index| EnvelopeBucket {
            index,
            min: None,
            max: None,
            has_gap: false,
        })
        .collect();
    for sample in samples {
        let delta = sample.time_ns - window.start_ns;
        let index = (u128::from(delta) * 256 / u128::from(span)).min(255) as usize;
        let bucket = &mut output[index];
        let Some(value) = sample.value else {
            bucket.has_gap = true;
            continue;
        };
        let point = ChartPoint {
            time_ns: sample.time_ns,
            x: delta as f64 / span as f64,
            value,
        };
        if bucket.min.is_none_or(|old| value < old.value) {
            bucket.min = Some(point);
        }
        if bucket.max.is_none_or(|old| value > old.value) {
            bucket.max = Some(point);
        }
    }
    output
}

fn main() {
    let samples: Vec<_> = (0..65_536u64)
        .map(|i| Sample {
            time_ns: 1_700_000_000_000_000_000 + i,
            value: if i % 1024 == 1023 {
                None
            } else if i % 512 == 511 {
                Some(10_000.)
            } else {
                Some((i % 256) as f64)
            },
        })
        .collect();
    let window = TimeWindow {
        start_ns: samples[0].time_ns,
        end_ns: samples.last().unwrap().time_ns,
    };
    for _ in 0..5 {
        black_box(raw(black_box(&samples), window));
        black_box(division_envelope(black_box(&samples), window));
        black_box(chart_envelope(black_box(&samples), window, 256).unwrap());
    }
    let mut raw_us = Vec::new();
    let mut envelope_us = Vec::new();
    let mut division_us = Vec::new();
    for iteration in 0..21 {
        let start = Instant::now();
        black_box(raw(black_box(&samples), window));
        raw_us.push(start.elapsed().as_secs_f64() * 1_000_000.);
        for candidate in if iteration % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        } {
            let start = Instant::now();
            if candidate {
                black_box(chart_envelope(black_box(&samples), window, 256).unwrap());
                envelope_us.push(start.elapsed().as_secs_f64() * 1_000_000.);
            } else {
                black_box(division_envelope(black_box(&samples), window));
                division_us.push(start.elapsed().as_secs_f64() * 1_000_000.);
            }
        }
    }
    let output = chart_envelope(&samples, window, 256).unwrap();
    assert_eq!(output, division_envelope(&samples, window));
    // This registered fixture partitions into 256-sample buckets; independent
    // direct slices check the bucket formula and spike/gap preservation.
    for (bucket, original) in output.iter().zip(samples.as_chunks::<256>().0.iter()) {
        let values: Vec<_> = original.iter().filter_map(|s| s.value).collect();
        assert_eq!(
            bucket.min.unwrap().value,
            values.iter().copied().reduce(f64::min).unwrap()
        );
        assert_eq!(
            bucket.max.unwrap().value,
            values.iter().copied().reduce(f64::max).unwrap()
        );
        assert_eq!(bucket.has_gap, original.iter().any(|s| s.value.is_none()));
        for point in [bucket.min.unwrap(), bucket.max.unwrap()] {
            assert!(
                original
                    .iter()
                    .any(|s| s.time_ns == point.time_ns && s.value == Some(point.value))
            );
        }
    }
    raw_us.sort_by(f64::total_cmp);
    envelope_us.sort_by(f64::total_cmp);
    division_us.sort_by(f64::total_cmp);
    let raw_points = samples.iter().filter(|s| s.value.is_some()).count();
    let output_points = output
        .iter()
        .map(|b| usize::from(b.min.is_some()) + usize::from(b.max.is_some()))
        .sum::<usize>();
    println!(
        "{{\"samples\":{},\"buckets\":{},\"raw_points\":{},\"envelope_points\":{},\"raw_logical_bytes\":{},\"envelope_logical_bytes\":{},\"raw_p50_us\":{},\"raw_p95_us\":{},\"envelope_p50_us\":{},\"envelope_p95_us\":{},\"division_p50_us\":{},\"division_p95_us\":{},\"invariants\":\"passed\"}}",
        samples.len(),
        output.len(),
        raw_points,
        output_points,
        raw_points * size_of::<ChartPoint>(),
        output.len() * size_of::<fabric_ui::model::EnvelopeBucket>(),
        raw_us[10],
        raw_us[19],
        envelope_us[10],
        envelope_us[19],
        division_us[10],
        division_us[19]
    );
}
