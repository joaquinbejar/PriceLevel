// benches/latency/alloc.rs
//! A minimal counting `#[global_allocator]` wrapper around
//! [`std::alloc::System`], scoped to this latency harness binary only.
//!
//! # Why `unsafe` is here, and why that is permitted
//!
//! Implementing [`std::alloc::GlobalAlloc`] requires `unsafe impl` — there is
//! no safe trait for a global allocator. `rules/global_rules.md` forbids new
//! production `unsafe`; this is bench-only code (`benches/latency/`, not
//! `src/`), which the same rules and the task that produced this file
//! explicitly permit for exactly this purpose. It ships in no build of the
//! library itself: `cargo build` / `cargo build --release` never compile
//! this file, only `cargo bench --bench latency` does.
//!
//! # What it counts, and why counting is disabled by default
//!
//! Every allocation and deallocation the process makes goes through
//! [`System`] regardless (this wrapper delegates unconditionally); the
//! `COUNTING_ENABLED` flag only gates whether the two counters are updated.
//! Latency scenarios run with counting **disabled** so the reported latency
//! numbers are not inflated by counter bookkeeping; the one atomic `Relaxed`
//! load per allocation call that remains even when disabled is the
//! irreducible cost of having this allocator installed at all, and is
//! disclosed in the run manifest's `allocator` field. The dedicated
//! allocation-measurement pass (`benches/latency/scenarios.rs`'s
//! `run_allocation_measurements`) enables counting, resets the counters,
//! runs a small number of representative single operations, and reads the
//! deltas back — entirely separate from any timed latency loop, per issue
//! #142's "measure allocation... separately from latency runs".

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Delegates every call to [`System`], counting bytes and call counts only
/// while [`COUNTING_ENABLED`] is set.
struct CountingAllocator;

static COUNTING_ENABLED: AtomicBool = AtomicBool::new(false);
static ALLOC_COUNT: AtomicU64 = AtomicU64::new(0);
static ALLOC_BYTES: AtomicU64 = AtomicU64::new(0);
static DEALLOC_COUNT: AtomicU64 = AtomicU64::new(0);
static DEALLOC_BYTES: AtomicU64 = AtomicU64::new(0);

// SAFETY: every method delegates to `System`, which already satisfies
// `GlobalAlloc`'s contract; this wrapper adds only non-mutating counter
// bookkeeping around each delegated call and returns exactly what `System`
// returned, so it preserves that contract unchanged.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING_ENABLED.load(Ordering::Relaxed) {
            ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
            ALLOC_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        // SAFETY: `layout` is exactly the caller's layout, forwarded
        // unchanged, and the caller of this method already upholds
        // `GlobalAlloc::alloc`'s preconditions on it.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if COUNTING_ENABLED.load(Ordering::Relaxed) {
            DEALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
            DEALLOC_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        // SAFETY: `ptr` / `layout` are exactly the caller's arguments,
        // forwarded unchanged, under the same precondition as above.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if COUNTING_ENABLED.load(Ordering::Relaxed) {
            // Count a realloc as one dealloc of the old size and one alloc of
            // the new size, so per-byte totals stay accurate even though it
            // is a single call.
            DEALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
            DEALLOC_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
            ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
            ALLOC_BYTES.fetch_add(new_size as u64, Ordering::Relaxed);
        }
        // SAFETY: same forwarding argument as `alloc` / `dealloc`, with
        // `new_size` forwarded unchanged from the caller.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

/// A point-in-time snapshot of the allocation counters.
#[derive(Debug, Clone, Copy, Default)]
pub struct AllocStats {
    /// Number of `alloc` (and the alloc half of `realloc`) calls counted.
    pub alloc_count: u64,
    /// Total bytes requested across those `alloc` calls.
    pub alloc_bytes: u64,
    /// Number of `dealloc` (and the dealloc half of `realloc`) calls counted.
    pub dealloc_count: u64,
    /// Total bytes released across those `dealloc` calls.
    pub dealloc_bytes: u64,
}

impl AllocStats {
    /// Reads the current counters without resetting them.
    #[must_use]
    pub fn read() -> Self {
        Self {
            alloc_count: ALLOC_COUNT.load(Ordering::Relaxed),
            alloc_bytes: ALLOC_BYTES.load(Ordering::Relaxed),
            dealloc_count: DEALLOC_COUNT.load(Ordering::Relaxed),
            dealloc_bytes: DEALLOC_BYTES.load(Ordering::Relaxed),
        }
    }

    /// Element-wise difference `self - earlier`, saturating at zero per
    /// field (the counters only ever increase while counting is enabled, so
    /// underflow here would indicate a reset raced the read, not a real
    /// negative allocation count).
    #[must_use]
    pub fn since(self, earlier: Self) -> Self {
        Self {
            alloc_count: self.alloc_count.saturating_sub(earlier.alloc_count),
            alloc_bytes: self.alloc_bytes.saturating_sub(earlier.alloc_bytes),
            dealloc_count: self.dealloc_count.saturating_sub(earlier.dealloc_count),
            dealloc_bytes: self.dealloc_bytes.saturating_sub(earlier.dealloc_bytes),
        }
    }
}

/// Resets every counter to zero. Call this immediately before an
/// allocation-measurement pass, with counting already enabled or about to be
/// enabled, so the read afterward reflects only that pass.
pub fn reset() {
    ALLOC_COUNT.store(0, Ordering::Relaxed);
    ALLOC_BYTES.store(0, Ordering::Relaxed);
    DEALLOC_COUNT.store(0, Ordering::Relaxed);
    DEALLOC_BYTES.store(0, Ordering::Relaxed);
}

/// Enables counter bookkeeping on every subsequent `alloc` / `dealloc` /
/// `realloc` call, on any thread, until [`disable`] is called.
pub fn enable() {
    COUNTING_ENABLED.store(true, Ordering::Relaxed);
}

/// Disables counter bookkeeping. Latency scenarios run with counting
/// disabled; see the module docs.
pub fn disable() {
    COUNTING_ENABLED.store(false, Ordering::Relaxed);
}
