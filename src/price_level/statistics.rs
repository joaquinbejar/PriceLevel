use crate::errors::{ExhaustedCounter, PriceLevelError};
use crate::utils::text::{Fields, split_exactly_once};
use crate::utils::{TimestampMs, UnixClock};
use portable_atomic::AtomicU128;
use serde::de::{self, MapAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

/// Tracks performance statistics for a price level.
///
/// All counters are private atomics so that no external consumer can
/// `.store()` / `.fetch_add()` directly and desync them from the order queue
/// or from the checked-arithmetic invariants enforced in
/// [`record_execution`](Self::record_execution). Mutation happens only through
/// the `record_*` / [`reset`](Self::reset) methods; reads happen only through
/// the public accessors.
///
/// # Atomic ordering
///
/// The individual counters are `Relaxed` observability atomics: nothing in the
/// engine reads a statistic to gate a queue mutation, and no other field's
/// visibility is published through them. Point accessors ([`orders_executed`](Self::orders_executed),
/// the average ratios, …) load them `Relaxed` and remain best-effort — a ratio
/// across two counters can still be transiently torn and self-corrects once
/// recording quiesces.
///
/// A **multi-field** read — [`Clone`] (which backs the checksummed
/// `PriceLevel::snapshot`) and the serde / [`Display`](std::fmt::Display)
/// serialization paths — instead goes through a **seqlock** (issue #129) so it
/// copies a consistent set that never mixes a pre- and post-`record_execution`
/// prefix (which would otherwise checksum a state the level never held). The
/// `stats_seq` sequence counter is bumped to odd on a writer's entry
/// ([`record_execution`](Self::record_execution) / [`reset`](Self::reset)) and
/// back to even on its exit, both `Release`; a reader loads it `Acquire`, copies
/// the fields, `Acquire`-fences, re-loads it, and retries if it changed or was
/// odd. The lone read-modify-write loops in `checked_fetch_add_u64`
/// and `checked_fetch_add_u128` are standard
/// `compare_exchange_weak` CAS retries.
///
/// # Writer contract (issue #153)
///
/// The statistics support **exactly one concurrent writer** of the execution
/// aggregates at a time:
///
/// - [`record_execution`](Self::record_execution) is driven by the single
///   logical matcher of the level (`PriceLevel::match_order`; see "Concurrency
///   Model" in the crate docs). Two `record_execution` calls overlapping in
///   time on the same instance are **unsupported**.
/// - [`reset`](Self::reset) / [`reset_at`](Self::reset_at) require
///   **quiescence**: no `record_execution` (hence no `match_order`) in flight.
///
/// The sequence guard is a publication protocol for **readers**, not a writer
/// lock: entering a write section is a plain increment, not an exclusive
/// acquire, so it neither serializes two writers nor excludes `reset` from a
/// `record_execution`. With two overlapping writers the sequence can pass
/// through an even value while the first writer's transaction is still
/// half-applied, and a reader can accept that partial tuple. The crate does
/// not detect this schedule; it is outside the contract rather than protected.
/// Concurrent recorders still leave arithmetically correct **final** totals
/// (every counter update is an atomic checked RMW and a rollback subtracts
/// exactly what its own call added), but [`Clone`], serialization and
/// [`Display`](std::fmt::Display) are only guaranteed coherent under the
/// single-writer contract.
///
/// Under that contract, a multi-field reader running concurrently with the
/// writer, from any number of threads, returns a state the statistics actually
/// held between two write sections: every execution recorded before it began
/// is fully present or fully absent, never a partial prefix, and an overflow
/// rollback is never observed. [`record_order_added`](Self::record_order_added)
/// and [`record_order_removed`](Self::record_order_removed) are single-counter
/// increments outside the write section; they may be called from any number of
/// threads (admissions and cancels run concurrently with the matcher), and a
/// multi-field read sees each of those counters at some value it held during
/// the read. The single-field accessors and the average ratios are `Relaxed`
/// point reads with no cross-field guarantee.
///
/// A reader retries while a write section is open, so a writer descheduled
/// inside its section delays readers (they spin) until it resumes. The
/// section is short and finite; a successful record is allocation-free, but a
/// rejected one (a maker timestamp in the future of execution, or a
/// multiplication / counter overflow) formats its error while the section is
/// still open, so readers also wait on that formatting and allocator work. A
/// panicking writer closes the section through the guard's `Drop`.
///
/// # Counter exhaustion (issue #165)
///
/// No counter here wraps. The additive aggregates are checked RMWs (see
/// [`record_execution`](Self::record_execution)); `orders_added` /
/// `orders_removed` are checked too:
/// [`record_order_added`](Self::record_order_added) and
/// [`record_order_removed`](Self::record_order_removed) refuse to move a
/// counter already at `usize::MAX`, leave it there, set the sticky
/// [`stats_degraded`](Self::stats_degraded) flag and return
/// [`PriceLevelError::CounterExhausted`]. They run from any thread, outside the
/// seqlock write section, so the exhaustion path uses only multi-writer-safe
/// operations (a CAS loop on the counter, a CAS on the flag) and takes no part
/// in the single-writer protocol. The engine records them after its queue
/// mutation has committed; the mutation stands, and the flag is the typed
/// signal that the counters under-count.
///
/// The seqlock sequence is 64 bits and is never reused (a reader that saw an
/// old value again could accept a torn copy). A writer reserves its whole
/// section on entry: it opens only if the even sequence `s` satisfies
/// `s <= u64::MAX - 2`, so the exit increment to `s + 2` is always in range
/// and `Drop` can neither fail nor wrap. A refused entry mutates nothing but
/// the degraded flag: [`record_execution`](Self::record_execution) drops the
/// execution all-or-nothing, marks the statistics degraded and returns
/// [`PriceLevelError::CounterExhausted`];
/// [`reset_at`](Self::reset_at) / [`reset`](Self::reset) return the same
/// error and change nothing. A single flag store is a one-field change, so a
/// concurrent multi-field reader still copies a state the statistics held.
/// Recovery is a rebuild: [`Clone`] and every decode path start a fresh
/// sequence at zero (so does `PriceLevel::from_snapshot`).
///
/// # `value_executed` width (issue #140)
///
/// `value_executed` accumulates `quantity * price`, the same product that
/// [`MatchResult::executed_value`](crate::execution::MatchResult::executed_value)
/// and [`Trade::total_value`](crate::execution::Trade::total_value) return as
/// `u128`. It is stored in a `u128` so that fixed-point callers, whose product
/// carries the scale of both operands, do not exhaust the accumulator under
/// ordinary volume. The ceiling is raised, not removed: a `u128` overflow is
/// still rejected all-or-nothing and marks the statistics degraded.
///
/// The accumulator is a [`portable_atomic::AtomicU128`] because `std`'s
/// `AtomicU128` is not stable. It is lock-free where the CPU provides a native
/// 128-bit CAS: aarch64, and x86_64 with `cmpxchg16b` (detected at run time
/// unless enabled at compile time). On a target without one, `portable-atomic`
/// falls back to a global lock for this single counter; the other counters and
/// the order queue are unaffected.
///
/// # Layout (issue #154)
///
/// On 64-bit targets the fields form one unpadded 96-byte block (16-byte
/// aligned), so the producer counters `orders_added` / `orders_removed` usually
/// share a cache line with the matcher's execution aggregates and `stats_seq`.
/// That false sharing was measured and deliberately kept: separating the
/// groups onto their own 128-byte lines removed it from a bare statistics
/// object but showed no consistent or demonstrated repeatable p99 / p99.9
/// benefit on a shared level, while raising
/// the per-level allocation from 112 to 384 bytes. The data and method are in
/// `BENCH.md`, "Statistics cache contention".
#[derive(Debug)]
pub struct PriceLevelStatistics {
    /// Number of orders added
    orders_added: AtomicUsize,

    /// Number of orders removed
    orders_removed: AtomicUsize,

    /// Number of orders executed
    orders_executed: AtomicUsize,

    /// Total quantity executed
    quantity_executed: AtomicU64,

    /// Total value executed (`sum(quantity * price)`), `u128` (issue #140).
    value_executed: AtomicU128,

    /// Last execution timestamp
    last_execution_time: AtomicU64,

    /// Statistics initialization timestamp (set at construction / reset).
    /// Not updated on order arrival — see `first_arrival_time()`.
    first_arrival_time: AtomicU64,

    /// Sum of waiting times for orders
    sum_waiting_time: AtomicU64,

    /// Sticky flag: set once and never cleared (except by [`reset`](Self::reset))
    /// when an execution's statistics contribution was **dropped** — a
    /// [`record_execution`](Self::record_execution) that failed validation or
    /// overflowed a counter and so contributed to NONE of the aggregates
    /// (all-or-nothing, issue #117). The trade itself is unaffected; this flag
    /// is the observable signal that the recorded aggregates under-count the
    /// true executions. Serialized so it round-trips through a snapshot.
    stats_degraded: AtomicBool,

    /// Seqlock sequence for consistent multi-field reads (issue #129). Even when
    /// no writer is in a section, odd while a writer ([`record_execution`](Self::record_execution)
    /// / [`reset`](Self::reset)) is mutating. Purely internal — never serialized
    /// — so a restored / cloned value starts even (0).
    stats_seq: AtomicU64,
}

/// RAII guard bracketing a statistics WRITE section for the seqlock (issue
/// #129). Constructing it bumps `stats_seq` to odd; dropping it bumps back to
/// even, so a concurrent multi-field reader retries if it overlapped either
/// increment. Using a guard keeps the section correct across the early returns
/// in [`PriceLevelStatistics::record_execution`].
///
/// Entry is a checked increment, not an exclusive acquire: the guard assumes
/// the single-writer contract (issue #153) and does not serialize two
/// overlapping writers.
///
/// # Exhaustion (issue #165)
///
/// Entry reserves the whole section: it refuses to open unless the sequence
/// is at most [`STATS_SEQ_ENTRY_LIMIT`], so the entry value `s + 1` and the
/// exit value `s + 2` both stay at or below [`STATS_SEQ_CEILING`]. Under the
/// single-writer contract nothing else moves the sequence while the section
/// is open, so the exit increment is proven in range and `Drop` never fails
/// or wraps.
///
/// # No permanently odd sequence (pre-release hardening)
///
/// Every transition, entry or exit, is refused if it would move the sequence
/// above [`STATS_SEQ_CEILING`] (`u64::MAX - 1`, even). The only odd value an
/// exit could be stranded on is `u64::MAX`, which is therefore unreachable.
/// With overlapping writers (a contract violation) an exit can be refused,
/// but only when the sequence already sits at the even ceiling, where it then
/// stays (no entry opens above [`STATS_SEQ_ENTRY_LIMIT`]); otherwise every
/// entry is matched by an exit and the quiescent sequence is even. Either
/// way [`PriceLevelStatistics::read_consistent`] cannot spin forever once
/// writers stop.
struct WriteSeqGuard<'a> {
    seq: &'a AtomicU64,
}

