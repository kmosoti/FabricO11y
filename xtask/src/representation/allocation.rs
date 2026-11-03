//! Rust allocation instrumentation; native SQLite allocations are excluded.
#[cfg(feature = "allocation-metrics")]
mod instrumented {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
    pub struct Counter;
    static TOTAL: AtomicUsize = AtomicUsize::new(0);
    static LIVE: AtomicUsize = AtomicUsize::new(0);
    static PEAK: AtomicUsize = AtomicUsize::new(0);
    fn add(n: usize) {
        TOTAL.fetch_add(n, Relaxed);
        let live = LIVE.fetch_add(n, Relaxed) + n;
        PEAK.fetch_max(live, Relaxed);
    }
    // SAFETY: every allocation and deallocation delegates unchanged layouts and
    // pointers to System; counters never dereference memory or allocate.
    unsafe impl GlobalAlloc for Counter {
        unsafe fn alloc(&self, l: Layout) -> *mut u8 {
            let p = unsafe { System.alloc(l) };
            if !p.is_null() {
                add(l.size());
            }
            p
        }
        unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
            let p = unsafe { System.alloc_zeroed(l) };
            if !p.is_null() {
                add(l.size());
            }
            p
        }
        unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
            unsafe { System.dealloc(p, l) };
            LIVE.fetch_sub(l.size(), Relaxed);
        }
        unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
            let q = unsafe { System.realloc(p, l, n) };
            if !q.is_null() {
                LIVE.fetch_sub(l.size(), Relaxed);
                add(n);
            }
            q
        }
    }
    pub fn stats() -> serde_json::Value {
        serde_json::json!({"allocated_bytes":TOTAL.load(Relaxed),"peak_live_bytes":PEAK.load(Relaxed),"live_bytes":LIVE.load(Relaxed),"scope":"whole-process Rust allocator; SQLite native allocations excluded"})
    }
}
#[cfg(feature = "allocation-metrics")]
pub use instrumented::{Counter, stats};
#[cfg(not(feature = "allocation-metrics"))]
pub fn stats() -> serde_json::Value {
    serde_json::Value::Null
}
