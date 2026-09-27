//! loom model of the fill-or-kill guard's bounded writer hand-off (issue #206).
//!
//! Unlike `cancel_match.rs`, this checks the **production code itself**: the
//! guard lives in `src/price_level/fok_guard.rs`, which takes every
//! synchronization primitive from a sibling `fok_sync` module. The library
//! points `fok_sync` at `std`; this file includes `fok_guard.rs` verbatim
//! and points `fok_sync` at loom's instrumented `RwLock`, atomics,
//! `spin_loop` and `yield_now`, so loom explores the real protocol:
//!
//! - a mutator tries the shared side, and only when that would block
//!   announces itself, blocks in `read()` and withdraws once it holds it;
//! - a fill-or-kill matcher that sees an announcement before taking the
//!   exclusive side waits a bounded number of rounds, holding no lock.
//!
//! Checked under every interleaving loom produces:
//!
//! - exclusion: no mutator is inside while the matcher holds the exclusive
//!   side, and vice versa (the wrapper never bypasses the lock);
//! - no deadlock and bounded matcher wait, including with a mutator that
//!   announces and never arrives (loom fails a model that cannot finish);
//! - the announcement count returns to zero;
//! - the hand-off guarantee: when the matcher saw an announced mutator and
//!   the budget did not run out, that mutator's critical section completed
//!   before the matcher's exclusive section began.
//!
//! Run with:
//! ```text
//! RUSTFLAGS="--cfg loom" cargo test --release --test loom_fok_handoff
//! ```
//! Without `--cfg loom` this file compiles to an empty crate.

#![cfg(loom)]

/// loom's primitives under the names `fok_guard.rs` imports.
mod fok_sync {
    pub(crate) use loom::hint::spin_loop;
    pub(crate) use loom::sync::atomic::{AtomicUsize, Ordering};
    pub(crate) use loom::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};
    pub(crate) use loom::thread::yield_now;
    pub(crate) use std::sync::{LockResult, TryLockError};
}

#[allow(dead_code)]
#[path = "../../src/price_level/fok_guard.rs"]
mod fok_guard;

use fok_guard::{FokGuard, handoff_tally};
use loom::sync::Arc;
use loom::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Small budgets keep the state space tractable; the protocol does not
/// depend on their size.
const SPINS: u32 = 1;
const YIELDS: u32 = 2;

struct Level {
    guard: FokGuard,
    /// Mutators inside the shared side.
    shared: AtomicUsize,
    /// `true` while the matcher holds the exclusive side.
    exclusive: AtomicBool,
    /// Set by the mutator inside its critical section.
    mutated: AtomicBool,
}

impl Level {
    fn new() -> Self {
        Self {
            guard: FokGuard::new(),
            shared: AtomicUsize::new(0),
            exclusive: AtomicBool::new(false),
            mutated: AtomicBool::new(false),
        }
    }

    fn mutate(&self) {
        let guard = match self.guard.read() {
            Ok(guard) => guard,
            Err(poison) => poison.into_inner(),
        };
        self.shared.fetch_add(1, Ordering::SeqCst);
        assert!(
            !self.exclusive.load(Ordering::SeqCst),
            "mutator inside a FOK"
        );
        self.mutated.store(true, Ordering::SeqCst);
        self.shared.fetch_sub(1, Ordering::SeqCst);
        drop(guard);
    }

    /// One fill-or-kill acquisition. Returns whether the mutator had already
    /// completed when the exclusive section began, and the hand-off outcome
    /// of this call `(waited, exhausted)`.
    fn fill_or_kill(&self) -> (bool, (u64, u64)) {
        let (waited, exhausted) = handoff_tally();
        let guard = match self.guard.write_with_budget(SPINS, YIELDS) {
            Ok(guard) => guard,
            Err(poison) => poison.into_inner(),
        };
        let outcome = handoff_tally();
        self.exclusive.store(true, Ordering::SeqCst);
        assert_eq!(
            self.shared.load(Ordering::SeqCst),
            0,
            "FOK beside a mutator"
        );
        let mutated = self.mutated.load(Ordering::SeqCst);
        self.exclusive.store(false, Ordering::SeqCst);
        drop(guard);
        (mutated, (outcome.0 - waited, outcome.1 - exhausted))
    }
}

#[test]
fn fok_handoff_admits_announced_mutator_first() {
    loom::model(|| {
        let level = Arc::new(Level::new());
        let mutator = {
            let level = Arc::clone(&level);
            loom::thread::spawn(move || level.mutate())
        };
        // Two back-to-back acquisitions: the barging pattern of issue #206.
        for _ in 0..2 {
            let (mutated, (waited, exhausted)) = level.fill_or_kill();
            if waited == 1 && exhausted == 0 {
                assert!(
                    mutated,
                    "hand-off drained, yet the announced mutator had not run first"
                );
            }
        }
        mutator.join().expect("mutator");
        assert!(level.mutated.load(Ordering::SeqCst));
        assert_eq!(level.guard.test_waiting_mutators(), 0);
    });
}

#[test]
fn fok_handoff_bounded_by_budget_when_mutator_never_arrives() {
    loom::model(|| {
        let level = Arc::new(Level::new());
        // A phantom announcement: indistinguishable from a mutator that was
        // preempted before it could acquire.
        let phantom = level.guard.test_announce();
        let mutator = {
            let level = Arc::clone(&level);
            loom::thread::spawn(move || level.mutate())
        };
        let (_, (waited, exhausted)) = level.fill_or_kill();
        assert_eq!(
            (waited, exhausted),
            (1, 1),
            "the budget ran out, then the FOK ran"
        );
        mutator.join().expect("mutator");
        drop(phantom);
        assert_eq!(level.guard.test_waiting_mutators(), 0);
    });
}