/// Largest value the sequence may ever take (pre-release hardening). It is
/// even, and `u64::MAX` (odd) is never reached, so a refused exit can never
/// leave the sequence permanently odd.
const STATS_SEQ_CEILING: u64 = u64::MAX - 1;

/// Largest sequence value from which a write section may open (issue #165):
/// entry moves it to at most `u64::MAX - 2` and exit to at most
/// [`STATS_SEQ_CEILING`]. For the even values a single writer starts from
/// this admits exactly what the former `u64::MAX - 2` limit did.
const STATS_SEQ_ENTRY_LIMIT: u64 = u64::MAX - 3;

/// The checked, ceiling-bounded `+1` shared by entry and exit.
#[inline]
fn seq_step(s: u64) -> Option<u64> {
    s.checked_add(1).filter(|next| *next <= STATS_SEQ_CEILING)
}

impl<'a> WriteSeqGuard<'a> {
    /// Opens a write section, or returns `Err` with the sequence untouched
    /// when it has no headroom for both the entry and the exit increment.
    #[inline]
    fn try_new(seq: &'a AtomicU64) -> Result<Self, PriceLevelError> {
        // Enter: even -> odd. A `Relaxed` checked RMW (a CAS loop; one
        // iteration under the single-writer contract) plus a `Release` fence
        // so the field writes that follow cannot be reordered before the odd
        // marker a reader watches for. Same ordering as the previous
        // `fetch_add(1, Relaxed)`; only the range check is new.
        if seq
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |s| {
                if s <= STATS_SEQ_ENTRY_LIMIT {
                    seq_step(s)
                } else {
                    None
                }
            })
            .is_err()
        {
            return Err(PriceLevelError::counter_exhausted(
                ExhaustedCounter::StatisticsSequence,
            ));
        }
        std::sync::atomic::fence(Ordering::Release);
        Ok(Self { seq })
    }
}

impl Drop for WriteSeqGuard<'_> {
    #[inline]
    fn drop(&mut self) {
        // Exit: odd -> even, `Release` so every field write in the section
        // happens-before a reader's `Acquire` load of the (now even) sequence.
        // Proven in range: entry admitted `s <= u64::MAX - 3`, so the value
        // here is at most `u64::MAX - 2` under the single-writer contract. The
        // increment is still checked against the even ceiling, so a contract
        // violation (an overlapping writer) can neither wrap the sequence nor
        // strand it on the odd `u64::MAX`: a refused exit happens only at the
        // even ceiling, where the sequence then stays.
        let _ = self
            .seq
            .fetch_update(Ordering::Release, Ordering::Relaxed, seq_step);
    }
}

/// An order-event statistic that could not be recorded (issue #165).
#[derive(Debug)]
pub(crate) struct OrderEventDrop {
    /// The typed exhaustion error.
    pub(crate) error: PriceLevelError,
    /// `true` only for the single call whose degraded-flag CAS moved it
    /// `false -> true`; every concurrent or later drop sees `false`.
    pub(crate) degraded_now: bool,
}

/// A consistent point-in-time copy of every statistics field, read under the
/// seqlock (issue #129). Plain values, no atomics — so `Clone` / serialize
/// materialize a coherent set rather than a torn mix of counters.
#[derive(Clone, Copy)]
struct StatsData {
    orders_added: usize,
    orders_removed: usize,
    orders_executed: usize,
    quantity_executed: u64,
    value_executed: u128,
    last_execution_time: u64,
    first_arrival_time: u64,
    sum_waiting_time: u64,
    stats_degraded: bool,
}

