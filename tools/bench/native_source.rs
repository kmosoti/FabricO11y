//! Native app-log feeder; never constructs or forwards Fabric Batches.
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
fn ns() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let root = &a[1];
    let per_tick: usize = a[2].parse().unwrap();
    assert!(per_tick == 50 || per_tick == 500);
    let mut files: Vec<File> = (0..20)
        .map(|i| {
            OpenOptions::new()
                .append(true)
                .open(format!("{root}/node{i:02}.log"))
                .unwrap()
        })
        .collect();
    let mut clocks = BufWriter::new(File::create(format!("{root}/source-clocks.csv")).unwrap());
    let began = Instant::now();
    let epoch = ns();
    for tick in 0..250_u64 {
        let due = began + Duration::from_millis(tick * 100);
        if let Some(wait) = due.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
        let factor = if (100..150).contains(&tick) { 3 } else { 1 };
        for (i, file) in files.iter_mut().enumerate() {
            let count = per_tick * factor;
            let mut bytes = Vec::with_capacity(count * 901);
            for j in 0..count {
                let prefix = format!("load-{i:02}:{tick:03}:{j:04} ");
                let mut body = prefix.into_bytes();
                let mut state =
                    2703163393_u64 ^ ((i as u64 + 1) << 40) ^ (tick << 16) ^ (j as u64 + 1);
                while body.len() < 900 {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    body.push(if (tick + j as u64) % 2 == 0 {
                        b'R'
                    } else {
                        b'!' + (state % 94) as u8
                    });
                }
                bytes.extend_from_slice(&body);
                bytes.push(b'\n');
            }
            file.write_all(&bytes).unwrap();
            let after = ns();
            writeln!(
                clocks,
                "{i},{tick},{count},{},{after},{}",
                epoch + u128::from(tick) * 100_000_000,
                began
                    .elapsed()
                    .as_nanos()
                    .saturating_sub(u128::from(tick) * 100_000_000)
            )
            .unwrap();
        }
    }
    clocks.flush().unwrap();
    println!(
        "{{\"epoch_ns\":{epoch},\"elapsed_s\":{}}}",
        began.elapsed().as_secs_f64()
    );
}
