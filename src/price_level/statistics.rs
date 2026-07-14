use crate::errors::PriceLevelError;
use serde::de::{self, MapAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

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
/// odd. Writers are serialized by the engine model (one matcher per level +
/// `reset`'s quiescence contract), which the seqlock assumes. The lone
/// read-modify-write loop in [`checked_fetch_add_u64`](Self::checked_fetch_add_u64)
/// is a standard `compare_exchange_weak` CAS retry.
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

    /// Total value executed
    value_executed: AtomicU64,

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
struct WriteSeqGuard<'a> {
    seq: &'a AtomicU64,
}

impl<'a> WriteSeqGuard<'a> {
    #[inline]
    fn new(seq: &'a AtomicU64) -> Self {
        // Enter: even -> odd. `Relaxed` RMW plus a `Release` fence so the field
        // writes that follow cannot be reordered before the odd marker a reader
        // watches for.
        seq.fetch_add(1, Ordering::Relaxed);
        std::sync::atomic::fence(Ordering::Release);
        Self { seq }
    }
}

impl Drop for WriteSeqGuard<'_> {
    #[inline]
    fn drop(&mut self) {
        // Exit: odd -> even, `Release` so every field write in the section
        // happens-before a reader's `Acquire` load of the (now even) sequence.
        self.seq.fetch_add(1, Ordering::Release);
    }
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
    value_executed: u64,
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
    /// [`reset`](Self::reset)) overlapped it, so the copy is NEVER a torn mix of
    /// a pre- and post-write prefix (a bounded fallback that returned a torn copy
    /// would defeat the checksummed snapshot this backs).
    ///
    /// # Liveness
    ///
    /// The loop's work PER attempt is bounded (one sequence load + a nine-field
    /// copy), and it converges under the advisory writer-serialization contract
    /// (one matcher per level + `reset`'s quiescence): a writer holds the section
    /// for only a short, allocation-free burst before dropping the guard back to
    /// even, and `record_execution` is finite, so a reader exits on the first
    /// iteration when uncontended and otherwise as soon as recording quiesces
    /// (which it always does — the matcher cannot record forever). A panicking
    /// writer still restores the even sequence via the guard's `Drop`, so the
    /// reader is never stranded on a permanently-odd sequence.
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
            value_executed: AtomicU64::new(data.value_executed),
            last_execution_time: AtomicU64::new(data.last_execution_time),
            first_arrival_time: AtomicU64::new(data.first_arrival_time),
            sum_waiting_time: AtomicU64::new(data.sum_waiting_time),
            stats_degraded: AtomicBool::new(data.stats_degraded),
            stats_seq: AtomicU64::new(0),
        }
    }

    #[inline]
    fn current_timestamp_milliseconds() -> Result<u64, PriceLevelError> {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| PriceLevelError::InvalidOperation {
                message: format!("system clock error while reading unix time: {error}"),
            })
            .map(|duration| duration.as_millis() as u64)
    }

    #[inline]
    fn current_timestamp_milliseconds_or_zero() -> u64 {
        Self::current_timestamp_milliseconds().unwrap_or(0)
    }

    /// Create new empty statistics
    #[must_use]
    pub fn new() -> Self {
        let current_time = Self::current_timestamp_milliseconds_or_zero();

        Self {
            orders_added: AtomicUsize::new(0),
            orders_removed: AtomicUsize::new(0),
            orders_executed: AtomicUsize::new(0),
            quantity_executed: AtomicU64::new(0),
            value_executed: AtomicU64::new(0),
            last_execution_time: AtomicU64::new(0),
            first_arrival_time: AtomicU64::new(current_time),
            sum_waiting_time: AtomicU64::new(0),
            stats_degraded: AtomicBool::new(false),
            stats_seq: AtomicU64::new(0),
        }
    }

    /// Record a new order being added
    pub fn record_order_added(&self) {
        self.orders_added.fetch_add(1, Ordering::Relaxed);
    }

    /// Record an order being removed without execution
    pub fn record_order_removed(&self) {
        self.orders_removed.fetch_add(1, Ordering::Relaxed);
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
    /// An accepted execution contributes to **every** aggregate, or to **none**.
    /// If a later counter overflows after earlier ones already advanced, this
    /// rolls the committed prefix back (a `fetch_sub` of exactly what this call
    /// added — sound by the same call-backed reservation argument as the #111 /
    /// #113 rollbacks: the subtracted units are exactly the units this call
    /// added, so the undo is commutative with concurrent `record_execution`
    /// deltas). That call-backed argument assumes no concurrent
    /// [`reset`](Self::reset) `store(0)` races the rollback — see the quiescence
    /// contract on `reset`. So a caller never observes a partial contribution in
    /// the final state. On any failure — a validation error or a counter overflow —
    /// the sticky [`stats_degraded`](Self::stats_degraded) flag is set: the
    /// dropped execution is then observable, even though the caller
    /// (`PriceLevel::match_order`) cannot fail the already-committed trade.
    /// Because these are independent lock-free atomics, a *concurrent* reader may
    /// still glimpse a prefix transiently before its rollback (the same window
    /// the #111 reserve-then-rollback admission has); the guarantee is on the
    /// committed final state, not on the transient.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::InvalidOperation`] if any of the counter
    /// accumulations overflow, if the value (`quantity * price`) overflows
    /// `u128`/`u64`, or if `order_timestamp` is strictly greater than
    /// `execution_timestamp` (a maker arriving in the future of execution).
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
        // in-flight prefix, and `reset` — also a writer — cannot interleave this
        // transaction's rollback. The guard's `Drop` closes the section (back to
        // even) on EVERY return path below, including the early validation errors.
        let _write = WriteSeqGuard::new(&self.stats_seq);

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

        let value_u64 = match u128::from(quantity)
            .checked_mul(price)
            .and_then(|value| u64::try_from(value).ok())
        {
            Some(value) => value,
            None => {
                self.mark_degraded();
                return Err(PriceLevelError::InvalidOperation {
                    message: "value_executed overflow (quantity * price exceeds u64 storage)"
                        .to_string(),
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

        if let Err(err) =
            Self::checked_fetch_add_u64(&self.quantity_executed, quantity, "quantity_executed")
        {
            self.orders_executed.fetch_sub(1, Ordering::Relaxed);
            self.mark_degraded();
            return Err(err);
        }

        if let Err(err) =
            Self::checked_fetch_add_u64(&self.value_executed, value_u64, "value_executed")
        {
            self.quantity_executed
                .fetch_sub(quantity, Ordering::Relaxed);
            self.orders_executed.fetch_sub(1, Ordering::Relaxed);
            self.mark_degraded();
            return Err(err);
        }

        if let Some(waiting_time) = waiting_time
            && let Err(err) = Self::checked_fetch_add_u64(
                &self.sum_waiting_time,
                waiting_time,
                "sum_waiting_time",
            )
        {
            self.value_executed.fetch_sub(value_u64, Ordering::Relaxed);
            self.quantity_executed
                .fetch_sub(quantity, Ordering::Relaxed);
            self.orders_executed.fetch_sub(1, Ordering::Relaxed);
            self.mark_degraded();
            return Err(err);
        }

        // Monotonic (issue #129): a concurrent / out-of-order record can never
        // move the "latest execution" backwards. Belt-and-braces with the
        // seqlock, but cheap and independently correct.
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

    /// Get total value executed
    #[must_use]
    pub fn value_executed(&self) -> u64 {
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
    /// Set when the statistics are created and on [`reset`](Self::reset) with the
    /// current wall-clock time (`0` if the system clock could not be read). It is
    /// **not** updated on order arrival, so it marks when statistics tracking
    /// began for this level, not the first order's actual arrival time.
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

    /// Get time since last execution (in milliseconds)
    #[must_use]
    pub fn time_since_last_execution(&self) -> Option<u64> {
        let last = self.last_execution_time.load(Ordering::Relaxed);
        if last == 0 {
            None
        } else {
            let current_time = Self::current_timestamp_milliseconds_or_zero();
            current_time.checked_sub(last)
        }
    }

    /// Reset all statistics to zero (and re-stamp `first_arrival_time`).
    ///
    /// # Quiescence contract
    ///
    /// This must only be called on a **quiescent** level — with no in-flight
    /// [`record_execution`](Self::record_execution) (and hence no in-flight
    /// `PriceLevel::match_order`). `reset` participates in the seqlock as a
    /// WRITER (issue #129), so it can never interleave the middle of a
    /// `record_execution` transaction's rollback (which would otherwise wrap a
    /// counter toward `u64::MAX` via a `store(0)` racing a `fetch_sub`) — but the
    /// seqlock protects READERS, it does not serialize two writers. The
    /// single-matcher-per-level model already serializes `record_execution`, and
    /// this quiescence requirement extends that to `reset`. No engine path calls
    /// `reset` during matching, so the race does not occur today; it remains a
    /// caller obligation because `reset` is public. A multi-field reader
    /// (`Clone` / serialize) racing `reset` retries and observes either the
    /// pre-reset or fully-reset state, never a mix.
    pub fn reset(&self) {
        let current_time = Self::current_timestamp_milliseconds_or_zero();

        // Seqlock write section: a concurrent multi-field reader retries rather
        // than capture a half-reset copy.
        let _write = WriteSeqGuard::new(&self.stats_seq);

        self.orders_added.store(0, Ordering::Relaxed);
        self.orders_removed.store(0, Ordering::Relaxed);
        self.orders_executed.store(0, Ordering::Relaxed);
        self.quantity_executed.store(0, Ordering::Relaxed);
        self.value_executed.store(0, Ordering::Relaxed);
        self.last_execution_time.store(0, Ordering::Relaxed);
        self.first_arrival_time
            .store(current_time, Ordering::Relaxed);
        self.sum_waiting_time.store(0, Ordering::Relaxed);
        self.stats_degraded.store(false, Ordering::Relaxed);
    }
}

impl Default for PriceLevelStatistics {
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
    fn clone(&self) -> Self {
        Self::from_data(self.read_consistent())
    }
}

impl fmt::Display for PriceLevelStatistics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Consistent multi-field read (issue #129): the emitted string is a
        // coherent snapshot, not a torn mix, and round-trips through `FromStr`.
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
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() != 2 || parts[0] != "PriceLevelStatistics" {
            return Err(PriceLevelError::InvalidFormat);
        }

        let fields_str = parts[1];
        let mut fields = std::collections::HashMap::new();

        for field_pair in fields_str.split(';') {
            let kv: Vec<&str> = field_pair.split('=').collect();
            if kv.len() == 2 {
                fields.insert(kv[0], kv[1]);
            }
        }

        let get_field = |field: &str| -> Result<&str, PriceLevelError> {
            match fields.get(field) {
                Some(result) => Ok(*result),
                None => Err(PriceLevelError::MissingField(field.to_string())),
            }
        };

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
        let value_executed = parse_u64("value_executed", value_executed_str)?;

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
                        value: (*value).to_string(),
                    })?
            }
            None => false,
        };

        Ok(PriceLevelStatistics {
            orders_added: AtomicUsize::new(orders_added),
            orders_removed: AtomicUsize::new(orders_removed),
            orders_executed: AtomicUsize::new(orders_executed),
            quantity_executed: AtomicU64::new(quantity_executed),
            value_executed: AtomicU64::new(value_executed),
            last_execution_time: AtomicU64::new(last_execution_time),
            first_arrival_time: AtomicU64::new(first_arrival_time),
            sum_waiting_time: AtomicU64::new(sum_waiting_time),
            stats_degraded: AtomicBool::new(stats_degraded),
            stats_seq: AtomicU64::new(0),
        })
    }
}

