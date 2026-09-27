//! loom model of the statistics seqlock under the single-writer contract
//! (issue #153).
//!
//! # What is modelled
//!
//! `PriceLevelStatistics` supports exactly one concurrent writer of its
//! execution aggregates (`record_execution`, driven by the single logical
//! matcher; `reset` only when quiescent). Multi-field readers (`Clone`, serde,
//! `Display`) go through `read_consistent`, a seqlock over `stats_seq`. This
//! model checks, under every interleaving and weak-memory outcome loom
//! explores, that a reader running concurrently with that ONE writer never
//! accepts a partial execution tuple, including across an overflow rollback
//! whose committed prefix is undone with `fetch_sub`.
//!
//! Overlapping writers are outside the contract (the guard entry is a plain
//! increment, not an exclusive acquire). They are not a passing case: the
//! `should_panic` test at the end pins that loom finds the partial-tuple
//! schedule described in #153, so the "unsupported" wording in the rustdoc
//! stays backed by evidence. If the publication protocol ever gains an
//! exclusive writer entry, that test starts failing and the docs must change
//! with it.
//!
//! The model is sensitive to the protocol: removing the reader's
//! `fence(Acquire)` makes `single_writer_reader_never_sees_partial_tuple`
//! fail.
//!
//! # Why a model, not the real type
//!
//! loom can only explore synchronization performed through its own
//! instrumented atomics. `PriceLevelStatistics` uses `std` atomics plus a
//! `portable_atomic::AtomicU128`, and `loom` is a `cfg(loom)` dev-dependency
//! that the library itself cannot import. Swapping the production atomics for
//! loom types would need `cfg(loom)` shims inside `src/` and a non-dev loom
//! dependency, so, as `tests/loom/cancel_match.rs` does for the queue, this
//! file reproduces the exact protocol with loom primitives:
//!
//! - writer entry: a `Relaxed` checked increment that refuses to open when
//!   the sequence exceeds `u64::MAX - 2` (issue #165), then `fence(Release)`
//!   (`WriteSeqGuard::try_new`); a refused entry only sets the degraded flag;
//! - field updates: `Relaxed` checked CAS adds, `Relaxed` `fetch_sub` rollback,
//!   `Relaxed` degraded-flag CAS (`record_execution`);
//! - writer exit: a checked `Release` increment, proven in range by the entry
//!   check (`WriteSeqGuard::drop`);
//! - reader: `seq.load(Acquire)`, retry if odd, `Relaxed` field loads,
//!   `fence(Acquire)`, `seq.load(Relaxed)`, accept iff unchanged
//!   (`read_consistent`).
//!
//! The model keeps three of the additive counters (`orders_executed`,
//! `quantity_executed`, `sum_waiting_time`) plus the degraded flag.
//! `value_executed` sits between `quantity_executed` and `sum_waiting_time` in
//! production and follows the identical `Relaxed` CAS add / `fetch_sub`
//! rollback pattern (loom also has no 128-bit atomic); it is left out only to
//! keep the exhaustive search tractable, since the first and last counters
//! already bracket the rollback. The real type is additionally exercised by
//! the single-writer / multi-reader stress test
//! `test_single_writer_readers_never_observe_partial_tuple` in
//! `src/price_level/tests/statistics.rs`.
//!
//! Run with:
//! ```text
//! RUSTFLAGS="--cfg loom" cargo test --test loom_stats_seqlock --release
//! ```
//! Each test uses a preemption bound (see `model`); `LOOM_MAX_PREEMPTIONS`
//! overrides it. Without `--cfg loom` this file compiles to an empty crate.

#![cfg(loom)]

use loom::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering, fence};
use std::sync::Arc;

/// Seed for `sum_waiting_time`: leaves 5 ms of headroom, so a record whose
/// waiting time exceeds it overflows at the LAST additive counter and has to
/// roll back the three counters it already advanced.
const WAIT_SEED: u64 = u64::MAX - 5;

/// Reader attempts per model execution. Bounded so loom's state space stays
/// finite; a reader that exhausts them without a clean copy returns `None`
/// (liveness is not what this model checks, coherence is).
const READ_ATTEMPTS: usize = 2;

/// Runs `f` under loom with a preemption bound, unless `LOOM_MAX_PREEMPTIONS`
/// sets one. Unbounded exploration of these writers against concurrent
/// readers does not finish in reasonable time; loom's documentation recommends
/// a bound of 2-3 as catching the overwhelming majority of ordering bugs, and
/// a torn seqlock read needs only one preemption inside the reader's copy.
fn model<F>(preemption_bound: usize, f: F)
where
    F: Fn() + Sync + Send + 'static,
{
    let mut builder = loom::model::Builder::new();
    if builder.preemption_bound.is_none() {
        builder.preemption_bound = Some(preemption_bound);
    }
    builder.check(f);
}

