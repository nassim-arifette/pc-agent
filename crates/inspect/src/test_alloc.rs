//! Test-only heap meter. The unit-test binary routes every allocation through the system
//! allocator unchanged; while a thread runs `measure`, its requested bytes are counted so tests
//! can compare accounted memory with what the code really keeps. Allocator bookkeeping (block
//! headers, rounding) is not included: compact structures with few large allocations make it
//! negligible, many small ones make it larger than counted.
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct Meter;

thread_local! {
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    static LIVE: Cell<isize> = const { Cell::new(0) };
    static PEAK: Cell<isize> = const { Cell::new(0) };
}

fn record(delta: isize) {
    // Const-initialized cells without destructors never allocate or panic on access.
    let _ = ACTIVE.try_with(|active| {
        if active.get() {
            LIVE.with(|live| {
                let now = live.get() + delta;
                live.set(now);
                PEAK.with(|peak| peak.set(peak.get().max(now)));
            });
        }
    });
}

// SAFETY: every method forwards the caller's layout and pointer unchanged to the system
// allocator, so the GlobalAlloc contract is upheld by `System`; the meter only updates
// thread-local counters, which neither allocate nor touch the returned memory.
unsafe impl GlobalAlloc for Meter {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded unchanged; the caller upholds `alloc`'s preconditions.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            record(layout.size() as isize);
        }
        ptr
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded unchanged; the caller upholds `alloc_zeroed`'s preconditions.
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            record(layout.size() as isize);
        }
        ptr
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller passes a block this allocator (System) returned with this layout.
        unsafe { System.dealloc(ptr, layout) };
        record(-(layout.size() as isize));
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: the caller passes a live System block, its layout and a valid new size.
        let moved = unsafe { System.realloc(ptr, layout, new_size) };
        if !moved.is_null() {
            // A moving reallocation briefly holds both blocks; count that transient peak too.
            record(new_size as isize);
            record(-(layout.size() as isize));
        }
        moved
    }
}

#[global_allocator]
static METER: Meter = Meter;

/// Heap bytes requested by the calling thread while `run` executed.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Usage {
    /// Highest live total above the starting point.
    pub peak: usize,
    /// Still allocated when `run` returned, including the returned value's heap.
    pub retained: usize,
}

pub(crate) fn measure<T>(run: impl FnOnce() -> T) -> (T, Usage) {
    let start = LIVE.with(Cell::get);
    PEAK.with(|peak| peak.set(start));
    ACTIVE.with(|active| active.set(true));
    let value = run();
    ACTIVE.with(|active| active.set(false));
    let live = LIVE.with(Cell::get);
    let peak = PEAK.with(Cell::get);
    (
        value,
        Usage {
            peak: (peak - start).max(0) as usize,
            retained: (live - start).max(0) as usize,
        },
    )
}
