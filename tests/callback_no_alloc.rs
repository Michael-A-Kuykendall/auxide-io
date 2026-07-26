use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

struct CountingAllocator;

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        System.alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
}

fn reset_alloc_count() {
    ALLOC_COUNT.store(0, Ordering::Relaxed);
}

fn alloc_count() -> usize {
    ALLOC_COUNT.load(Ordering::Relaxed)
}

#[test]
fn callback_no_alloc() {
    // Setup: Diagnostics Arc and data buffer are allocated once during stream construction.
    let d = auxide_io::stream_controller::Diagnostics::new();
    let mut data = [0.0f32; 128];

    // Reset counter — everything from here on mirrors a single callback invocation.
    reset_alloc_count();

    d.callback_count.fetch_add(1, Ordering::Relaxed);
    if data.len() > 16384 {
        d.overflow_count.fetch_add(1, Ordering::Relaxed);
    }
    data.fill(0.0);
    let max_sample = data.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
    d.update_peak(max_sample);

    let snapshot = auxide_io::stream_controller::DiagnosticsSnapshot {
        callback_count: d.callback_count.load(Ordering::Relaxed),
        overflow_count: d.overflow_count.load(Ordering::Relaxed),
        peak: f32::from_bits(d.peak.load(Ordering::Relaxed)),
        latency: None,
    };
    assert_eq!(snapshot.callback_count, 1);
    assert_eq!(snapshot.overflow_count, 0);
    assert_eq!(snapshot.peak, 0.0);

    assert_eq!(
        alloc_count(),
        0,
        "per-callback operations must not allocate"
    );
}