impl Serialize for PriceLevelStatistics {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Read all fields as ONE consistent seqlock snapshot (issue #129) so a
        // concurrent `record_execution` can never make the serialized (and hence
        // checksummed) statistics a torn pre/post-write mix.
        let d = self.read_consistent();

        // Serialize `stats_degraded` ONLY when it is `true` (dynamic 8/9-field
        // count). A non-degraded level then serializes in the exact pre-#117
        // 8-field form — byte-identical to a v2 statistics payload persisted
        // before this flag existed — so a `PriceLevelSnapshotPackage`'s SHA-256
        // checksum, recomputed over the re-serialized bytes on
        // `validate` / `from_snapshot_json`, still matches for BOTH a legacy v2
        // and a new v3 non-degraded package (issue #129 keeps checksum
        // recomputation version-agnostic — see `SNAPSHOT_FORMAT_VERSION`). A
        // degraded level adds the 9th field (the v3-only shape); `Deserialize` /
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

                let first_arrival_time = first_arrival_time.unwrap_or_else(|| {
                    PriceLevelStatistics::current_timestamp_milliseconds_or_zero()
                });

                let sum_waiting_time = sum_waiting_time.unwrap_or(0);
                // Optional for backward compatibility: a payload written before
                // the field existed decodes with the flag cleared.
                let stats_degraded = stats_degraded.unwrap_or(false);

                Ok(PriceLevelStatistics {
                    orders_added: AtomicUsize::new(orders_added),
                    orders_removed: AtomicUsize::new(orders_removed),
                    orders_executed: AtomicUsize::new(orders_executed),
                    quantity_executed: AtomicU64::new(quantity_executed),
                    value_executed: AtomicU64::new(value_executed),
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
