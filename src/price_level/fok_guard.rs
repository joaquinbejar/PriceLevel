//! The per-level fill-or-kill guard with a bounded writer hand-off (issue #206).
//!
//! A fill-or-kill match holds the level's reader-writer guard **exclusively**
//! across its dry run and sweep; `add_order`, `update_order` and `snapshot`
//! take its **shared** side (issue #112). `std::sync::RwLock` makes no
//! fairness promise, and a matcher that retakes the exclusive side in a loop
//! wins the race against mutators it has just woken: they are runnable but not
//! yet running when the matcher's next `write()` finds the lock free. Measured
//! on the rejected-FOK loop at depth 10,000 (a ~170 µs exclusive section), a
//! writer's p99 wait was 1.2 to 4.5 s and single waits reached 14.5 s; with
//! the hand-off, p99 is about one section (~190 µs) and the worst single
//! wait measured was under two. See `BENCH.md`.
//!
//! [`FokGuard`] adds one counter, `waiting_mutators`, to that lock:
//!
//! - A mutator first tries the shared side without blocking. Only when that
//!   would block does it **announce** itself (increment the counter), block
//!   in `read()`, and withdraw the announcement once it holds the shared side.
//! - A fill-or-kill match that sees an announced mutator before it requests
//!   the exclusive side **yields** for a bounded budget, until no announced
//!   mutator remains. Each announced mutator then only needs to be scheduled,
//!   not to finish, since the lock is free while the matcher yields; once it
//!   holds the shared side, the matcher's `write()` queues behind it as usual.
//!
//! The counter is a scheduling hint, never a synchronization edge: every
//! happens-before relation the level relies on still comes from the lock
//! itself, so a stale or missed read can cost one more exclusive section of
//! waiting but cannot break exclusion. The matcher never waits while holding
//! the lock, so the hand-off itself introduces no deadlock.
//!
//! # What this does and does not establish
//!
//! The hand-off is a **bounded number of courtesy attempts** with measured
//! latency improvements (`BENCH.md`), not a fairness guarantee:
//!
//! - It does **not** establish starvation freedom for either side. After the
//!   budget the matcher calls `RwLock::write` even if a mutator is still
//!   announced, and Rust leaves `RwLock` acquisition priority unspecified: a
//!   reader-preferring implementation could keep the matcher waiting in
//!   `write()` indefinitely, and a barging one can still delay a mutator
//!   beyond any fixed number of sections. Total lock-acquisition delay, for
//!   mutators and matcher alike, remains scheduler-dependent and unbounded.
//! - The budget counts rounds, not time: 64 `spin_loop` hints and 256
//!   `yield_now` calls. On an idle core a yield returns in about a
//!   microsecond, but on an oversubscribed host each yield can cost a
//!   scheduler time slice, so one hand-off can take hundreds of
//!   milliseconds.
//! - The hand-off changes how many sections a mutator typically waits for,
//!   not how long each lasts: a fill-or-kill that walks a deep level holds
//!   the lock for `O(depth log depth)`.
//!
//! **Typical case, not a guarantee.** With the supported one matcher per
//! level, a writer-preferring or queue-fair lock, and a mutator that is
//! scheduled within the budget, a blocked mutator waits for at most two
//! exclusive sections plus its own wake-up: between its failed `try_read`
//! and its announcement the matcher can finish the section in progress, read
//! a zero counter and take one more; later requests see the announcement.
//! With `k` concurrent fill-or-kill matchers (unsupported), a matcher B
//! queued in `write()` holds readers off on writer-preferring locks (the
//! Linux futex lock, for one) while matcher A spends its budget, so the
//! typical wait grows to about `k` sections.

// Every primitive comes through `fok_sync` so the loom model
// (`tests/loom/fok_handoff.rs`) can compile this very file against loom's
// instrumented equivalents.
use super::fok_sync::{
    AtomicUsize, LockResult, Ordering, RwLock, RwLockReadGuard, RwLockWriteGuard, TryLockError,
    spin_loop, yield_now,
};

/// Busy-wait rounds (`spin_loop` hints) a fill-or-kill match spends on an
/// announced mutator before it starts yielding its time slice (issue #206).
/// A woken mutator that is already on a core acquires the shared side within
/// a few hundred nanoseconds.
const HANDOFF_SPINS: u32 = 64;

