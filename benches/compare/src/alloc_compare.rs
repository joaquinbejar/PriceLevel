//! Allocation-count comparison for a representative subset of operations,
//! run separately from the Criterion pass (`benches/compare.rs`). Mirrors
//! the counting `#[global_allocator]` pattern in the main crate's
//! `benches/latency/alloc.rs` (same justification for the `unsafe impl
//! GlobalAlloc`: bench-only code, confined to this one binary, never part of
//! `cargo build` / `cargo build --release` of the published crate).
//!
//! # Scope, and how this differs from `BENCH.md`'s allocation pass
//!
//! `benches/latency/alloc_measurements.rs` in the main crate pre-builds
//! every harness-owned buffer OUTSIDE the counted window so it isolates one
//! bare public-API call's own allocations. The measurements below are
//! coarser: each `measure(...)` closure below includes its own per-iteration
//! fixture construction (building a fresh level, its resting orders, or the
//! JSON to restore from) INSIDE the counted window, because that
//! construction differs in shape from 0.9.2 to 0.10 (e.g. `PriceLevel::new`
//! itself, `Arc<OrderType<()>>` construction) and this comparison's goal is
//! "same total workload, either version", not an isolated single-call
//! count. Treat these numbers as "allocations for this whole workload unit",
//! comparable release-to-release, not as the isolated per-call counts
//! `BENCH.md` reports.
//!
//! Run (once per feature):
//! ```sh
//! cargo run --manifest-path benches/compare/Cargo.toml --release --bin alloc_compare --features old
//! cargo run --manifest-path benches/compare/Cargo.toml --release --bin alloc_compare --features new
//! ```
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use pricelevel_compare::VERSION_LABEL;
use pricelevel_compare::pl::{self, TakerKind, TimeInForce};
use pricelevel_compare::workloads as w;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

struct CountingAllocator;

static COUNTING_ENABLED: AtomicBool = AtomicBool::new(false);
static ALLOC_COUNT: AtomicU64 = AtomicU64::new(0);
static ALLOC_BYTES: AtomicU64 = AtomicU64::new(0);
static DEALLOC_COUNT: AtomicU64 = AtomicU64::new(0);
static DEALLOC_BYTES: AtomicU64 = AtomicU64::new(0);

// SAFETY: every method delegates unconditionally to `System`, which already
// satisfies `GlobalAlloc`'s contract; this wrapper only adds non-mutating
// counter bookkeeping around each delegated call.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING_ENABLED.load(Ordering::Relaxed) {
            ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
            ALLOC_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        // SAFETY: `layout` is forwarded unchanged from the caller.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if COUNTING_ENABLED.load(Ordering::Relaxed) {
            DEALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
            DEALLOC_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        // SAFETY: `ptr`/`layout` are forwarded unchanged from the caller.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if COUNTING_ENABLED.load(Ordering::Relaxed) {
            DEALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
            DEALLOC_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
            ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
            ALLOC_BYTES.fetch_add(new_size as u64, Ordering::Relaxed);
        }
        // SAFETY: same forwarding contract as `alloc`/`dealloc`, with
        // `new_size` passed through unchanged.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

#[derive(Clone, Copy, Default)]
struct Stats {
    alloc_count: u64,
    alloc_bytes: u64,
    dealloc_count: u64,
    dealloc_bytes: u64,
}

fn read() -> Stats {
    Stats {
        alloc_count: ALLOC_COUNT.load(Ordering::Relaxed),
        alloc_bytes: ALLOC_BYTES.load(Ordering::Relaxed),
        dealloc_count: DEALLOC_COUNT.load(Ordering::Relaxed),
        dealloc_bytes: DEALLOC_BYTES.load(Ordering::Relaxed),
    }
}

fn reset() {
    ALLOC_COUNT.store(0, Ordering::Relaxed);
    ALLOC_BYTES.store(0, Ordering::Relaxed);
    DEALLOC_COUNT.store(0, Ordering::Relaxed);
    DEALLOC_BYTES.store(0, Ordering::Relaxed);
}

/// Runs `op` `reps` times with counting enabled and reports the per-op
/// average. `op` must not allocate anything the caller wants excluded (build
/// every harness-owned input BEFORE calling this, matching
/// `benches/latency/alloc_measurements.rs`'s discipline).
fn measure<F: FnMut()>(name: &str, reps: u32, mut op: F) {
    reset();
    COUNTING_ENABLED.store(true, Ordering::Relaxed);
    for _ in 0..reps {
        op();
    }
    COUNTING_ENABLED.store(false, Ordering::Relaxed);
    let s = read();
    println!(
        "{:<28} version={:<8} reps={:<6} alloc_count/op={:<10.2} alloc_bytes/op={:<12.2} dealloc_count/op={:<10.2} dealloc_bytes/op={:<12.2}",
        name,
        VERSION_LABEL,
        reps,
        f64::from(s.alloc_count as u32) / f64::from(reps),
        s.alloc_bytes as f64 / f64::from(reps),
        f64::from(s.dealloc_count as u32) / f64::from(reps),
        s.dealloc_bytes as f64 / f64::from(reps),
    );
}

fn main() {
    let reps: u32 = std::env::var("PL_COMPARE_ALLOC_REPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2_000);

    let id_gen = w::trade_id_generator();

    measure("add_order_standard", reps, || {
        let level = w::fresh_level();
        let _ = level.add_order(w::standard_order(1, w::BASE_PRICE, 10, pl::Side::Buy));
    });

    measure("match_full", reps, || {
        let level = w::match_standard_full_fixture();
        let _ = w::run_match(
            &level,
            100,
            pl::Id::sequential(2),
            TimeInForce::Gtc,
            TakerKind::Standard,
            &id_gen,
        );
    });

    measure("match_sweep_100", reps, || {
        let level = w::match_sweep_fixture();
        let _ = w::run_match(
            &level,
            100,
            pl::Id::sequential(3),
            TimeInForce::Gtc,
            TakerKind::Standard,
            &id_gen,
        );
    });

    measure("snapshot_capture_100", reps, || {
        let level = w::iter_orders_fixture(100);
        let _ = w::snapshot_capture(&level);
    });

    measure("snapshot_to_json_100", reps, || {
        let level = w::iter_orders_fixture(100);
        let _ = w::snapshot_json(&level);
    });

    measure("restore_100", reps, || {
        let level = w::iter_orders_fixture(100);
        let json = w::snapshot_json(&level);
        let _ = w::restore_from_json(&json);
    });

    measure("match_result_analytics_256", reps, || {
        let result = w::match_result_with_trades(256);
        let _ = result.executed_quantity();
        let _ = result.executed_value();
        let _ = result.average_price();
    });

    measure("trade_list_parse_32", reps, || {
        let text = w::trade_list_text(32);
        let _ = w::parse_trade_list(&text);
    });
}
