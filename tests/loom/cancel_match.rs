//! loom model of the cancel-vs-partial-fill linearization (issue #81).
//!
//! # Why a model, not the real `OrderQueue`
//!
//! loom verifies concurrent code by exhaustively exploring thread interleavings,
//! but it can only do so for synchronization performed through *its own*
//! instrumented primitives (`loom::sync::*`, `loom::sync::atomic::*`). The real
//! [`OrderQueue`](pricelevel) is built on `dashmap` and `crossbeam_skiplist`,
//! which are third-party and **not** loom-aware: loom cannot see the locks /
//! atomics inside them, so a loom test driving the real queue would explore
//! essentially no meaningful interleavings of the structures that actually carry
//! the race. Such a test would "pass" while proving nothing.
//!
//! Instead this models the **protocol** the #81 fix relies on: the partial-fill
//! pop→update and a concurrent cancel of the same id both run under the maker's
//! per-entry lock (DashMap's per-entry / shard lock in the real code, a
//! `loom::sync::Mutex` here), so they serialize. The model reproduces the exact
//! shape of [`OrderQueue::match_front`]'s `KeepInPlace` action vs
//! [`OrderQueue::remove`]'s cancel, plus the advisory `visible` counter, and
//! asserts the lost-cancel invariant under every interleaving loom can produce.
//!
//! Run with:
//! ```text
//! RUSTFLAGS="--cfg loom" cargo test --test loom_cancel_match
//! ```
//! Without `--cfg loom` this file compiles to an empty crate (the `loom` dev
//! dependency is only pulled in under `cfg(loom)`), so a normal `cargo test`
//! never builds or links loom.

#![cfg(loom)]

use loom::sync::Mutex;
use loom::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

/// The original maker quantity resting at the level, in quantity units.
const ORIGINAL_QTY: i64 = 10;
/// What the taker consumes from the maker on a partial fill.
const TAKER_QTY: i64 = 3;

/// A single resting maker, modelling one `orders` entry guarded by its
/// per-entry lock. `Some(qty)` = resident with `qty` visible; `None` = removed.
struct Entry {
    /// Guarded slot: the maker's current visible quantity, or `None` if removed.
    /// The `Mutex` stands in for DashMap's per-entry / shard lock that both the
    /// matcher (`entry()` / `get_mut`) and the canceller (`remove`) must take.
    slot: Mutex<Option<i64>>,
    /// The advisory visible-quantity counter (issue #68): updated outside the
    /// lock with `Relaxed`, exactly as `PriceLevel::match_order` / cancel do.
    visible: AtomicI64,
    /// Total quantity matched into trades (matcher side).
    traded: AtomicI64,
    /// Total quantity removed by the cancel (canceller side).
    cancelled: AtomicI64,
}

#[test]
fn cancel_match_same_id_linearizable() {
    loom::model(|| {
        let entry = Arc::new(Entry {
            slot: Mutex::new(Some(ORIGINAL_QTY)),
            visible: AtomicI64::new(ORIGINAL_QTY),
            traded: AtomicI64::new(0),
            cancelled: AtomicI64::new(0),
        });

        // Matcher: pop the front maker and partially fill it IN PLACE under the
        // per-entry lock (the #81 `KeepInPlace` action). It never removes the
        // maker from the slot before deciding, so a concurrent cancel cannot
        // slip into a remove-then-reinsert gap.
        let matcher = {
            let entry = Arc::clone(&entry);
            loom::thread::spawn(move || {
                let mut guard = match entry.slot.lock() {
                    Ok(g) => g,
                    Err(poisoned) => poisoned.into_inner(),
                };
                if let Some(qty) = *guard {
                    // Partial fill: consume TAKER_QTY, leave the residual in
                    // place (still resident under the same id / lock).
                    let consumed = TAKER_QTY.min(qty);
                    *guard = Some(qty - consumed);
                    drop(guard);
                    // Advisory counters updated after the locked commit, keyed
                    // off the action committed under the lock (matched in place).
                    entry.traded.fetch_add(consumed, Ordering::Relaxed);
                    entry.visible.fetch_sub(consumed, Ordering::Relaxed);
                }
            })
        };

        // Canceller: remove the maker under the SAME per-entry lock and
        // decrement the counter by whatever quantity it actually removed.
        let canceller = {
            let entry = Arc::clone(&entry);
            loom::thread::spawn(move || {
                let mut guard = match entry.slot.lock() {
                    Ok(g) => g,
                    Err(poisoned) => poisoned.into_inner(),
                };
                if let Some(qty) = guard.take() {
                    drop(guard);
                    entry.cancelled.fetch_add(qty, Ordering::Relaxed);
                    entry.visible.fetch_sub(qty, Ordering::Relaxed);
                }
            })
        };

        matcher.join().expect("matcher thread panicked");
        canceller.join().expect("canceller thread panicked");

        // ---- Linearization invariants (the lost-cancel guarantee) ----

        let traded = entry.traded.load(Ordering::Relaxed);
        let cancelled = entry.cancelled.load(Ordering::Relaxed);
        let visible = entry.visible.load(Ordering::Relaxed);
        let residual = match entry.slot.lock() {
            Ok(g) => *g,
            Err(poisoned) => *poisoned.into_inner(),
        };
        let resting = residual.unwrap_or(0);

        // 1. Conservation: every unit of the original maker is accounted for as
        //    either traded, cancelled, or still resting. Nothing vanishes or is
        //    double-counted.
        assert_eq!(
            traded + cancelled + resting,
            ORIGINAL_QTY,
            "quantity must be conserved: traded={traded} cancelled={cancelled} resting={resting}"
        );

        // 2. Counter <-> queue lockstep: the advisory visible counter equals the
        //    quantity still resting (the queue contents).
        assert_eq!(
            visible, resting,
            "visible counter ({visible}) must equal still-resting quantity ({resting})"
        );

        // 3. The lost-cancel guarantee itself: because the canceller always runs
        //    and takes whatever it finds under the lock, the maker is never left
        //    silently resting. The cancel either fully won (removed the original)
        //    or fully lost the race to the matcher and then removed the residual.
        assert_eq!(
            residual, None,
            "cancel must never be lost: the maker must not be left resting"
        );

        // 4. The cancel always removed a positive amount (it never silently
        //    no-ops while the order is still there): either the full original
        //    (cancel-first) or the post-fill residual (match-first).
        assert!(
            cancelled == ORIGINAL_QTY || cancelled == ORIGINAL_QTY - TAKER_QTY,
            "cancel removed {cancelled}, expected {ORIGINAL_QTY} (cancel-first) or \
             {} (match-first)",
            ORIGINAL_QTY - TAKER_QTY
        );
    });
}