/// `yield_now` rounds after [`HANDOFF_SPINS`] before a fill-or-kill match
/// stops waiting for announced mutators and requests the exclusive side
/// anyway (issue #206). This bounds the matcher's extra latency when an
/// announced mutator cannot run (for example, it was preempted).
const HANDOFF_YIELDS: u32 = 256;

#[cfg(test)]
thread_local! {
    /// Test-only override of [`HANDOFF_YIELDS`] for the calling (matcher)
    /// thread.
    static HANDOFF_YIELDS_OVERRIDE: std::cell::Cell<Option<u32>> =
        const { std::cell::Cell::new(None) };
    /// Test-only tally of hand-offs on the calling thread: `(waited,
    /// exhausted)`. `waited` counts write requests that found an announced
    /// mutator; `exhausted` counts those that ran out of budget.
    static HANDOFF_TALLY: std::cell::Cell<(u64, u64)> =
        const { std::cell::Cell::new((0, 0)) };
    /// Test-only count of announcements made on the calling (mutator)
    /// thread.
    static ANNOUNCE_TALLY: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Restores the previous hand-off budget override on drop (test seam).
#[cfg(test)]
pub(crate) struct HandoffYieldsGuard(Option<u32>);

#[cfg(test)]
impl Drop for HandoffYieldsGuard {
    fn drop(&mut self) {
        HANDOFF_YIELDS_OVERRIDE.with(|cell| cell.set(self.0));
    }
}

/// Override the hand-off yield budget on the calling thread (test seam).
#[cfg(test)]
pub(crate) fn override_handoff_yields(yields: u32) -> HandoffYieldsGuard {
    HandoffYieldsGuard(HANDOFF_YIELDS_OVERRIDE.with(|cell| cell.replace(Some(yields))))
}

/// The calling thread's hand-off tally `(waited, exhausted)` (test seam).
#[cfg(test)]
pub(crate) fn handoff_tally() -> (u64, u64) {
    HANDOFF_TALLY.with(std::cell::Cell::get)
}

/// Announcements the calling thread has made (test seam).
#[cfg(test)]
pub(crate) fn announce_tally() -> u64 {
    ANNOUNCE_TALLY.with(std::cell::Cell::get)
}

#[inline]
fn handoff_yields() -> u32 {
    #[cfg(test)]
    if let Some(yields) = HANDOFF_YIELDS_OVERRIDE.with(std::cell::Cell::get) {
        return yields;
    }
    HANDOFF_YIELDS
}

/// The level's fill-or-kill reader-writer guard plus a bounded hand-off to
/// mutators blocked behind a looping fill-or-kill matcher (issue #206). See
/// the module docs for the protocol and what it does not guarantee.
pub(crate) struct FokGuard {
    lock: RwLock<()>,
    /// Mutators blocked on (or about to block on) the shared side. Bounded by
    /// the number of threads blocked in [`Self::read`] at once.
    waiting_mutators: AtomicUsize,
}

impl FokGuard {
    /// An unlocked guard with no announced mutator.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            lock: RwLock::new(()),
            waiting_mutators: AtomicUsize::new(0),
        }
    }

    /// Acquire the shared (mutator) side, announcing this mutator to a looping
    /// fill-or-kill matcher only when the acquisition would block.
    ///
    /// # Errors
    ///
    /// The lock's poison, exactly as [`RwLock::read`] reports it.
    #[inline]
    pub(crate) fn read(&self) -> LockResult<RwLockReadGuard<'_, ()>> {
        match self.lock.try_read() {
            Ok(guard) => Ok(guard),
            Err(TryLockError::Poisoned(poison)) => Err(poison),
            Err(TryLockError::WouldBlock) => self.read_contended(),
        }
    }

    /// Slow path of [`Self::read`]: announce, block, withdraw.
    #[cold]
    #[inline(never)]
    fn read_contended(&self) -> LockResult<RwLockReadGuard<'_, ()>> {
        let announcement = Announcement::new(&self.waiting_mutators);
        let guard = self.lock.read();
        // Withdraw as soon as the shared side is held (or its poison is
        // reported), before the caller's critical section: the matcher waits
        // only for mutators that still need the lock, never for one that is
        // already inside.
        drop(announcement);
        guard
    }

    /// Acquire the exclusive (fill-or-kill) side, first yielding to announced
    /// mutators for a bounded budget.
    ///
    /// # Errors
    ///
    /// The lock's poison, exactly as [`RwLock::write`] reports it.
    #[inline]
    pub(crate) fn write(&self) -> LockResult<RwLockWriteGuard<'_, ()>> {
        self.write_with_budget(HANDOFF_SPINS, handoff_yields())
    }

    /// [`Self::write`] with an explicit hand-off budget: `spins` busy-wait
    /// rounds, then `yields` time-slice yields.
    ///
    /// # Errors
    ///
    /// The lock's poison, exactly as [`RwLock::write`] reports it.
    #[inline]
    pub(crate) fn write_with_budget(
        &self,
        spins: u32,
        yields: u32,
    ) -> LockResult<RwLockWriteGuard<'_, ()>> {
        // The counter is a hint, so any ordering is correct, `Relaxed`
        // included: a missed announcement costs the mutator one more
        // section, never exclusion. SeqCst on the counter alone does not rule
        // out that miss either, because the lock operations it races are
        // only Acquire / Release; it is kept as a cheap choice (the same
        // `ldar` as an Acquire load on aarch64, a plain load on x86_64) that
        // makes the counter's own operations totally ordered.
        if self.waiting_mutators.load(Ordering::SeqCst) != 0 {
            self.hand_off(spins, yields);
        }
        self.lock.write()
    }

    /// Wait, holding no lock, until no mutator is announced or the budget is
    /// spent. Returns `true` when no announced mutator remains, `false` when
    /// the budget ran out first.
    #[cold]
    #[inline(never)]
    fn hand_off(&self, spins: u32, yields: u32) -> bool {
        let drained = self.wait_rounds(spins, spin_loop) || self.wait_rounds(yields, yield_now);
        #[cfg(test)]
        HANDOFF_TALLY.with(|cell| {
            let (waited, exhausted) = cell.get();
            // A tally that would overflow simply stops counting.
            if let (Some(waited), Some(exhausted)) = (
                waited.checked_add(1),
                exhausted.checked_add(u64::from(!drained)),
            ) {
                cell.set((waited, exhausted));
            }
        });
        drained
    }

    /// Up to `rounds` rounds of `pause` followed by a re-check; `true` as
    /// soon as no mutator is announced.
    #[inline]
    fn wait_rounds(&self, rounds: u32, pause: fn()) -> bool {
        (0..rounds).any(|_| {
            pause();
            self.waiting_mutators.load(Ordering::SeqCst) == 0
        })
    }

    /// Announce a mutator that is not blocked on the lock (test seam): the
    /// matcher cannot tell it apart from a mutator that was preempted before
    /// acquiring, so it exercises the hand-off budget.
    #[cfg(test)]
    pub(crate) fn test_announce(&self) -> impl Drop + '_ {
        Announcement::new(&self.waiting_mutators)
    }

    /// Currently announced mutators (test seam).
    #[cfg(test)]
    #[must_use]
    pub(crate) fn test_waiting_mutators(&self) -> usize {
        self.waiting_mutators.load(Ordering::SeqCst)
    }

    /// Whether no shared or exclusive holder exists right now (test seam): a
    /// non-blocking `try_write` that is released at once. `false` too when
    /// the lock is poisoned.
    #[cfg(all(test, not(loom)))]
    #[must_use]
    pub(crate) fn test_is_unheld(&self) -> bool {
        self.lock.try_write().is_ok()
    }
}