impl PriceLevelStatistics {
    fn checked_fetch_add_u64(
        target: &AtomicU64,
        value: u64,
        field: &str,
    ) -> Result<(), PriceLevelError> {
        // `Relaxed`: this is an advisory observability counter (see the
        // struct-level "Atomic ordering" note) — no happens-before rides on it.
        let mut current = target.load(Ordering::Relaxed);

        loop {
            let next =
                current
                    .checked_add(value)
                    .ok_or_else(|| PriceLevelError::InvalidOperation {
                        message: format!("{field} overflow"),
                    })?;

            // Standard lock-free CAS retry. Both the success and failure
            // orderings are `Relaxed`: the only invariant is that the stored
            // value is a checked sum of monotonic increments (the loop re-reads
            // `observed` and re-validates on contention). The counter publishes
            // nothing to another thread, so neither acquire on failure nor
            // release on success is needed. The retry body is allocation-free,
            // per the tight-CAS-loop rule.
            match target.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => return Ok(()),
                Err(observed) => current = observed,
            }
        }
    }

    /// Checked `+= value` on the `u128` value accumulator, mirroring
    /// [`checked_fetch_add_u64`](Self::checked_fetch_add_u64) (issue #140).
    #[inline]
    fn checked_fetch_add_u128(
        target: &AtomicU128,
        value: u128,
        field: &str,
    ) -> Result<(), PriceLevelError> {
        // `Relaxed`, for the same reasons as `checked_fetch_add_u64`: an advisory
        // observability counter that publishes nothing to another thread.
        let mut current = target.load(Ordering::Relaxed);
        loop {
            let next =
                current
                    .checked_add(value)
                    .ok_or_else(|| PriceLevelError::InvalidOperation {
                        message: format!("{field} overflow"),
                    })?;
            match target.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => return Ok(()),
                Err(observed) => current = observed,
            }
        }
    }

    /// Checked `+= value` on a `usize` counter, mirroring
    /// [`checked_fetch_add_u64`](Self::checked_fetch_add_u64). `orders_executed`
    /// can be seeded to `usize::MAX` through `FromStr` / serde, so a plain
    /// `fetch_add(1)` could wrap while the other aggregates advance (issue #129);
    /// this rejects the overflow so the all-or-nothing rollback can undo the
    /// prefix instead.
    fn checked_fetch_add_usize(
        target: &AtomicUsize,
        value: usize,
        field: &str,
    ) -> Result<(), PriceLevelError> {
        let mut current = target.load(Ordering::Relaxed);
        loop {
            let next =
                current
                    .checked_add(value)
                    .ok_or_else(|| PriceLevelError::InvalidOperation {
                        message: format!("{field} overflow"),
                    })?;
            match target.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => return Ok(()),
                Err(observed) => current = observed,
            }
        }
    }

    /// Checked rollback `-= value` on a `usize` counter (pre-release
    /// hardening; replaces a wrapping `fetch_sub`). Returns `false`, leaving
    /// the counter unchanged, if it holds less than `value`.
    #[inline]
    fn rollback_usize(target: &AtomicUsize, value: usize) -> bool {
        target
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |c| {
                c.checked_sub(value)
            })
            .is_ok()
    }

    /// As [`rollback_usize`](Self::rollback_usize), for a `u64` counter.
    #[inline]
    fn rollback_u64(target: &AtomicU64, value: u64) -> bool {
        target
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |c| {
                c.checked_sub(value)
            })
            .is_ok()
    }

    /// As [`rollback_usize`](Self::rollback_usize), for the `u128` value
    /// accumulator.
    #[inline]
    fn rollback_u128(target: &AtomicU128, value: u128) -> bool {
        target
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |c| {
                c.checked_sub(value)
            })
            .is_ok()
    }

    /// Passes `err` through, logging at ERROR first when a rollback was
    /// refused. Called after the seqlock write section has closed, so the
    /// subscriber never runs while readers spin on an odd sequence.
    #[inline]
    fn after_rollback(err: PriceLevelError, intact: bool) -> PriceLevelError {
        if !intact {
            Self::rollback_refused(&err);
        }
        err
    }

    /// ERROR report for a refused rollback: the statistics are already marked
    /// degraded, and a counter may keep part of the dropped execution.
    #[cold]
    #[inline(never)]
    fn rollback_refused(err: &PriceLevelError) {
        tracing::error!(
            error = %err,
            "statistics rollback refused: a counter held less than this record \
             added (invariant already broken); statistics marked degraded"
        );
    }

    /// Set the sticky degraded flag; returns `true` iff THIS call transitioned it
    /// `false -> true` (issue #129). The caller (`PriceLevel::match_order`) logs
    /// the WARN only on that transition, so a burst of dropped executions marks
    /// the level degraded once and logs once, not once per drop.
    pub(crate) fn mark_degraded(&self) -> bool {
        self.stats_degraded
            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    }

    /// Read a consistent snapshot of every field under the seqlock (issue #129).
    ///
    /// Retries until a full copy brackets an even, unchanged sequence — i.e. no
    /// writer transaction ([`record_execution`](Self::record_execution) /
    /// [`reset`](Self::reset)) overlapped it, so under the single-writer
    /// contract (issue #153) the copy is NEVER a torn mix of a pre- and
    /// post-write prefix (a bounded fallback that returned a torn copy would
    /// defeat the checksummed snapshot this backs). With two overlapping writers
    /// (unsupported) an even, unchanged sequence no longer implies that no
    /// transaction was in flight, and this guarantee does not hold.
    ///
    /// # Liveness
    ///
    /// The loop's work PER attempt is bounded (one sequence load + a nine-field
    /// copy), and it converges under the writer contract (one matcher per level
    /// and a quiescent `reset`): a writer holds the section for only a short,
    /// allocation-free burst before dropping the guard back to even, and
    /// `record_execution` is finite, so a reader exits on the first iteration
    /// when uncontended and otherwise as soon as recording quiesces (which it
    /// always does — the matcher cannot record forever). A writer descheduled
    /// inside its section keeps readers spinning until it resumes. A panicking
    /// writer still restores the even sequence via the guard's `Drop`, so the
    /// reader is never stranded on a permanently-odd sequence; nor is it after
    /// overlapping writers exhaust the sequence, because no transition may
    /// reach the odd `u64::MAX` (see `WriteSeqGuard`).
    fn read_consistent(&self) -> StatsData {
        loop {
            let s1 = self.stats_seq.load(Ordering::Acquire);
            if s1 & 1 != 0 {
                // A writer is mid-section; spin until it exits.
                std::hint::spin_loop();
                continue;
            }
            let data = StatsData {
                orders_added: self.orders_added.load(Ordering::Relaxed),
                orders_removed: self.orders_removed.load(Ordering::Relaxed),
                orders_executed: self.orders_executed.load(Ordering::Relaxed),
                quantity_executed: self.quantity_executed.load(Ordering::Relaxed),
                value_executed: self.value_executed.load(Ordering::Relaxed),
                last_execution_time: self.last_execution_time.load(Ordering::Relaxed),
                first_arrival_time: self.first_arrival_time.load(Ordering::Relaxed),
                sum_waiting_time: self.sum_waiting_time.load(Ordering::Relaxed),
                stats_degraded: self.stats_degraded.load(Ordering::Relaxed),
            };
            // Ensure the field loads complete before re-reading the sequence.
            std::sync::atomic::fence(Ordering::Acquire);
            let s2 = self.stats_seq.load(Ordering::Relaxed);
            if s1 == s2 {
                return data;
            }
            std::hint::spin_loop();
        }
    }

    /// Reconstruct from a plain [`StatsData`] copy (seqlock reader output), with
    /// a fresh even sequence.
    fn from_data(data: StatsData) -> Self {
        Self {
            orders_added: AtomicUsize::new(data.orders_added),
            orders_removed: AtomicUsize::new(data.orders_removed),
            orders_executed: AtomicUsize::new(data.orders_executed),
            quantity_executed: AtomicU64::new(data.quantity_executed),
            value_executed: AtomicU128::new(data.value_executed),
            last_execution_time: AtomicU64::new(data.last_execution_time),
            first_arrival_time: AtomicU64::new(data.first_arrival_time),
            sum_waiting_time: AtomicU64::new(data.sum_waiting_time),
            stats_degraded: AtomicBool::new(data.stats_degraded),
            stats_seq: AtomicU64::new(0),
        }
    }

    /// Creates empty statistics whose start time is **unstamped**
    /// (`first_arrival_time() == 0`).
    ///
    /// This constructor is deterministic and reads no clock (issue #171): two
    /// levels built from the same input produce byte-identical snapshots, and
    /// no clock failure can be hidden behind an infallible constructor. Use
    /// [`Self::new_at`] with a known time, or [`Self::try_new`] with a
    /// caller-supplied [`UnixClock`], to record when tracking began.
    #[must_use]
    pub fn new() -> Self {
        Self::new_at(TimestampMs::ZERO)
    }

    /// Creates empty statistics whose start time
    /// ([`first_arrival_time`](Self::first_arrival_time)) is the caller-supplied
    /// `started_at`. Infallible and clock-free.
    #[must_use]
    pub fn new_at(started_at: TimestampMs) -> Self {
        Self {
            orders_added: AtomicUsize::new(0),
            orders_removed: AtomicUsize::new(0),
            orders_executed: AtomicUsize::new(0),
            quantity_executed: AtomicU64::new(0),
            value_executed: AtomicU128::new(0),
            last_execution_time: AtomicU64::new(0),
            first_arrival_time: AtomicU64::new(started_at.as_u64()),
            sum_waiting_time: AtomicU64::new(0),
            stats_degraded: AtomicBool::new(false),
            stats_seq: AtomicU64::new(0),
        }
    }

    /// Creates empty statistics stamped with the current time read once from
    /// a caller-supplied [`UnixClock`].
    ///
    /// # Errors
    ///
    /// Returns the clock's error unchanged; no fallback start time is
    /// substituted.
    pub fn try_new<C>(clock: &C) -> Result<Self, PriceLevelError>
    where
        C: UnixClock + ?Sized,
    {
        Ok(Self::new_at(clock.try_now_ms()?))
    }

    /// Record a new order being added.
    ///
    /// A single checked `Relaxed` increment outside the seqlock write section;
    /// safe to call from any number of threads concurrently with the matcher
    /// (see the struct-level "Writer contract").
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::CounterExhausted`] (counter
    /// [`ExhaustedCounter::OrdersAdded`]) if `orders_added` is already
    /// `usize::MAX` (issue #165). The counter keeps that value instead of
    /// wrapping to zero, and the sticky [`stats_degraded`](Self::stats_degraded)
    /// flag is set because the count now under-counts admissions.
    pub fn record_order_added(&self) -> Result<(), PriceLevelError> {
        self.record_order_event(&self.orders_added, ExhaustedCounter::OrdersAdded)
            .map_err(|drop| drop.error)
    }

    /// Record an order being removed without execution.
    ///
    /// A single checked `Relaxed` increment outside the seqlock write section;
    /// safe to call from any number of threads concurrently with the matcher
    /// (see the struct-level "Writer contract").
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::CounterExhausted`] (counter
    /// [`ExhaustedCounter::OrdersRemoved`]) if `orders_removed` is already
    /// `usize::MAX` (issue #165). The counter keeps that value instead of
    /// wrapping to zero, and the sticky [`stats_degraded`](Self::stats_degraded)
    /// flag is set because the count now under-counts removals.
    pub fn record_order_removed(&self) -> Result<(), PriceLevelError> {
        self.record_order_event(&self.orders_removed, ExhaustedCounter::OrdersRemoved)
            .map_err(|drop| drop.error)
    }

    /// [`record_order_added`](Self::record_order_added) that also reports
    /// whether THIS call is the one that set the degraded flag (issue #165).
    /// Used by the engine to log a drop exactly once across threads.
    #[inline]
    pub(crate) fn record_order_added_reporting(&self) -> Result<(), OrderEventDrop> {
        self.record_order_event(&self.orders_added, ExhaustedCounter::OrdersAdded)
    }

    /// [`record_order_removed`](Self::record_order_removed) that also reports
    /// whether THIS call is the one that set the degraded flag (issue #165).
    #[inline]
    pub(crate) fn record_order_removed_reporting(&self) -> Result<(), OrderEventDrop> {
        self.record_order_event(&self.orders_removed, ExhaustedCounter::OrdersRemoved)
    }

    /// Checked `+= 1` on an order-event counter (issue #165). Multi-writer
    /// safe: a `Relaxed` CAS loop that refuses to pass `usize::MAX`, then, on
    /// refusal, a CAS on the degraded flag whose outcome identifies the one
    /// call that transitioned it. Allocation-free on both paths.
    #[inline]
    fn record_order_event(
        &self,
        counter: &AtomicUsize,
        kind: ExhaustedCounter,
    ) -> Result<(), OrderEventDrop> {
        if counter
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |c| c.checked_add(1))
            .is_ok()
        {
            return Ok(());
        }
        let degraded_now = self.mark_degraded();
        Err(OrderEventDrop {
            error: PriceLevelError::counter_exhausted(kind),
            degraded_now,
        })
    }

    /// Record an order execution.
    ///
    /// The `execution_timestamp` is the taker timestamp threaded in from the
    /// caller (the same value stamped onto the emitted [`Trade`]s). It is used
    /// both as the level's `last_execution_time` and as the reference time for
    /// the per-maker waiting-time accumulation. This keeps the match path
    /// clock-free and deterministic: no wall-clock read happens during a match.
    ///
    /// [`Trade`]: crate::execution::Trade
    ///
    /// # All-or-nothing (issue #117)
    ///
    /// Under the writer contract below, an accepted execution contributes to
    /// **every** aggregate, or to **none**. If a later counter overflows after
    /// earlier ones already advanced, this rolls the committed prefix back (a
    /// checked subtraction of exactly what this call added, never below zero
    /// because those units are still present). So a caller that honours the
    /// contract never observes a partial contribution in the final state.
    /// Without the contract the guarantee does not hold: a rollback refused
    /// because an overlapping [`reset`](Self::reset) already zeroed the
    /// counter leaves that counter unchanged, is logged at ERROR, and can
    /// leave part of the prefix behind (see the writer contract). On any
    /// failure — a validation error or a counter overflow — the sticky
    /// [`stats_degraded`](Self::stats_degraded) flag is set: the dropped
    /// execution is then observable, even though the caller
    /// (`PriceLevel::match_order`) cannot fail the already-committed trade.
    ///
    /// # Writer contract (issue #153)
    ///
    /// At most one `record_execution` may be in flight per instance, and none
    /// may overlap a [`reset`](Self::reset) / [`reset_at`](Self::reset_at). The
    /// engine satisfies this through its single logical matcher per level;
    /// direct callers of this public method must serialize their calls
    /// themselves. The whole call runs inside the seqlock write section, so
    /// under this contract a concurrent [`Clone`], serialization or
    /// [`Display`](std::fmt::Display) sees the execution fully applied or not
    /// at all, and never a prefix that is later rolled back. The `Relaxed`
    /// single-field accessors are outside that protocol and may glimpse the
    /// prefix transiently.
    ///
    /// Overlapping calls are **unsupported**. They still produce correct final
    /// totals (each update is an atomic checked RMW, and a rollback subtracts
    /// exactly what its own call added, so it commutes with the other call's
    /// deltas), but the sequence guard does not protect a concurrent
    /// multi-field reader from them: it can accept a partial tuple. An overlap
    /// with `reset` is worse: a `store(0)` landing between a committed prefix
    /// and its rollback leaves part of the prefix behind (the checked rollback
    /// is refused rather than wrapping the counter). Neither is prevented by
    /// the guard; both are caller contract violations.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::InvalidOperation`] if any of the counter
    /// accumulations overflow (`value_executed` is a `u128` accumulator, issue
    /// #140), if the value (`quantity * price`) overflows `u128`, or if
    /// `order_timestamp` is strictly greater than `execution_timestamp` (a
    /// maker arriving in the future of execution). Returns
    /// [`PriceLevelError::CounterExhausted`] (counter
    /// [`ExhaustedCounter::StatisticsSequence`]) if the seqlock sequence has
    /// no headroom left for another write section (issue #165). Under the
    /// writer contract every failure leaves the aggregates untouched; every
    /// failure sets the degraded flag.
    pub fn record_execution(
        &self,
        quantity: u64,
        price: u128,
        order_timestamp: u64,
        execution_timestamp: u64,
    ) -> Result<(), PriceLevelError> {
        let current_time = execution_timestamp;

        // Bracket the whole record as a seqlock WRITE (issue #129): a concurrent
        // multi-field reader (`Clone` / serialize) retries rather than capture an
        // in-flight prefix or a later-rolled-back one. The guard is NOT a writer
        // lock: it does not exclude a second `record_execution` or a `reset`;
        // the single-writer contract (issue #153) does. The guard's `Drop`
        // closes the section (back to even) on EVERY return path below,
        // including the early validation errors.
        //
        // An exhausted sequence (issue #165) refuses the section before any
        // counter moves: the execution is dropped all-or-nothing like any
        // other rejected record, and the degraded flag makes the drop visible.
        let write_section = match WriteSeqGuard::try_new(&self.stats_seq) {
            Ok(guard) => guard,
            Err(err) => {
                self.mark_degraded();
                return Err(err);
            }
        };

        // Validate everything that can fail BEFORE mutating any counter, so a
        // rejected record leaves the statistics untouched. Any failure marks the
        // stats degraded: this execution's contribution is being dropped.
        let waiting_time = if order_timestamp > 0 {
            match current_time.checked_sub(order_timestamp) {
                Some(value) => Some(value),
                None => {
                    self.mark_degraded();
                    return Err(PriceLevelError::InvalidOperation {
                        message: format!(
                            "order timestamp {order_timestamp} is in the future of current time {current_time}"
                        ),
                    });
                }
            }
        } else {
            None
        };

        // `quantity * price` in `u128`, stored at full width (issue #140): the
        // multiplication itself can only overflow when `price` is within a
        // factor of `quantity` of `u128::MAX`.
        let value = match u128::from(quantity).checked_mul(price) {
            Some(value) => value,
            None => {
                self.mark_degraded();
                return Err(PriceLevelError::InvalidOperation {
                    message: "value_executed overflow (quantity * price exceeds u128)".to_string(),
                });
            }
        };

        // Commit the additive aggregates with a rollback of the already-committed
        // prefix on a later overflow (all-or-nothing). `orders_executed` is a
        // `usize` counter that could be seeded to `usize::MAX` via FromStr / serde
        // (issue #129), so it is a CHECKED add and part of the transaction —
        // rolled back like the others. `last_execution_time` is a non-additive
        // "latest" store, applied only after every additive commit succeeds so a
        // rejected record never advances it.
        if let Err(err) = Self::checked_fetch_add_usize(&self.orders_executed, 1, "orders_executed")
        {
            self.mark_degraded();
            return Err(err);
        }

        // Rollbacks are checked subtractions (pre-release hardening): each
        // undoes units this call just added, so a refusal is only possible if
        // an invariant is already broken (e.g. a `reset` overlapping this
        // record, a writer-contract violation). A refused rollback leaves that
        // counter where it is, never wraps it, and is reported at ERROR after
        // the seqlock section closes (see `rollback_refused`).
        if let Err(err) =
            Self::checked_fetch_add_u64(&self.quantity_executed, quantity, "quantity_executed")
        {
            let intact = Self::rollback_usize(&self.orders_executed, 1);
            self.mark_degraded();
            drop(write_section);
            return Err(Self::after_rollback(err, intact));
        }

        if let Err(err) =
            Self::checked_fetch_add_u128(&self.value_executed, value, "value_executed")
        {
            let intact = Self::rollback_u64(&self.quantity_executed, quantity)
                & Self::rollback_usize(&self.orders_executed, 1);
            self.mark_degraded();
            drop(write_section);
            return Err(Self::after_rollback(err, intact));
        }

        if let Some(waiting_time) = waiting_time
            && let Err(err) = Self::checked_fetch_add_u64(
                &self.sum_waiting_time,
                waiting_time,
                "sum_waiting_time",
            )
        {
            let intact = Self::rollback_u128(&self.value_executed, value)
                & Self::rollback_u64(&self.quantity_executed, quantity)
                & Self::rollback_usize(&self.orders_executed, 1);
            self.mark_degraded();
            drop(write_section);
            return Err(Self::after_rollback(err, intact));
        }

        // Monotonic (issue #129): an out-of-order record (or an unsupported
        // overlapping one) can never move the "latest execution" backwards.
        // Cheap and independent of the seqlock.
        self.last_execution_time
            .fetch_max(current_time, Ordering::Relaxed);

        Ok(())
    }

    /// Get total number of orders added
    #[must_use]
    pub fn orders_added(&self) -> usize {
        self.orders_added.load(Ordering::Relaxed)
    }

    /// Get total number of orders removed
    #[must_use]
    pub fn orders_removed(&self) -> usize {
        self.orders_removed.load(Ordering::Relaxed)
    }

    /// Get total number of orders executed
    #[must_use]
    pub fn orders_executed(&self) -> usize {
        self.orders_executed.load(Ordering::Relaxed)
    }

    /// Get total quantity executed
    #[must_use]
    pub fn quantity_executed(&self) -> u64 {
        self.quantity_executed.load(Ordering::Relaxed)
    }

    /// Get total value executed: the running `sum(quantity * price)` over every
    /// recorded execution.
    ///
    /// Returned as `u128`, the same width as
    /// [`MatchResult::executed_value`](crate::execution::MatchResult::executed_value)
    /// and [`Trade::total_value`](crate::execution::Trade::total_value) (issue
    /// #140; it was `u64` before 0.10.0).
    #[must_use]
    pub fn value_executed(&self) -> u128 {
        self.value_executed.load(Ordering::Relaxed)
    }

    /// Get the timestamp of the most recent execution, in milliseconds since
    /// the Unix epoch.
    ///
    /// Returns `0` when no execution has been recorded yet.
    #[must_use]
    pub fn last_execution_time(&self) -> u64 {
        self.last_execution_time.load(Ordering::Relaxed)
    }

    /// Get the statistics initialization timestamp, in milliseconds since the
    /// Unix epoch.
    ///
    /// Set from the caller-supplied time at construction
    /// ([`new_at`](Self::new_at) / [`try_new`](Self::try_new)) and on
    /// [`reset`](Self::reset) / [`reset_at`](Self::reset_at). `0` means
    /// **unstamped**: the statistics were built with the deterministic
    /// [`new`](Self::new) / [`Default`] (as `PriceLevel::new` does), or restored
    /// from a legacy payload that omitted the field. The crate never writes `0`
    /// to stand in for a failed clock read. It is **not** updated on order
    /// arrival, so it marks when statistics tracking began for this level, not
    /// the first order's actual arrival time.
    #[must_use]
    pub fn first_arrival_time(&self) -> u64 {
        self.first_arrival_time.load(Ordering::Relaxed)
    }

    /// Get the accumulated waiting time across all executed orders, in
    /// milliseconds.
    ///
    /// This is the sum of `execution_timestamp - order_timestamp` over every
    /// recorded execution that carried a non-zero maker timestamp. Divide by
    /// [`orders_executed`](Self::orders_executed) for the average; see
    /// [`average_waiting_time`](Self::average_waiting_time).
    #[must_use]
    pub fn sum_waiting_time(&self) -> u64 {
        self.sum_waiting_time.load(Ordering::Relaxed)
    }

    /// Returns `true` if the recorded statistics are **degraded** — at least one
    /// execution's contribution was dropped all-or-nothing (a validation error
    /// or a counter overflow in [`record_execution`](Self::record_execution),
    /// issue #117).
    ///
    /// The flag is sticky: once set it stays set until [`reset`](Self::reset).
    /// When `true`, the aggregate counters under-count the true executions; the
    /// emitted trade stream is unaffected. Cleared by `reset`.
    #[must_use]
    pub fn stats_degraded(&self) -> bool {
        self.stats_degraded.load(Ordering::Relaxed)
    }

    /// Get average execution price.
    ///
    /// Reads `value_executed` and `quantity_executed` as two independent
    /// `Relaxed` loads, so under concurrent recording the ratio can be
    /// transiently inconsistent (a torn read across the two counters); it
    /// self-corrects once recording quiesces.
    #[must_use]
    pub fn average_execution_price(&self) -> Option<f64> {
        let qty = self.quantity_executed.load(Ordering::Relaxed);
        let value = self.value_executed.load(Ordering::Relaxed);

        if qty == 0 {
            None
        } else {
            Some(value as f64 / qty as f64)
        }
    }

    /// Get average waiting time for executed orders (in milliseconds).
    ///
    /// Reads `sum_waiting_time` and `orders_executed` as two independent
    /// `Relaxed` loads, so under concurrent recording the ratio can be
    /// transiently inconsistent (a torn read across the two counters); it
    /// self-corrects once recording quiesces.
    #[must_use]
    pub fn average_waiting_time(&self) -> Option<f64> {
        let count = self.orders_executed.load(Ordering::Relaxed);
        let sum = self.sum_waiting_time.load(Ordering::Relaxed);

        if count == 0 {
            None
        } else {
            Some(sum as f64 / count as f64)
        }
    }

    /// Milliseconds elapsed between the most recent execution and `now`.
    ///
    /// Returns `Ok(None)` when no execution has been recorded yet, and
    /// `Ok(Some(elapsed))` otherwise. Clock-free: the caller supplies `now`.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::InvalidOperation`] if `now` is earlier than
    /// the last execution time (a clock running behind the recorded execution),
    /// rather than conflating it with the "no execution" case. With a
    /// concurrent matcher, a `now` the caller sampled before a fill that lands
    /// before this call's load is reported this way; prefer
    /// [`time_since_last_execution`](Self::time_since_last_execution), which
    /// loads the last execution before sampling its clock.
    pub fn time_since_last_execution_at(
        &self,
        now: TimestampMs,
    ) -> Result<Option<u64>, PriceLevelError> {
        let last = self.last_execution_time.load(Ordering::Relaxed);
        Self::elapsed_since_execution(last, now)
    }

    /// Milliseconds elapsed since the most recent execution, reading the
    /// current time once from a caller-supplied [`UnixClock`].
    ///
    /// Returns `Ok(None)` when no execution has been recorded yet; the clock is
    /// not read in that case.
    ///
    /// `last_execution_time` is loaded **once, before** the clock is sampled,
    /// and that same value is used for the difference. A fill recorded by a
    /// concurrent matcher after the load therefore cannot make a healthy clock
    /// reading look earlier than the last execution: the result is measured
    /// from the execution observed at the load.
    ///
    /// # Errors
    ///
    /// Returns the clock's error unchanged, or
    /// [`PriceLevelError::InvalidOperation`] if the clock reports a time
    /// earlier than the execution loaded before it was read.
    pub fn time_since_last_execution<C>(&self, clock: &C) -> Result<Option<u64>, PriceLevelError>
    where
        C: UnixClock + ?Sized,
    {
        let last = self.last_execution_time.load(Ordering::Relaxed);
        if last == 0 {
            return Ok(None);
        }
        let now = clock.try_now_ms()?;
        Self::elapsed_since_execution(last, now)
    }

    /// Shared elapsed-time computation over an already-loaded
    /// `last_execution_time` (`0` means no execution) and a sampled `now`.
    #[inline]
    fn elapsed_since_execution(
        last: u64,
        now: TimestampMs,
    ) -> Result<Option<u64>, PriceLevelError> {
        if last == 0 {
            return Ok(None);
        }
        let now = now.as_u64();
        now.checked_sub(last)
            .map(Some)
            .ok_or_else(|| PriceLevelError::InvalidOperation {
                message: format!("current time {now} is before the last execution time {last}"),
            })
    }

    /// Reset all statistics to zero and re-stamp `first_arrival_time` with the
    /// current time read once from a caller-supplied [`UnixClock`].
    ///
    /// The clock is read **before** anything is mutated, so a failed read
    /// leaves every counter, timestamp and the degraded flag exactly as they
    /// were (issue #171).
    ///
    /// # Errors
    ///
    /// Returns the clock's error unchanged, or the
    /// [`reset_at`](Self::reset_at) error; the statistics are untouched in
    /// both cases.
    ///
    /// # Quiescence contract
    ///
    /// Same as [`reset_at`](Self::reset_at).
    pub fn reset<C>(&self, clock: &C) -> Result<(), PriceLevelError>
    where
        C: UnixClock + ?Sized,
    {
        let started_at = clock.try_now_ms()?;
        self.reset_at(started_at)
    }

    /// Reset all statistics to zero and set `first_arrival_time` to the
    /// caller-supplied `started_at`. Clock-free.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::CounterExhausted`] (counter
    /// [`ExhaustedCounter::StatisticsSequence`]) if the seqlock sequence has
    /// no headroom left for another write section (issue #165). Nothing is
    /// changed: the counters, timestamps and degraded flag keep their values,
    /// and the degraded flag is not set, because no execution was dropped. The
    /// sequence itself is never reset (a reused value could let a reader accept
    /// a torn copy); rebuild the statistics (for example through a snapshot
    /// restore) to start a fresh sequence.
    ///
    /// # Quiescence contract
    ///
    /// This must only be called on a **quiescent** level — with no in-flight
    /// [`record_execution`](Self::record_execution) (and hence no in-flight
    /// `PriceLevel::match_order`) and no other reset. A reset is a seqlock
    /// WRITER (issue #129), but the sequence guard protects READERS only; it
    /// does not exclude a concurrent `record_execution` (issue #153). A reset
    /// overlapping a `record_execution` whose overflow rollback is in progress
    /// can `store(0)` a counter between the committed prefix and its
    /// rollback; the checked rollback is then refused (logged at ERROR, the
    /// statistics stay degraded) rather than wrapping the counter, but the
    /// totals no longer describe the executions. Quiescence is
    /// what rules this out, not the guard. No engine path resets during
    /// matching; it remains a caller obligation because reset is public.
    ///
    /// Under this contract, a multi-field reader (`Clone` / serialize /
    /// `Display`) racing the reset retries and observes either the pre-reset
    /// or the fully reset execution state, never a mix. Concurrent
    /// [`record_order_added`](Self::record_order_added) /
    /// [`record_order_removed`](Self::record_order_removed) are single-counter
    /// increments that the reset may or may not include.
    pub fn reset_at(&self, started_at: TimestampMs) -> Result<(), PriceLevelError> {
        // Seqlock write section: a concurrent multi-field reader retries rather
        // than capture a half-reset copy. Refused with nothing mutated when the
        // sequence has no headroom (issue #165).
        let _write = WriteSeqGuard::try_new(&self.stats_seq)?;

        self.orders_added.store(0, Ordering::Relaxed);
        self.orders_removed.store(0, Ordering::Relaxed);
        self.orders_executed.store(0, Ordering::Relaxed);
        self.quantity_executed.store(0, Ordering::Relaxed);
        self.value_executed.store(0, Ordering::Relaxed);
        self.last_execution_time.store(0, Ordering::Relaxed);
        self.first_arrival_time
            .store(started_at.as_u64(), Ordering::Relaxed);
        self.sum_waiting_time.store(0, Ordering::Relaxed);
        self.stats_degraded.store(false, Ordering::Relaxed);
        Ok(())
    }

    /// Restarts the private seqlock sequence at 0 on an exclusively owned
    /// value, keeping every counter and the degraded flag (issue #150).
    ///
    /// A snapshot restore moves the persisted statistics into the rebuilt
    /// level instead of cloning them; [`Clone`] always produced a fresh even
    /// sequence, and this keeps that behaviour, so rebuilding a level through
    /// a snapshot still recovers one whose sequence was near exhaustion
    /// (issue #165). `&mut self` proves no reader or writer can observe the
    /// store, so no atomic read-modify-write is needed.
    #[inline]
    pub(crate) fn restart_seq_exclusive(&mut self) {
        *self.stats_seq.get_mut() = 0;
    }

    /// Test-only seeding seam (issue #165): place the seqlock sequence at
    /// `value` so the exhaustion protocol can be exercised without an
    /// astronomical number of write sections.
    #[cfg(test)]
    pub(crate) fn test_seed_stats_seq(&self, value: u64) {
        self.stats_seq.store(value, Ordering::Relaxed);
    }

    /// Test-only seeding seam (issue #165): place `orders_added` /
    /// `orders_removed` near their limit.
    #[cfg(test)]
    pub(crate) fn test_seed_order_events(&self, added: usize, removed: usize) {
        self.orders_added.store(added, Ordering::Relaxed);
        self.orders_removed.store(removed, Ordering::Relaxed);
    }

    /// Test-only read of the seqlock sequence (issue #165).
    #[cfg(test)]
    #[must_use]
    pub(crate) fn test_stats_seq(&self) -> u64 {
        self.stats_seq.load(Ordering::Relaxed)
    }
}