/// Model of the execution aggregates plus the seqlock sequence.
struct Stats {
    seq: AtomicU64,
    orders_executed: AtomicUsize,
    quantity_executed: AtomicU64,
    sum_waiting_time: AtomicU64,
    stats_degraded: AtomicBool,
}

/// One copy accepted by the reader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Tuple {
    orders_executed: usize,
    quantity_executed: u64,
    sum_waiting_time: u64,
    stats_degraded: bool,
}

/// Mirror of `STATS_SEQ_ENTRY_LIMIT` (issue #165).
const SEQ_ENTRY_LIMIT: u64 = u64::MAX - 2;

/// Mirror of `WriteSeqGuard`.
struct Guard<'a>(&'a AtomicU64);

impl<'a> Guard<'a> {
    fn enter(seq: &'a AtomicU64) -> Option<Self> {
        seq.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |s| {
            if s <= SEQ_ENTRY_LIMIT {
                s.checked_add(1)
            } else {
                None
            }
        })
        .ok()?;
        fence(Ordering::Release);
        Some(Self(seq))
    }
}

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        let _ = self
            .0
            .fetch_update(Ordering::Release, Ordering::Relaxed, |s| s.checked_add(1));
    }
}

/// Mirror of `checked_fetch_add_*`: `Relaxed` load + checked add + `Relaxed`
/// `compare_exchange_weak` retry.
fn checked_add_u64(target: &AtomicU64, delta: u64) -> Result<(), ()> {
    let mut current = target.load(Ordering::Relaxed);
    loop {
        let next = current.checked_add(delta).ok_or(())?;
        match target.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return Ok(()),
            Err(observed) => current = observed,
        }
    }
}

fn checked_add_usize(target: &AtomicUsize, delta: usize) -> Result<(), ()> {
    let mut current = target.load(Ordering::Relaxed);
    loop {
        let next = current.checked_add(delta).ok_or(())?;
        match target.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return Ok(()),
            Err(observed) => current = observed,
        }
    }
}

impl Stats {
    fn new() -> Self {
        Self {
            seq: AtomicU64::new(0),
            orders_executed: AtomicUsize::new(0),
            quantity_executed: AtomicU64::new(0),
            sum_waiting_time: AtomicU64::new(WAIT_SEED),
            stats_degraded: AtomicBool::new(false),
        }
    }

    fn mark_degraded(&self) {
        let _ =
            self.stats_degraded
                .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed);
    }

    /// Mirror of `record_execution`'s commit / rollback sequence (validation
    /// has no shared-state effect before the guard, so it is elided).
    fn record(&self, quantity: u64, waiting: Option<u64>) -> Result<(), ()> {
        let Some(_write) = Guard::enter(&self.seq) else {
            // Refused entry (issue #165): drop the execution, mark degraded.
            self.mark_degraded();
            return Err(());
        };

        if checked_add_usize(&self.orders_executed, 1).is_err() {
            self.mark_degraded();
            return Err(());
        }
        if checked_add_u64(&self.quantity_executed, quantity).is_err() {
            self.orders_executed.fetch_sub(1, Ordering::Relaxed);
            self.mark_degraded();
            return Err(());
        }
        if let Some(waiting) = waiting
            && checked_add_u64(&self.sum_waiting_time, waiting).is_err()
        {
            self.quantity_executed
                .fetch_sub(quantity, Ordering::Relaxed);
            self.orders_executed.fetch_sub(1, Ordering::Relaxed);
            self.mark_degraded();
            return Err(());
        }
        Ok(())
    }

    /// Mirror of `read_consistent`, bounded to `READ_ATTEMPTS`.
    fn read_consistent(&self) -> Option<Tuple> {
        for _ in 0..READ_ATTEMPTS {
            let s1 = self.seq.load(Ordering::Acquire);
            if s1 & 1 != 0 {
                loom::thread::yield_now();
                continue;
            }
            let data = Tuple {
                orders_executed: self.orders_executed.load(Ordering::Relaxed),
                quantity_executed: self.quantity_executed.load(Ordering::Relaxed),
                sum_waiting_time: self.sum_waiting_time.load(Ordering::Relaxed),
                stats_degraded: self.stats_degraded.load(Ordering::Relaxed),
            };
            fence(Ordering::Acquire);
            let s2 = self.seq.load(Ordering::Relaxed);
            if s1 == s2 {
                return Some(data);
            }
            loom::thread::yield_now();
        }
        None
    }
}

/// State before any record.
const INITIAL: Tuple = Tuple {
    orders_executed: 0,
    quantity_executed: 0,
    sum_waiting_time: WAIT_SEED,
    stats_degraded: false,
};

/// State after the successful record (qty 2, no waiting time).
const AFTER_OK: Tuple = Tuple {
    orders_executed: 1,
    quantity_executed: 2,
    sum_waiting_time: WAIT_SEED,
    stats_degraded: false,
};

/// State after the rolled-back record: aggregates unchanged, flag set.
const AFTER_ROLLBACK: Tuple = Tuple {
    stats_degraded: true,
    ..AFTER_OK
};

