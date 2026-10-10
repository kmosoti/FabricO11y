//! Opt-in, bounded benchmark ledger; never installed by shipping binaries.
//! Native effects and allocator observations belong outside the semantic core.
use std::cell::Cell;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// Thread CPU ns, global allocator live/peak/requested bytes. Global allocation
/// observations overlap concurrent threads and nested spans; they are not
/// exclusive per-phase ownership or C-library allocation accounting.
pub type Sample = [u64; 4];
struct Observer {
    sample: fn() -> Sample,
    epoch: Instant,
    rows: Mutex<Vec<Record>>,
}
static OBSERVER: OnceLock<Observer> = OnceLock::new();
thread_local! { static DEPTH: Cell<usize> = const { Cell::new(0) }; }
#[derive(Debug)]
pub struct Record {
    pub name: &'static str,
    pub thread: std::thread::ThreadId,
    pub depth: usize,
    pub start_ns: u64,
    pub wall_ns: u64,
    pub before: Sample,
    pub after: Sample,
}
/// Install once before fixture construction. No stdout or filesystem work in
/// useful spans; bounded ledger capacity fails visibly rather than losing data.
pub fn install(sample: fn() -> Sample) {
    assert!(
        OBSERVER
            .set(Observer {
                sample,
                epoch: Instant::now(),
                rows: Mutex::new(Vec::with_capacity(262_144))
            })
            .is_ok()
    );
}
pub struct Span(Option<(&'static str, Instant, Sample, usize)>);
pub fn span(name: &'static str) -> Span {
    let value = OBSERVER.get().map(|observer| {
        let before = (observer.sample)();
        let depth = DEPTH.with(|d| {
            let n = d.get();
            d.set(n + 1);
            n
        });
        (name, Instant::now(), before, depth)
    });
    Span(value)
}
impl Drop for Span {
    fn drop(&mut self) {
        if let Some((name, started, before, depth)) = self.0.take() {
            let ended = Instant::now();
            let observer = OBSERVER.get().unwrap();
            let after = (observer.sample)();
            DEPTH.with(|d| d.set(depth));
            let row = Record {
                name,
                thread: std::thread::current().id(),
                depth,
                start_ns: started.duration_since(observer.epoch).as_nanos() as u64,
                wall_ns: ended.duration_since(started).as_nanos() as u64,
                before,
                after,
            };
            let mut rows = observer.rows.lock().unwrap();
            assert!(rows.len() < 262_144, "phase probe ledger exhausted");
            rows.push(row);
        }
    }
}
pub fn take() -> Vec<Record> {
    OBSERVER
        .get()
        .map(|o| std::mem::take(&mut *o.rows.lock().unwrap()))
        .unwrap_or_default()
}