/// Test-only layout probe (issue #154).
///
/// Returns `(field, byte offset, byte size)` for every field of
/// [`PriceLevelStatistics`], in declaration order, as laid out by the compiler
/// for the current target. Offsets come from [`std::mem::offset_of!`], so they
/// reflect any field reordering `rustc` applied to this `repr(Rust)` struct.
/// Used to report which fields can share a cache line; it has no production
/// caller.
#[cfg(test)]
impl PriceLevelStatistics {
    pub(crate) fn field_layout() -> [(&'static str, usize, usize); 10] {
        use std::mem::{offset_of, size_of};
        [
            (
                "orders_added",
                offset_of!(Self, orders_added),
                size_of::<AtomicUsize>(),
            ),
            (
                "orders_removed",
                offset_of!(Self, orders_removed),
                size_of::<AtomicUsize>(),
            ),
            (
                "orders_executed",
                offset_of!(Self, orders_executed),
                size_of::<AtomicUsize>(),
            ),
            (
                "quantity_executed",
                offset_of!(Self, quantity_executed),
                size_of::<AtomicU64>(),
            ),
            (
                "value_executed",
                offset_of!(Self, value_executed),
                size_of::<AtomicU128>(),
            ),
            (
                "last_execution_time",
                offset_of!(Self, last_execution_time),
                size_of::<AtomicU64>(),
            ),
            (
                "first_arrival_time",
                offset_of!(Self, first_arrival_time),
                size_of::<AtomicU64>(),
            ),
            (
                "sum_waiting_time",
                offset_of!(Self, sum_waiting_time),
                size_of::<AtomicU64>(),
            ),
            (
                "stats_degraded",
                offset_of!(Self, stats_degraded),
                size_of::<AtomicBool>(),
            ),
            (
                "stats_seq",
                offset_of!(Self, stats_seq),
                size_of::<AtomicU64>(),
            ),
        ]
    }
}

impl Default for PriceLevelStatistics {
    /// Deterministic, clock-free empty statistics with an unstamped start time;
    /// identical to [`PriceLevelStatistics::new`].
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for PriceLevelStatistics {
    /// Clones the statistics by reading a **consistent** snapshot of every field
    /// under the seqlock (issue #129).
    ///
    /// This is the representation persisted in
    /// [`PriceLevelSnapshot`](crate::price_level::PriceLevelSnapshot) and hence
    /// covered by that snapshot's SHA-256 checksum, so the copy must not mix a
    /// pre- and post-`record_execution` prefix — the seqlock retries until it
    /// captures a state the level actually held. A restored level therefore
    /// carries the recorded statistics rather than a fresh, zeroed set.
    ///
    /// The coherence guarantee holds under the single-writer contract (one
    /// `record_execution` at a time, quiescent `reset`; see the struct-level
    /// "Writer contract", issue #153). Clones may run from any number of
    /// threads concurrently with that writer. Under overlapping recorders
    /// (unsupported) a clone can capture a partial execution.
    fn clone(&self) -> Self {
        Self::from_data(self.read_consistent())
    }
}

impl fmt::Display for PriceLevelStatistics {
    /// Formats one seqlock-consistent copy of every field, coherent under the
    /// single-writer contract (see the struct-level "Writer contract", issue
    /// #153).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Consistent multi-field read (issue #129): under the single-writer
        // contract the emitted string is a coherent snapshot, not a torn mix,
        // and round-trips through `FromStr`.
        let d = self.read_consistent();
        write!(
            f,
            "PriceLevelStatistics:orders_added={};orders_removed={};orders_executed={};quantity_executed={};value_executed={};last_execution_time={};first_arrival_time={};sum_waiting_time={};stats_degraded={}",
            d.orders_added,
            d.orders_removed,
            d.orders_executed,
            d.quantity_executed,
            d.value_executed,
            d.last_execution_time,
            d.first_arrival_time,
            d.sum_waiting_time,
            d.stats_degraded
        )
    }
}

impl FromStr for PriceLevelStatistics {
    type Err = PriceLevelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Exactly one `:` separates the `PriceLevelStatistics` tag from the field list.
        let fields_str = match split_exactly_once(s, b':') {
            Some(("PriceLevelStatistics", fields_str)) => fields_str,
            _ => return Err(PriceLevelError::InvalidFormat),
        };