/// One writer (a successful record, then a record that overflows at the last
/// counter and rolls back) against one concurrent reader: every accepted copy
/// is one of the three states the statistics held between write sections.
#[test]
fn single_writer_reader_never_sees_partial_tuple() {
    model(4, || {
        let stats = Arc::new(Stats::new());

        let writer = {
            let stats = Arc::clone(&stats);
            loom::thread::spawn(move || {
                assert_eq!(stats.record(2, None), Ok(()));
                assert_eq!(stats.record(3, Some(10)), Err(()));
            })
        };

        let reader = {
            let stats = Arc::clone(&stats);
            loom::thread::spawn(move || stats.read_consistent())
        };

        writer.join().expect("writer panicked");
        if let Some(copy) = reader.join().expect("reader panicked") {
            assert!(
                copy == INITIAL || copy == AFTER_OK || copy == AFTER_ROLLBACK,
                "reader accepted a partial tuple: {copy:?}"
            );
        }

        let final_state = stats.read_consistent();
        assert_eq!(final_state, Some(AFTER_ROLLBACK));
    });
}

/// One writer whose only record rolls back, against two concurrent readers:
/// neither ever accepts the rolled-back prefix.
#[test]
fn single_writer_rollback_invisible_to_two_readers() {
    model(2, || {
        let stats = Arc::new(Stats::new());

        let writer = {
            let stats = Arc::clone(&stats);
            loom::thread::spawn(move || {
                assert_eq!(stats.record(3, Some(10)), Err(()));
            })
        };

        let readers: Vec<_> = (0..2)
            .map(|_| {
                let stats = Arc::clone(&stats);
                loom::thread::spawn(move || stats.read_consistent())
            })
            .collect();

        writer.join().expect("writer panicked");
        let after = Tuple {
            stats_degraded: true,
            ..INITIAL
        };
        for reader in readers {
            if let Some(copy) = reader.join().expect("reader panicked") {
                assert!(
                    copy == INITIAL || copy == after,
                    "reader accepted a rolled-back prefix: {copy:?}"
                );
            }
        }
        assert_eq!(stats.read_consistent(), Some(after));
    });
}

/// Issue #165: the writer's LAST admissible section (sequence seeded at the
/// entry limit's largest even value) exits in range, and the following record
/// is refused without opening a section. A concurrent reader only ever accepts
/// the initial state, the committed record, or that record plus the degraded
/// flag the refusal sets; it never spins on an odd sequence left behind.
#[test]
fn single_writer_exhausted_sequence_refusal_is_coherent() {
    model(4, || {
        let stats = Arc::new(Stats::new());
        stats.seq.store(u64::MAX - 3, Ordering::Relaxed);

        let writer = {
            let stats = Arc::clone(&stats);
            loom::thread::spawn(move || {
                assert_eq!(stats.record(2, None), Ok(()));
                assert_eq!(stats.record(3, None), Err(()));
            })
        };

        let reader = {
            let stats = Arc::clone(&stats);
            loom::thread::spawn(move || stats.read_consistent())
        };

        writer.join().expect("writer panicked");
        if let Some(copy) = reader.join().expect("reader panicked") {
            assert!(
                copy == INITIAL || copy == AFTER_OK || copy == AFTER_ROLLBACK,
                "reader accepted a partial tuple: {copy:?}"
            );
        }
        assert_eq!(stats.seq.load(Ordering::Relaxed), u64::MAX - 1);
        assert_eq!(stats.read_consistent(), Some(AFTER_ROLLBACK));
    });
}

/// UNSUPPORTED schedule, pinned so the documentation stays honest: two
/// overlapping `record_execution` writers and one reader. The sequence guard
/// is not a writer lock, so loom finds the #153 interleaving in which the
/// reader accepts a copy holding one writer's `orders_executed` increment
/// without its `quantity_executed` increment (or vice versa). This test
/// asserts coherence and is EXPECTED to fail it; it does not describe
/// behavior callers may rely on.
#[test]
#[should_panic(expected = "reader accepted a partial tuple")]
fn overlapping_writers_are_unsupported_reader_can_see_partial_tuple() {
    model(3, || {
        let stats = Arc::new(Stats::new());

        let writers: Vec<_> = (0..2)
            .map(|_| {
                let stats = Arc::clone(&stats);
                loom::thread::spawn(move || {
                    assert_eq!(stats.record(2, None), Ok(()));
                })
            })
            .collect();

        let reader = {
            let stats = Arc::clone(&stats);
            loom::thread::spawn(move || stats.read_consistent())
        };

        for writer in writers {
            writer.join().expect("writer panicked");
        }
        if let Some(copy) = reader.join().expect("reader panicked") {
            assert!(
                copy.quantity_executed == 2 * copy.orders_executed as u64,
                "reader accepted a partial tuple: {copy:?}"
            );
        }
    });
}