/// One announced mutator; withdrawn on drop, including on unwind.
struct Announcement<'a> {
    /// `None` when the counter could not be incremented (it would overflow),
    /// in which case nothing is withdrawn either: the mutator simply waits
    /// without a hand-off, as before issue #206.
    counter: Option<&'a AtomicUsize>,
}

impl<'a> Announcement<'a> {
    #[inline]
    fn new(counter: &'a AtomicUsize) -> Self {
        // Checked rather than `fetch_add`: the count is bounded by the threads
        // blocked at once, but a wrapped counter would read as zero (no
        // hand-off) or as a phantom mutator (a wasted budget), so overflow
        // degrades to no announcement instead. The CAS loop runs only on the
        // contended path.
        let announced = counter
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_add(1))
            .is_ok();
        #[cfg(test)]
        if announced {
            ANNOUNCE_TALLY.with(|cell| {
                if let Some(next) = cell.get().checked_add(1) {
                    cell.set(next);
                }
            });
        }
        Self {
            counter: announced.then_some(counter),
        }
    }
}

impl Drop for Announcement<'_> {
    #[inline]
    fn drop(&mut self) {
        if let Some(counter) = self.counter {
            // Cannot fail: this announcement's own increment is still counted.
            let _ = counter.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1));
        }
    }
}