        // `key=value` pairs: a pair without exactly one `=` is ignored and a
        // repeated key keeps its last value (see `utils::text::Fields`).
        const FIELD_NAMES: [&str; 9] = [
            "orders_added",
            "orders_removed",
            "orders_executed",
            "quantity_executed",
            "value_executed",
            "last_execution_time",
            "first_arrival_time",
            "sum_waiting_time",
            "stats_degraded",
        ];
        let fields = Fields::parse(fields_str, &FIELD_NAMES);
        let get_field = |name: &str| fields.require(name);

        let parse_usize = |field: &str, value: &str| -> Result<usize, PriceLevelError> {
            value
                .parse::<usize>()
                .map_err(|_| PriceLevelError::InvalidFieldValue {
                    field: field.to_string(),
                    value: value.to_string(),
                })
        };

        let parse_u64 = |field: &str, value: &str| -> Result<u64, PriceLevelError> {
            value
                .parse::<u64>()
                .map_err(|_| PriceLevelError::InvalidFieldValue {
                    field: field.to_string(),
                    value: value.to_string(),
                })
        };

        // Parse all fields
        let orders_added_str = get_field("orders_added")?;
        let orders_added = parse_usize("orders_added", orders_added_str)?;

        let orders_removed_str = get_field("orders_removed")?;
        let orders_removed = parse_usize("orders_removed", orders_removed_str)?;

        let orders_executed_str = get_field("orders_executed")?;
        let orders_executed = parse_usize("orders_executed", orders_executed_str)?;

        let quantity_executed_str = get_field("quantity_executed")?;
        let quantity_executed = parse_u64("quantity_executed", quantity_executed_str)?;

        let value_executed_str = get_field("value_executed")?;
        // `u128` since issue #140; a string written by an older version (a
        // `u64` value) parses unchanged.
        let value_executed =
            value_executed_str
                .parse::<u128>()
                .map_err(|_| PriceLevelError::InvalidFieldValue {
                    field: "value_executed".to_string(),
                    value: value_executed_str.to_string(),
                })?;

        let last_execution_time_str = get_field("last_execution_time")?;
        let last_execution_time = parse_u64("last_execution_time", last_execution_time_str)?;

        let first_arrival_time_str = get_field("first_arrival_time")?;
        let first_arrival_time = parse_u64("first_arrival_time", first_arrival_time_str)?;

        let sum_waiting_time_str = get_field("sum_waiting_time")?;
        let sum_waiting_time = parse_u64("sum_waiting_time", sum_waiting_time_str)?;

        // `stats_degraded` is optional for backward compatibility: a string
        // produced before the field existed decodes with the flag cleared.
        let stats_degraded = match fields.get("stats_degraded") {
            Some(value) => {
                value
                    .parse::<bool>()
                    .map_err(|_| PriceLevelError::InvalidFieldValue {
                        field: "stats_degraded".to_string(),
                        value: value.to_string(),
                    })?
            }
            None => false,
        };

        Ok(PriceLevelStatistics {
            orders_added: AtomicUsize::new(orders_added),
            orders_removed: AtomicUsize::new(orders_removed),
            orders_executed: AtomicUsize::new(orders_executed),
            quantity_executed: AtomicU64::new(quantity_executed),
            value_executed: AtomicU128::new(value_executed),
            last_execution_time: AtomicU64::new(last_execution_time),
            first_arrival_time: AtomicU64::new(first_arrival_time),
            sum_waiting_time: AtomicU64::new(sum_waiting_time),
            stats_degraded: AtomicBool::new(stats_degraded),
            stats_seq: AtomicU64::new(0),
        })
    }
}

impl Serialize for PriceLevelStatistics {
    /// Serializes one seqlock-consistent copy of every field, coherent under
    /// the single-writer contract (see the struct-level "Writer contract",
    /// issue #153).
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Read all fields as ONE consistent seqlock snapshot (issue #129) so,
        // under the single-writer contract (issue #153), the concurrent
        // `record_execution` can never make the serialized (and hence
        // checksummed) statistics a torn pre/post-write mix.
        let d = self.read_consistent();

        // Serialize `stats_degraded` ONLY when it is `true` (dynamic 8/9-field
        // count). A non-degraded level then serializes in the exact pre-#117
        // 8-field form — byte-identical to a v2 statistics payload persisted
        // before this flag existed — so a `PriceLevelSnapshotPackage`'s SHA-256
        // checksum, recomputed over the re-serialized bytes on
        // `validate` / `from_snapshot_json`, still matches for a legacy v2, a v3
        // and a new v4 non-degraded package (issue #129 keeps checksum
        // recomputation version-agnostic — see `SNAPSHOT_FORMAT_VERSION`). A
        // degraded level adds the 9th field (v3+ shape); `Deserialize` /
        // `FromStr` default a missing flag to `false`, so both directions
        // round-trip.
        let degraded = d.stats_degraded;
        let field_count = if degraded { 9 } else { 8 };
        let mut state = serializer.serialize_struct("PriceLevelStatistics", field_count)?;

        state.serialize_field("orders_added", &d.orders_added)?;
        state.serialize_field("orders_removed", &d.orders_removed)?;
        state.serialize_field("orders_executed", &d.orders_executed)?;
        state.serialize_field("quantity_executed", &d.quantity_executed)?;
        state.serialize_field("value_executed", &d.value_executed)?;
        state.serialize_field("last_execution_time", &d.last_execution_time)?;
        state.serialize_field("first_arrival_time", &d.first_arrival_time)?;
        state.serialize_field("sum_waiting_time", &d.sum_waiting_time)?;
        if degraded {
            state.serialize_field("stats_degraded", &true)?;
        }

        state.end()
    }
}

impl<'de> Deserialize<'de> for PriceLevelStatistics {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        enum Field {
            OrdersAdded,
            OrdersRemoved,
            OrdersExecuted,
            QuantityExecuted,
            ValueExecuted,
            LastExecutionTime,
            FirstArrivalTime,
            SumWaitingTime,
            StatsDegraded,
        }

        impl<'de> Deserialize<'de> for Field {
            fn deserialize<D>(deserializer: D) -> Result<Field, D::Error>
            where
                D: Deserializer<'de>,
            {
                struct FieldVisitor;

                impl Visitor<'_> for FieldVisitor {
                    type Value = Field;

                    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                        formatter.write_str("field name")
                    }

                    fn visit_str<E>(self, value: &str) -> Result<Field, E>
                    where
                        E: de::Error,
                    {
                        match value {
                            "orders_added" => Ok(Field::OrdersAdded),
                            "orders_removed" => Ok(Field::OrdersRemoved),
                            "orders_executed" => Ok(Field::OrdersExecuted),
                            "quantity_executed" => Ok(Field::QuantityExecuted),
                            "value_executed" => Ok(Field::ValueExecuted),
                            "last_execution_time" => Ok(Field::LastExecutionTime),
                            "first_arrival_time" => Ok(Field::FirstArrivalTime),
                            "sum_waiting_time" => Ok(Field::SumWaitingTime),
                            "stats_degraded" => Ok(Field::StatsDegraded),
                            _ => Err(de::Error::unknown_field(value, FIELDS)),
                        }
                    }
                }

                deserializer.deserialize_identifier(FieldVisitor)
            }
        }

        struct StatisticsVisitor;

        impl<'de> Visitor<'de> for StatisticsVisitor {
            type Value = PriceLevelStatistics;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("struct PriceLevelStatistics")
            }

            fn visit_map<V>(self, mut map: V) -> Result<PriceLevelStatistics, V::Error>
            where
                V: MapAccess<'de>,
            {
                let mut orders_added = None;
                let mut orders_removed = None;
                let mut orders_executed = None;
                let mut quantity_executed = None;
                let mut value_executed = None;
                let mut last_execution_time = None;
                let mut first_arrival_time = None;
                let mut sum_waiting_time = None;
                let mut stats_degraded = None;

                while let Some(key) = map.next_key()? {
                    match key {
                        Field::OrdersAdded => {
                            if orders_added.is_some() {
                                return Err(de::Error::duplicate_field("orders_added"));
                            }
                            orders_added = Some(map.next_value()?);
                        }
                        Field::OrdersRemoved => {
                            if orders_removed.is_some() {
                                return Err(de::Error::duplicate_field("orders_removed"));
                            }
                            orders_removed = Some(map.next_value()?);
                        }
                        Field::OrdersExecuted => {
                            if orders_executed.is_some() {
                                return Err(de::Error::duplicate_field("orders_executed"));
                            }
                            orders_executed = Some(map.next_value()?);
                        }
                        Field::QuantityExecuted => {
                            if quantity_executed.is_some() {
                                return Err(de::Error::duplicate_field("quantity_executed"));
                            }
                            quantity_executed = Some(map.next_value()?);
                        }
                        Field::ValueExecuted => {
                            if value_executed.is_some() {
                                return Err(de::Error::duplicate_field("value_executed"));
                            }
                            value_executed = Some(map.next_value()?);
                        }
                        Field::LastExecutionTime => {
                            if last_execution_time.is_some() {
                                return Err(de::Error::duplicate_field("last_execution_time"));
                            }
                            last_execution_time = Some(map.next_value()?);
                        }
                        Field::FirstArrivalTime => {
                            if first_arrival_time.is_some() {
                                return Err(de::Error::duplicate_field("first_arrival_time"));
                            }
                            first_arrival_time = Some(map.next_value()?);
                        }
                        Field::SumWaitingTime => {
                            if sum_waiting_time.is_some() {
                                return Err(de::Error::duplicate_field("sum_waiting_time"));
                            }
                            sum_waiting_time = Some(map.next_value()?);
                        }
                        Field::StatsDegraded => {
                            if stats_degraded.is_some() {
                                return Err(de::Error::duplicate_field("stats_degraded"));
                            }
                            stats_degraded = Some(map.next_value()?);
                        }
                    }
                }

                let orders_added = orders_added.unwrap_or(0);
                let orders_removed = orders_removed.unwrap_or(0);
                let orders_executed = orders_executed.unwrap_or(0);
                let quantity_executed = quantity_executed.unwrap_or(0);
                let value_executed = value_executed.unwrap_or(0);
                let last_execution_time = last_execution_time.unwrap_or(0);

                // A legacy payload that omits the start time decodes as
                // UNSTAMPED (`0`), deterministically (issue #171). The previous
                // behavior stamped the restore instant, which was never the
                // original start time and made decoding depend on the wall clock
                // (and silently wrote `0` on a clock failure). Every package this
                // crate writes carries the field, so checksummed v2/v3/v4
                // packages are unaffected.
                let first_arrival_time = first_arrival_time.unwrap_or(0);

                let sum_waiting_time = sum_waiting_time.unwrap_or(0);
                // Optional for backward compatibility: a payload written before
                // the field existed decodes with the flag cleared.
                let stats_degraded = stats_degraded.unwrap_or(false);

                Ok(PriceLevelStatistics {
                    orders_added: AtomicUsize::new(orders_added),
                    orders_removed: AtomicUsize::new(orders_removed),
                    orders_executed: AtomicUsize::new(orders_executed),
                    quantity_executed: AtomicU64::new(quantity_executed),
                    value_executed: AtomicU128::new(value_executed),
                    last_execution_time: AtomicU64::new(last_execution_time),
                    first_arrival_time: AtomicU64::new(first_arrival_time),
                    sum_waiting_time: AtomicU64::new(sum_waiting_time),
                    stats_degraded: AtomicBool::new(stats_degraded),
                    stats_seq: AtomicU64::new(0),
                })
            }
        }

        const FIELDS: &[&str] = &[
            "orders_added",
            "orders_removed",
            "orders_executed",
            "quantity_executed",
            "value_executed",
            "last_execution_time",
            "first_arrival_time",
            "sum_waiting_time",
            "stats_degraded",
        ];

        deserializer.deserialize_struct("PriceLevelStatistics", FIELDS, StatisticsVisitor)
    }
}

#[cfg(test)]
// Test-only arithmetic (Testing section of `rules/global_rules.md`).
#[allow(clippy::arithmetic_side_effects)]
mod tests {
    use super::{PriceLevelStatistics, STATS_SEQ_CEILING, WriteSeqGuard};

    /// Pre-release hardening: overlapping write sections (a writer-contract
    /// violation) near exhaustion must not strand the sequence on an odd
    /// value, or `read_consistent` would spin forever. Before the fix,
    /// overlapping sections opened near `u64::MAX - 2` could drive the
    /// sequence to the odd `u64::MAX`, where the last exit was refused.
    #[test]
    fn overlapping_sections_near_exhaustion_never_leave_sequence_odd() {
        for start in (u64::MAX - 9)..=(u64::MAX - 1) {
            for open in 1..=5usize {
                let stats = PriceLevelStatistics::new();
                stats.test_seed_stats_seq(start);
                let guards: Vec<WriteSeqGuard<'_>> = (0..open)
                    .filter_map(|_| WriteSeqGuard::try_new(&stats.stats_seq).ok())
                    .collect();
                assert!(stats.test_stats_seq() <= STATS_SEQ_CEILING);
                let opened = guards.len();
                drop(guards);
                let end = stats.test_stats_seq();
                assert!(end <= STATS_SEQ_CEILING, "start {start} open {open}");
                if start % 2 == 0 {
                    assert_eq!(end % 2, 0, "start {start} open {open} opened {opened}");
                    // Terminates: the sequence is even and no writer is open.
                    let _ = stats.read_consistent();
                }
            }
        }
    }

    /// Pre-release hardening: the all-or-nothing rollback is a checked
    /// subtraction; a counter holding less than the delta is left unchanged
    /// (never wrapped) and the refusal is reported.
    #[test]
    fn rollback_refuses_instead_of_wrapping() {
        use portable_atomic::AtomicU128;
        use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

        let a = AtomicUsize::new(0);
        assert!(!PriceLevelStatistics::rollback_usize(&a, 1));
        assert_eq!(a.load(Ordering::Relaxed), 0);
        let b = AtomicU64::new(4);
        assert!(!PriceLevelStatistics::rollback_u64(&b, 5));
        assert_eq!(b.load(Ordering::Relaxed), 4);
        assert!(PriceLevelStatistics::rollback_u64(&b, 4));
        assert_eq!(b.load(Ordering::Relaxed), 0);
        let c = AtomicU128::new(7);
        assert!(!PriceLevelStatistics::rollback_u128(&c, 8));
        assert_eq!(c.load(Ordering::Relaxed), 7);
    }

    #[test]
    fn single_writer_limits_are_unchanged() {
        let stats = PriceLevelStatistics::new();
        stats.test_seed_stats_seq(u64::MAX - 3);
        let guard = WriteSeqGuard::try_new(&stats.stats_seq).expect("last section opens");
        assert_eq!(stats.test_stats_seq(), u64::MAX - 2);
        drop(guard);
        assert_eq!(stats.test_stats_seq(), u64::MAX - 1);
        assert!(WriteSeqGuard::try_new(&stats.stats_seq).is_err());
        assert_eq!(stats.test_stats_seq(), u64::MAX - 1);
    }
}
