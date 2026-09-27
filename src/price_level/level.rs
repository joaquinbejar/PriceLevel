//! Core price level implementation

use crate::UuidGenerator;
use crate::errors::{CapacityResource, ExhaustedCounter, PriceLevelError};
use crate::execution::{MatchResult, TakerKind, Trade};
use crate::orders::{Id, OrderType, OrderUpdate, Side, TimeInForce};
use crate::price_level::order_queue::{
    FrontAction, FrontOutcome, OrderQueue, ParkedSeqs, RemoveOutcome, UpdateDecision,
};
use crate::price_level::snapshot::{BorrowedOrders, SnapshotAggregates, deserialize_plain_orders};
use crate::price_level::statistics::OrderEventDrop;
use crate::price_level::{PriceLevelSnapshot, PriceLevelSnapshotPackage, PriceLevelStatistics};
use crate::utils::alloc::{try_push_back_deque, try_reserve_exact_vec};
use crate::utils::text::{
    MAX_TEXT_NESTING_DEPTH, MAX_TEXT_NESTING_DEPTH_INSIDE_LIST, NestingError, TopLevelSplit,
    try_reserve_str,
};
use crate::utils::{IdBlock, Price, Quantity, TimestampMs};
use serde::{Deserialize, Serialize};
use std::fmt::Display;
use std::str::FromStr;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::fok_guard::FokGuard;

/// Upper bound on the order walks one [`PriceLevel::snapshot`] call performs
/// before it gives up with [`PriceLevelError::InvalidOperation`] (issue #162).
///
/// A walk is only repeated when the previous one was rejected (a mixed-side
/// view across a side transition, or collected aggregates that overflow
/// `u64`), which requires a concurrent mutation to land inside the walk. The
/// bound turns "retry until mutation pauses" into a finite, reported outcome
/// instead of an unbounded loop under sustained mutation.
pub(crate) const SNAPSHOT_MAX_ATTEMPTS: u32 = 8;

/// Error for a collected snapshot vector that mixes both sides.
#[cold]
fn snapshot_mixed_side() -> PriceLevelError {
    PriceLevelError::InvalidOperation {
        message: "snapshot walk captured a mixed-side view across a side transition".to_string(),
    }
}

/// Error returned when every bounded snapshot attempt was rejected. Carries
/// the last rejection's reason.
#[cold]
fn snapshot_attempts_exhausted(price: u128, last: Option<PriceLevelError>) -> PriceLevelError {
    let reason = match last {
        Some(PriceLevelError::InvalidOperation { message }) => message,
        Some(other) => other.to_string(),
        None => "no attempt was made".to_string(),
    };
    PriceLevelError::InvalidOperation {
        message: format!(
            "snapshot of price level {price} could not collect a coherent view after \
             {SNAPSHOT_MAX_ATTEMPTS} attempts under concurrent mutation: {reason}"
        ),
    }
}

/// Pre-size hint for a non-fill-or-kill sweep's result vectors (issues #106,
/// #163): the tighter of the taker quantity (each trade consumes at least one
/// unit) and the resting-order count (each trade consumes one maker step).
///
/// Width policy: `incoming_quantity` is converted with a checked
/// `usize::try_from`, never a truncating cast. A quantity that does not fit
/// `usize` (only possible on targets narrower than 64 bits) is larger than
/// any `usize`, so the minimum is exactly `order_count` — the result is the
/// exact bound, not a clamped default. The hint is advisory: the sweep still
/// reserves fallibly per step.
#[inline]
#[must_use]
pub(crate) fn sweep_capacity_hint(incoming_quantity: u64, order_count: usize) -> usize {
    match usize::try_from(incoming_quantity) {
        Ok(quantity) => quantity.min(order_count),
        // `incoming_quantity > usize::MAX >= order_count`.
        Err(_) => order_count,
    }
}

/// Error for a resting-order count that would underflow on release (issue
/// #163): a removal found the level's count already at zero, i.e. the count
/// disagrees with the queue.
#[cold]
#[inline(never)]
pub(crate) fn topology_underflow(price: u128) -> PriceLevelError {
    PriceLevelError::InvalidOperation {
        message: format!(
            "price level {price} topology count underflow: a removal found a zero resting-order count"
        ),
    }
}

/// Minimum number of makers the fill-or-kill dry run visits lazily before
/// it switches to one bulk collection of the rest (issue #143).
const LAZY_WALK_MIN: u64 = 8;

/// Share of the resting orders the fill-or-kill dry run visits lazily, as a
/// divisor: `count / 64` (issue #143).
const LAZY_WALK_DIVISOR: u64 = 64;

/// Lazy-walk budget of the fill-or-kill dry run for a level resting `count`
/// orders (issue #143): `max(8, count / 64)`.
///
/// Measured per maker on a 10,000-order level (release, Apple M5 Max): the
/// lazy step (index entry + `DashMap` lookup + `Arc` clone) costs about
/// 27 ns, against about 14 ns for its share of one collect-and-sort. A fill
/// within the budget never touches the makers behind it, while a walk that
/// must go further (for example a fill-or-kill proving a kill) pays the
/// budget's lazy steps on top of the bulk pass: about `27 / (64 * 14)`, some
/// 3%, of that pass on a deep level, and at most 8 steps on a shallow one.
/// The budget only moves work between the two phases; the visited sequence,
/// and so the prediction, is the same.
#[inline]
fn lazy_walk_budget(count: u64) -> u64 {
    #[cfg(test)]
    if let Some(budget) = LAZY_WALK_BUDGET_OVERRIDE.with(std::cell::Cell::get) {
        return budget;
    }
    count
        .checked_div(LAZY_WALK_DIVISOR)
        .map_or(LAZY_WALK_MIN, |share| share.max(LAZY_WALK_MIN))
}

#[cfg(test)]
thread_local! {
    static LAZY_WALK_BUDGET_OVERRIDE: std::cell::Cell<Option<u64>> =
        const { std::cell::Cell::new(None) };
}

/// Restores the previous lazy-walk budget override on drop.
#[cfg(test)]
pub(crate) struct LazyWalkBudgetGuard(Option<u64>);

#[cfg(test)]
impl Drop for LazyWalkBudgetGuard {
    fn drop(&mut self) {
        LAZY_WALK_BUDGET_OVERRIDE.with(|cell| cell.set(self.0));
    }
}

/// Test seam (issue #143): forces the dry run's lazy-walk budget on this
/// thread so small books exercise the bulk continuation.
#[cfg(test)]
pub(crate) fn override_lazy_walk_budget(budget: u64) -> LazyWalkBudgetGuard {
    LazyWalkBudgetGuard(LAZY_WALK_BUDGET_OVERRIDE.with(|cell| cell.replace(Some(budget))))
}

/// Counts one more parked maker for the fill-or-kill park-set preflight
/// (issue #164).
///
/// # Errors
///
/// [`PriceLevelError::CapacityExceeded`] (resource
/// [`CapacityResource::SweepScratch`]) if the count does not fit `usize`: the
/// real sweep could not record that park either, so the dry run stops there
/// with the error instead of silently under-reporting the fill.
#[inline]
pub(crate) fn count_park(parks: usize) -> Result<usize, PriceLevelError> {
    parks
        .checked_add(1)
        .ok_or_else(|| PriceLevelError::capacity_exceeded(CapacityResource::SweepScratch, 1))
}

/// Error for a post-lock replenish counter transition that could not be
/// applied (issue #164): the level counters no longer describe the queue and
/// the level has been poisoned.
#[cold]
#[inline(never)]
fn replenish_counter_failure(price: u128) -> PriceLevelError {
    PriceLevelError::InvalidOperation {
        message: format!(
            "price level {price} replenish counter transition refused after the queue commit; level poisoned — reconstruct it from a snapshot"
        ),
    }
}

/// Bit layout of the [`PriceLevel::topology`] word (issue #126): the high two
/// bits carry the pinned-side tag, the low bits the resting-order count. Packing
/// both into one atomic makes the side pin and the count move together in a
/// single compare-exchange, so a drain's un-pin can never race an admission's
/// pin across two independent atomics.
mod topology {
    use crate::errors::PriceLevelError;
    use crate::orders::Side;

    /// Bits reserved for the resting-order count (the rest hold the side tag).
    /// `u64::MAX >> 2` orders is astronomically beyond any level's capacity, so
    /// nothing is lost by borrowing the top two bits for the tag.
    ///
    /// The shifts by this constant in [`pack`] / [`tag`] are valid by
    /// construction (`62 < u64::BITS`); they are not a reachable shift overflow.
    pub(super) const COUNT_BITS: u32 = 62;
    pub(super) const COUNT_MASK: u64 = (1 << COUNT_BITS) - 1;

    /// Largest resting-order count a level admits or restores (issue #163):
    /// the 62-bit count field, further capped at `usize::MAX` on narrower
    /// targets so [`super::PriceLevel::order_count`] converts it to `usize`
    /// exactly. The caps are widening constants (`u32` / `u16` -> `u64`),
    /// never a narrowing cast.
    #[cfg(target_pointer_width = "64")]
    pub(super) const MAX_COUNT: u64 = COUNT_MASK;
    #[cfg(target_pointer_width = "32")]
    pub(super) const MAX_COUNT: u64 = u32::MAX as u64;
    #[cfg(target_pointer_width = "16")]
    pub(super) const MAX_COUNT: u64 = u16::MAX as u64;

    pub(super) const TAG_UNPINNED: u64 = 0;
    pub(super) const TAG_BUY: u64 = 1;
    pub(super) const TAG_SELL: u64 = 2;

    #[inline]
    pub(super) fn tag_of(side: Side) -> u64 {
        match side {
            Side::Buy => TAG_BUY,
            Side::Sell => TAG_SELL,
        }
    }

    #[inline]
    pub(super) fn side_of_tag(tag: u64) -> Option<Side> {
        match tag {
            TAG_BUY => Some(Side::Buy),
            TAG_SELL => Some(Side::Sell),
            _ => None,
        }
    }

    /// Pack a side tag and a count the caller has already bounded: `tag` is
    /// one of the `TAG_*` constants (at most two bits) and `count <=
    /// MAX_COUNT` (from a checked admission, a checked release, or a literal
    /// `0` / `1`). A count derived from external input goes through
    /// [`try_pack`] instead.
    #[inline]
    pub(super) fn pack(tag: u64, count: u64) -> u64 {
        (tag << COUNT_BITS) | count
    }

    /// Checked [`pack`] for a count derived from external input (issue #163):
    /// a snapshot's order-vector length on restore. Validates the tag and the
    /// count bound so an oversized count can never bleed into the tag bits.
    ///
    /// # Errors
    ///
    /// [`PriceLevelError::InvalidOperation`] if `tag` is not a `TAG_*` value
    /// or `count` exceeds [`MAX_COUNT`].
    pub(super) fn try_pack(tag: u64, count: usize) -> Result<u64, PriceLevelError> {
        if tag > TAG_SELL {
            return Err(PriceLevelError::InvalidOperation {
                message: format!("price level topology tag {tag} is not a valid side tag"),
            });
        }
        match u64::try_from(count) {
            Ok(count) if count <= MAX_COUNT => Ok(pack(tag, count)),
            _ => Err(PriceLevelError::InvalidOperation {
                message: format!(
                    "price level order count {count} exceeds the representable maximum {MAX_COUNT}"
                ),
            }),
        }
    }

    /// `usize` view of a word's count half (issue #163). Every stored count is
    /// `<= MAX_COUNT <= usize::MAX` (admission and restore enforce it), so the
    /// checked conversion always succeeds; `None` would mean a count that
    /// bypassed those checks.
    #[inline]
    pub(super) fn count_usize(word: u64) -> Option<usize> {
        usize::try_from(count(word)).ok()
    }

    #[inline]
    pub(super) fn tag(word: u64) -> u64 {
        word >> COUNT_BITS
    }

    #[inline]
    pub(super) fn count(word: u64) -> u64 {
        word & COUNT_MASK
    }
}

// Deterministic race seam for the post-only decision boundary (issue #130).
//
// `match_order` fires `fire_post_only_decision_hook` BETWEEN the post-only depth
// decision and its commit, so a test can install a hook that mutates the level
// (e.g. `add_order`) in that exact window and assert the outcome
// deterministically, without relying on scheduler stress. Production builds
// compile none of this — the hook call site is `#[cfg(test)]`.
#[cfg(test)]
thread_local! {
    static POST_ONLY_DECISION_HOOK: std::cell::RefCell<Option<Box<dyn FnMut()>>> =
        const { std::cell::RefCell::new(None) };
}

/// Install a hook fired at the post-only decision boundary (test seam, issue
/// #130). Returns a guard that clears the hook on drop.
#[cfg(test)]
pub(crate) fn set_post_only_decision_hook(hook: Box<dyn FnMut()>) -> PostOnlyHookGuard {
    POST_ONLY_DECISION_HOOK.with(|slot| *slot.borrow_mut() = Some(hook));
    PostOnlyHookGuard
}

/// Clears the post-only decision hook when dropped (test seam, issue #130).
#[cfg(test)]
pub(crate) struct PostOnlyHookGuard;

#[cfg(test)]
impl Drop for PostOnlyHookGuard {
    fn drop(&mut self) {
        POST_ONLY_DECISION_HOOK.with(|slot| *slot.borrow_mut() = None);
    }
}

// Deterministic seam at the start of a non-fill-or-kill sweep (issue #164):
// fired after every pre-sweep check (self-match, post-only) and before the
// first `match_front`, so a test can admit an order the pre-checks did not
// see (e.g. one sharing the taker id, which the sweep then parks). Never
// fired for a fill-or-kill taker, whose exclusive guard would deadlock an
// admission. Production builds compile none of this.
#[cfg(test)]
thread_local! {
    static SWEEP_START_HOOK: std::cell::RefCell<Option<Box<dyn FnMut()>>> =
        const { std::cell::RefCell::new(None) };
}

/// Clears the sweep-start hook when dropped (test seam, issue #164).
#[cfg(test)]
pub(crate) struct SweepStartHookGuard;

#[cfg(test)]
impl Drop for SweepStartHookGuard {
    fn drop(&mut self) {
        SWEEP_START_HOOK.with(|slot| *slot.borrow_mut() = None);
    }
}

/// Install a one-shot hook fired at the start of a non-fill-or-kill sweep
/// (test seam, issue #164).
#[cfg(test)]
pub(crate) fn set_sweep_start_hook(hook: Box<dyn FnMut()>) -> SweepStartHookGuard {
    SWEEP_START_HOOK.with(|slot| *slot.borrow_mut() = Some(hook));
    SweepStartHookGuard
}

/// Fire (and consume) the sweep-start hook if one is installed.
#[cfg(test)]
fn fire_sweep_start_hook() {
    let hook = SWEEP_START_HOOK.with(|slot| slot.borrow_mut().take());
    if let Some(mut hook) = hook {
        hook();
    }
}

// Deterministic seam between the terminal self-match lookup and the
// fill-or-kill exclusive-guard acquisition (issue #164 review): a test can
// admit an order sharing the taker id in exactly the window a concurrent
// mutator could, on the matcher thread and with no lock held, so the dry run
// then sees (and parks) it. Production builds compile none of this.
#[cfg(test)]
thread_local! {
    static PRE_FOK_LOCK_HOOK: std::cell::RefCell<Option<Box<dyn FnMut()>>> =
        const { std::cell::RefCell::new(None) };
}

/// Clears the pre-fill-or-kill-lock hook when dropped (test seam).
#[cfg(test)]
pub(crate) struct PreFokLockHookGuard;

#[cfg(test)]
impl Drop for PreFokLockHookGuard {
    fn drop(&mut self) {
        PRE_FOK_LOCK_HOOK.with(|slot| *slot.borrow_mut() = None);
    }
}

/// Install a one-shot hook fired after the self-match lookup and before a
/// fill-or-kill taker acquires the exclusive guard (test seam, issue #164).
#[cfg(test)]
pub(crate) fn set_pre_fok_lock_hook(hook: Box<dyn FnMut()>) -> PreFokLockHookGuard {
    PRE_FOK_LOCK_HOOK.with(|slot| *slot.borrow_mut() = Some(hook));
    PreFokLockHookGuard
}

/// Fire (and consume) the pre-fill-or-kill-lock hook if one is installed.
#[cfg(test)]
fn fire_pre_fok_lock_hook() {
    let hook = PRE_FOK_LOCK_HOOK.with(|slot| slot.borrow_mut().take());
    if let Some(mut hook) = hook {
        hook();
    }
}

// Deterministic seam fired right after a fill-or-kill taker acquires the
// exclusive guard, before its dry run (issue #206): a test observes the level
// while no mutator can run, e.g. to record the true FIFO front the sweep must
// consume, or to hold the guard while a mutator blocks. Unlike the one-shot
// hooks above it stays installed across calls until its guard drops.
// Production builds compile none of this.
#[cfg(test)]
thread_local! {
    static FOK_LOCKED_HOOK: std::cell::RefCell<Option<Box<dyn FnMut()>>> =
        const { std::cell::RefCell::new(None) };
}

/// Clears the fill-or-kill-locked hook when dropped (test seam, issue #206).
#[cfg(test)]
pub(crate) struct FokLockedHookGuard;

#[cfg(test)]
impl Drop for FokLockedHookGuard {
    fn drop(&mut self) {
        FOK_LOCKED_HOOK.with(|slot| *slot.borrow_mut() = None);
    }
}

/// Install a persistent hook fired each time a fill-or-kill taker on this
/// thread holds the exclusive guard (test seam, issue #206).
#[cfg(test)]
pub(crate) fn set_fok_locked_hook(hook: Box<dyn FnMut()>) -> FokLockedHookGuard {
    FOK_LOCKED_HOOK.with(|slot| *slot.borrow_mut() = Some(hook));
    FokLockedHookGuard
}

/// Fire the fill-or-kill-locked hook, if installed, and keep it installed.
#[cfg(test)]
fn fire_fok_locked_hook() {
    let hook = FOK_LOCKED_HOOK.with(|slot| slot.borrow_mut().take());
    if let Some(mut hook) = hook {
        hook();
        FOK_LOCKED_HOOK.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.is_none() {
                *slot = Some(hook);
            }
        });
    }
}

/// Fire the post-only decision hook if one is installed (test seam, issue #130).
#[cfg(test)]
fn fire_post_only_decision_hook() {
    // Take the hook OUT of the slot while firing so a re-entrant `match_order`
    // inside the hook does not double-borrow the `RefCell`.
    let hook = POST_ONLY_DECISION_HOOK.with(|slot| slot.borrow_mut().take());
    if let Some(mut hook) = hook {
        hook();
        POST_ONLY_DECISION_HOOK.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.is_none() {
                *slot = Some(hook);
            }
        });
    }
}

// Injection seam for the update decision (issue #163). `UpdateQuantity`
// passes its decided order through `apply_update_decision_hook` just before
// returning the decision to `OrderQueue::update_entry_with`, so a test can
// substitute an order carrying a DIFFERENT id and assert that the queue
// rejects it before any counter is reserved. Production builds compile none of
// this — the call site is `#[cfg(test)]`.
#[cfg(test)]
type UpdateDecisionHook = Box<dyn FnMut(Arc<OrderType<()>>) -> Arc<OrderType<()>>>;

#[cfg(test)]
thread_local! {
    static UPDATE_DECISION_HOOK: std::cell::RefCell<Option<UpdateDecisionHook>> =
        const { std::cell::RefCell::new(None) };
}

/// Install a hook that rewrites the order an `UpdateQuantity` decision commits
/// (test seam, issue #163). Returns a guard that clears the hook on drop.
#[cfg(test)]
pub(crate) fn set_update_decision_hook(hook: UpdateDecisionHook) -> UpdateDecisionHookGuard {
    UPDATE_DECISION_HOOK.with(|slot| {
        if let Ok(mut slot) = slot.try_borrow_mut() {
            *slot = Some(hook);
        }
    });
    UpdateDecisionHookGuard
}

/// Clears the update decision hook when dropped (test seam, issue #163).
#[cfg(test)]
pub(crate) struct UpdateDecisionHookGuard;

#[cfg(test)]
impl Drop for UpdateDecisionHookGuard {
    fn drop(&mut self) {
        UPDATE_DECISION_HOOK.with(|slot| {
            if let Ok(mut slot) = slot.try_borrow_mut() {
                *slot = None;
            }
        });
    }
}

/// Pass `order` through the update decision hook if one is installed (test
/// seam, issue #163); identity otherwise. Never panics: a busy slot (a
/// re-entrant update inside the hook) is treated as "no hook".
#[cfg(test)]
fn apply_update_decision_hook(order: Arc<OrderType<()>>) -> Arc<OrderType<()>> {
    let hook =
        UPDATE_DECISION_HOOK.with(|slot| slot.try_borrow_mut().ok().and_then(|mut s| s.take()));
    match hook {
        Some(mut hook) => {
            let rewritten = hook(order);
            UPDATE_DECISION_HOOK.with(|slot| {
                if let Ok(mut slot) = slot.try_borrow_mut()
                    && slot.is_none()
                {
                    *slot = Some(hook);
                }
            });
            rewritten
        }
        None => order,
    }
}

/// Signed change of one level quantity counter for an update moving a
/// component `old -> new` (issue #163). Built with `abs_diff`, which is exact
/// and total, so no guarded ordinary subtraction is needed to derive it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CounterDelta {
    /// The component grows by this many quantity units.
    Increase(u64),
    /// The component shrinks by this many quantity units.
    Decrease(u64),
}

impl CounterDelta {
    /// The delta taking a component from `old` to `new`.
    #[inline]
    #[must_use]
    pub(crate) fn between(old: u64, new: u64) -> Self {
        if new >= old {
            Self::Increase(new.abs_diff(old))
        } else {
            Self::Decrease(old.abs_diff(new))
        }
    }

    /// The inverse delta (what undoes this one).
    #[inline]
    #[must_use]
    pub(crate) fn inverse(self) -> Self {
        match self {
            Self::Increase(d) => Self::Decrease(d),
            Self::Decrease(d) => Self::Increase(d),
        }
    }

    /// Apply the delta to `counter` with a checked `fetch_update`: an increase
    /// never overflows `u64`, a decrease never underflows. `Relaxed`: advisory
    /// level counters (issue #68); the queue commit carries the happens-before.
    ///
    /// Returns `true` if applied, `false` if the checked step failed (the
    /// counter is then unchanged).
    #[inline]
    #[must_use = "a failed checked step must be handled"]
    pub(crate) fn apply(self, counter: &AtomicU64) -> bool {
        let step = |c: u64| match self {
            Self::Increase(d) => c.checked_add(d),
            Self::Decrease(d) => c.checked_sub(d),
        };
        counter
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, step)
            .is_ok()
    }
}

/// Counter reservation plan for one `UpdateQuantity` (issue #163): computed as
/// pure data by the update decision and applied only after the queue has
/// validated that decision, so a rejected decision never leaves a reservation
/// behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct UpdatePlan {
    visible: CounterDelta,
    hidden: CounterDelta,
}

impl UpdatePlan {
    /// Plan for a maker moving `visible: old -> new` and `hidden: old -> new`.
    #[inline]
    #[must_use]
    pub(crate) fn new(
        old_visible: u64,
        new_visible: u64,
        old_hidden: u64,
        new_hidden: u64,
    ) -> Self {
        Self {
            visible: CounterDelta::between(old_visible, new_visible),
            hidden: CounterDelta::between(old_hidden, new_hidden),
        }
    }

    /// Reserve both counters, all or nothing.
    ///
    /// An increase is applied first when the components move in opposite
    /// directions, so the rollback of the first reservation after a failed
    /// second one only ever subtracts units this call added (still counted,
    /// so provably in range). Only when both components shrink can the
    /// rollback re-add units, and the second shrink failing already requires a
    /// counter below its own live contribution. If a rollback still fails,
    /// `rollback_failed` is set so the caller poisons the level (the counters
    /// no longer describe the queue) instead of reporting a clean rejection.
    ///
    /// # Errors
    ///
    /// [`PriceLevelError::InvalidOperation`] if either reservation would
    /// overflow / underflow its counter; the first reservation is rolled back.
    pub(crate) fn reserve(
        self,
        visible_counter: &AtomicU64,
        hidden_counter: &AtomicU64,
        rollback_failed: &mut bool,
    ) -> Result<(), PriceLevelError> {
        let visible = (self.visible, visible_counter);
        let hidden = (self.hidden, hidden_counter);
        let (first, second) = match (self.visible, self.hidden) {
            (CounterDelta::Decrease(_), CounterDelta::Increase(_)) => (hidden, visible),
            _ => (visible, hidden),
        };
        if !first.0.apply(first.1) {
            return Err(update_counter_overflow());
        }
        if !second.0.apply(second.1) {
            if !first.0.inverse().apply(first.1) {
                *rollback_failed = true;
            }
            return Err(update_counter_overflow());
        }
        Ok(())
    }
}

/// Error for an update whose level-counter reservation would overflow or
/// underflow a counter (issue #128 / #163).
#[cold]
#[inline(never)]
fn update_counter_overflow() -> PriceLevelError {
    PriceLevelError::InvalidOperation {
        message: "price level quantity counter overflow on update".to_string(),
    }
}

/// A price level in a limit order book.
///
/// The ordered index (`crossbeam-skiplist`) and the atomic counters are
/// lock-free; the complete public methods are not. A `Gtc` / `Ioc` / `Gtd` /
/// `Day` match commits each fill under the maker's `DashMap` shard write lock,
/// the serialization point it shares with a cancel or resize of that order
/// (see [`Self::match_order`]). It supports **one logical matcher per level**:
/// concurrent `match_order` calls on the same level must be serialized by the
/// caller. The mutators [`Self::add_order`] and [`Self::update_order`] (cancel
/// and resize included) take their target's shard write lock and the
/// **shared** side of a per-level reader-writer guard. That
/// acquisition is normally uncontended (it only coordinates with a fill-or-kill
/// match), but it can BLOCK behind a concurrent fill-or-kill: a `Fok` match
/// takes the guard's **exclusive** side across its feasibility check and sweep —
/// a critical section proportional to the makers the fill visits while they
/// fit the dry run's lazy budget, and `O(depth log depth)` past it (issue
/// #143) — so it stays all-or-nothing against
/// concurrent mutation. See the `fok_guard` field and [`Self::match_order`] for
/// the full argument (issue #112). A blocked mutator announces itself, and a
/// fill-or-kill match yields to announced mutators for a bounded budget before
/// it retakes the exclusive side, so a matcher looping fill-or-kill calls
/// cannot starve them (issue #206; see `price_level::fok_guard`).
///
/// # Topology
///
/// Every resting order sits at [`Self::price`] and shares a single side. The
/// side is **pinned atomically** rather than derived from the queue: a single
/// `topology` word packs `(pinned side, resting order count)` so that an
/// admission's side decision and the drain that un-pins an emptied level are
/// one compare-exchange, never two racing atomics (issue #126). The first
/// admitted maker pins the side; each later same-side admission bumps the
/// count under the same CAS; the removal that brings the count to zero un-pins
/// in the same CAS, so a fully drained level accepts either side again. An
/// opposite-side admission into a non-empty level is rejected.
///
/// Single-side coherence is a **correctness invariant**, not an
/// eventually-consistent one — unlike the advisory quantity / count counters
/// (issue #68), a level that admitted two makers of opposite sides would not
/// converge to a correct state later. Pinning side+count in one atomic upholds
/// it under **arbitrary concurrent admissions and removals**, closing both
/// races the earlier derive-from-queue scheme left open:
///
/// - Two **opposite-side admissions into a genuinely empty level** now
///   serialize on the pin CAS: exactly one establishes the side, the other
///   observes a non-empty opposite-side level and is rejected.
/// - An **opposite-side admission racing a same-side upsize** cannot slip
///   through a transient queue gap: the pin persists across a maker's
///   demotion because the count never reaches zero (and as of issue #119 the
///   quantity-increase demotion re-sequences in place without vacating the id
///   at all).
///
/// A concurrent [`Self::snapshot`] cannot capture a torn old-side/new-side view
/// across a drain-then-re-admit either: a `topology_epoch` is bumped on every
/// side pin / un-pin, and `snapshot` retries its materialization if the epoch
/// moves under it (see there).
///
/// `Debug` reads every field into locals (materializing the queue) before it
/// writes to the caller's formatter, and omits the fill-or-kill guard, so no
/// shard lock or `RwLock` guard is held while caller-supplied formatting
/// destination code runs (issue #172).
pub struct PriceLevel {
    /// The price of this level
    price: u128,

    /// Total visible quantity at this price level
    visible_quantity: AtomicU64,

    /// Total hidden quantity at this price level
    hidden_quantity: AtomicU64,

    /// Packed `(pinned side, resting order count)` — the atomic topology word
    /// (issue #126). The side tag lives in the high two bits, the count in the
    /// low [`topology::COUNT_BITS`]; see the [`topology`] module for the layout
    /// and [`Self::topology_admit`] / [`Self::topology_release_one`] for the CAS
    /// protocol. Replaces the former standalone `order_count` counter — the
    /// count is now read back out of this word.
    topology: AtomicU64,

    /// Monotonic counter bumped on every side pin / un-pin (issue #126). A
    /// [`Self::snapshot`] reads it before and after materializing the orders and
    /// retries if it moved, so a checksummed snapshot can never capture a torn
    /// old-side/new-side view across a drain-then-re-admit transition.
    topology_epoch: AtomicU64,

    /// Queue of orders at this price level
    orders: OrderQueue,

    /// Statistics for this price level
    stats: Arc<PriceLevelStatistics>,

    /// Fill-or-kill exclusion guard (issue #112).
    ///
    /// A fill-or-kill match must be **all-or-nothing** with respect to
    /// concurrent `add_order` / `update_order` (cancel / resize): between its
    /// depth dry-run and its sweep, a concurrent shrink could otherwise leave it
    /// partially filled. All-or-nothing across N makers is a cross-maker
    /// transactional property the per-entry atomics cannot express, so the FOK
    /// path takes this guard's **write** (exclusive) side across its dry-run and
    /// sweep, and the mutators ([`Self::add_order`] / [`Self::update_order`])
    /// take the **read** (shared) side. The common paths therefore pay only an
    /// uncontended shared acquisition; a FOK (a cold, specific TIF) excludes
    /// mutators for the duration of its feasibility check and sweep — an
    /// exclusive section proportional to the makers the fill visits while
    /// they fit the dry run's lazy budget, and `O(depth log depth)` past it
    /// (issue #143), not a constant-time one. The non-FOK sweep
    /// takes NO fill-or-kill guard (it still takes each maker's `DashMap` shard
    /// lock) — it relies on the
    /// single-matcher-per-level model and the existing per-entry cancel
    /// atomicity (issue #81). The guarded value is `()`, so a poisoned lock is
    /// recovered with `into_inner` — but a poison means a holder PANICKED
    /// mid-operation, which may have left the level half-mutated, so the recovery
    /// also trips [`Self::level_poisoned`] and the level then fails fast rather
    /// than silently reopening (issue #130).
    ///
    /// `std::sync::RwLock` promises no fairness, so the lock is wrapped in a
    /// [`FokGuard`] that adds a bounded hand-off (issue #206): a mutator whose
    /// shared acquisition would block announces itself, and a fill-or-kill
    /// match that sees an announcement waits, holding no lock, until every
    /// announced mutator holds the shared side or its budget runs out. The
    /// announcement count is a scheduling hint only; exclusion still comes
    /// from the lock alone.
    fok_guard: FokGuard,

    /// Sticky fail-fast flag set when a poisoned [`Self::fok_guard`] is recovered
    /// (issue #130): a guard holder panicked mid-operation, so the level may be
    /// half-mutated and cannot be trusted. While set, `add_order` / `update_order`
    /// return [`PriceLevelError::InvalidOperation`] and `match_order` refuses to
    /// match (returns an empty result); `snapshot` stays allowed for diagnostics
    /// / reconstruction. The production match sweep has no unwind path (audited),
    /// so this is defense-in-depth; a poisoned level requires reconstruction from
    /// a snapshot. Never cleared once set.
    ///
    /// It is also set (issue #163) when an internal invariant is found broken
    /// AFTER a committed queue removal (the topology count underflows on the
    /// release that follows the removal, or an update's counter rollback
    /// cannot be applied): the level's count or counters then no longer
    /// describe its queue, so it fails fast the same way. Every such state is
    /// validated before the removal / commit it follows, so it is reachable
    /// only if the count or a counter already disagreed with the queue.
    level_poisoned: AtomicBool,

    /// Monotonic counter bumped by every committing `add_order` / `update_order`
    /// (cancel / resize) so the non-atomic post-only depth scan can linearize
    /// (issue #130). [`Self::has_matchable_depth`] reads it, scans the queue,
    /// re-reads it, and retries on change: a stable epoch across the scan means
    /// no mutation committed during it, giving the post-only verdict a
    /// linearization point instead of a torn read.
    mutation_epoch: AtomicU64,
}

/// Result of [`PriceLevel::dry_run`]: what a sweep would fill, how many
/// trades it would emit, how many makers it would re-sequence at the tail
/// (replenishments, each needing a fresh FIFO sequence; issue #165), plus the
/// typed failure that would stop the sweep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DryRun {
    pub(crate) filled: u64,
    pub(crate) trades: usize,
    /// Replenishments that keep the maker resident (`ReplaceAtTail`), each of
    /// which reserves one fresh FIFO sequence in the real sweep (issue #165).
    pub(crate) replenishes: u64,
    /// Makers the real sweep would park (self-trade skip or no-progress
    /// guard), each of which inserts one sequence into the sweep's
    /// parked-sequence set (issue #164). Fill-or-kill reserves that set
    /// before the first mutation.
    pub(crate) parks: usize,
    /// The [`OrderType::match_against`] error the real sweep would hit at the
    /// maker where the dry run stopped (issue #169). `filled` / `trades` are
    /// then the committed prefix the real sweep would report alongside it.
    pub(crate) error: Option<PriceLevelError>,
}

/// What the dry run may assume about concurrent re-sequencing (issue #143).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DryRunIsolation {
    /// The caller holds the fill-or-kill guard's exclusive side under the
    /// one-matcher contract: no admission, update or sweep can run, so the
    /// bounded lazy walk over the index is exact.
    FokExclusive,
    /// No guard (the public [`PriceLevel::matchable_quantity`]): a GTC
    /// replenish or a demoting resize can re-sequence a maker during the
    /// walk, so the walk collects from the id-keyed map instead.
    Unguarded,
}

#[cfg(test)]
thread_local! {
    static DRY_RUN_TAIL_REVISITED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Test coverage probe (issue #143): whether the dry run has revisited a
/// replenished tranche from its tail buffer on this thread since the last
/// call. A flag, not a counter, so the probe does no arithmetic.
#[cfg(test)]
pub(crate) fn test_take_tail_revisited() -> bool {
    DRY_RUN_TAIL_REVISITED.with(|cell| cell.replace(false))
}

/// Terminal epoch value (issue #165): an epoch never moves past it, and a
/// reader that loads it treats the epoch as "changed, unknown". See
/// `PriceLevel::bump_epoch`.
const EPOCH_EXHAUSTED: u64 = u64::MAX;

/// Headroom kept below [`EPOCH_EXHAUSTED`] for bumps already in flight when an
/// operation passes the headroom check (issue #165).
const EPOCH_HEADROOM: u64 = 1 << 32;

/// An operation is refused before it mutates anything once either epoch has
/// reached this value (issue #165).
const EPOCH_MUTATION_LIMIT: u64 = EPOCH_EXHAUSTED - EPOCH_HEADROOM;

impl PriceLevel {
    /// Reconstructs a price level directly from a snapshot.
    ///
    /// The rebuilt level carries the per-level statistics persisted in the
    /// snapshot (orders added / removed / executed, quantity / value executed,
    /// waiting-time aggregates, and execution / arrival timestamps) rather than
    /// a fresh, zeroed set — so a restored level resumes with its recorded
    /// history.
    ///
    /// The orders are validated in two walks (issue #150): an allocation-free
    /// checked aggregate fold, then one fused pass over ids and topology.
    /// They are then enqueued in vector order, which is the restored price-time priority
    /// (issue #109). The snapshot's stored aggregate fields are not trusted:
    /// the restored counters are recomputed from the orders.
    ///
    /// # Errors
    ///
    /// When a snapshot violates several rules, the error returned is the
    /// highest-ranked one below, independent of where in the orders vector
    /// each violation sits (the ranking is the order of the pre-#150 separate
    /// validation walks, preserved exactly):
    ///
    /// 1. [`PriceLevelError::InvalidOperation`] if an order's own visible +
    ///    hidden total overflows `u64`, or if the level's visible or hidden sum
    ///    overflows `u64` (the same per-order and per-level invariants
    ///    [`Self::add_order`] enforces at admission); the first such order in
    ///    vector order is reported.
    /// 2. [`PriceLevelError::CapacityExceeded`] (resource
    ///    [`CapacityResource::RestoreScratch`]) if the duplicate-id scratch set
    ///    cannot be reserved (issue #164).
    /// 3. [`PriceLevelError::DuplicateOrderId`] for the first repeated id.
    /// 4. [`PriceLevelError::InvalidOperation`] for the first order whose price
    ///    differs from the snapshot price or whose side differs from the first
    ///    order's side.
    /// 5. [`PriceLevelError::CounterExhausted`] if the queue cannot mint an
    ///    insertion sequence (not reachable from a fresh queue), then
    ///    [`PriceLevelError::InvalidOperation`] if the order count does not fit
    ///    the level's topology word.
    ///
    /// Nothing is built on error.
    pub fn from_snapshot(snapshot: PriceLevelSnapshot) -> Result<Self, PriceLevelError> {
        let validated = snapshot.into_validated_restore()?;

        let order_count = validated.aggregates.order_count;
        // Fallible (issue #165): uniqueness was validated above, and a fresh
        // queue cannot exhaust its sequence on a `Vec`, but any insertion
        // failure is propagated rather than silently dropping an order.
        let queue = OrderQueue::try_from(validated.orders)?;

        // Pin the restored side alongside the restored count in the topology
        // word (issue #126). An empty snapshot restores Unpinned; a non-empty
        // one pins the single side the validation proved coherent. The count is
        // validated against `topology::MAX_COUNT` with a checked conversion
        // (issue #163) rather than assumed to fit.
        let side_tag = validated
            .side
            .map_or(topology::TAG_UNPINNED, topology::tag_of);
        let topology_word = topology::try_pack(side_tag, order_count)?;

        Ok(Self {
            price: validated.price.as_u128(),
            visible_quantity: AtomicU64::new(validated.aggregates.visible_quantity.as_u64()),
            hidden_quantity: AtomicU64::new(validated.aggregates.hidden_quantity.as_u64()),
            topology: AtomicU64::new(topology_word),
            topology_epoch: AtomicU64::new(0),
            orders: queue,
            // Moved, not cloned: the snapshot is consumed.
            stats: Arc::new(validated.statistics),
            fok_guard: FokGuard::new(),
            level_poisoned: AtomicBool::new(false),
            mutation_epoch: AtomicU64::new(0),
        })
    }

    /// The pre-#150 restore, kept verbatim and test-only so the two-walk
    /// validation of [`Self::from_snapshot`] can be compared against it
    /// (identical results, identical error precedence).
    #[cfg(test)]
    pub(crate) fn from_snapshot_legacy(
        mut snapshot: PriceLevelSnapshot,
    ) -> Result<Self, PriceLevelError> {
        snapshot.refresh_aggregates()?;

        // Reject a snapshot whose orders vector repeats an id. Building the
        // queue would drop the duplicate (keep-first), but the reconstructed
        // counters are taken from the snapshot's own aggregates / order count,
        // so a silently-dropped duplicate would leave the restored level's
        // counters disagreeing with its queue. Fail deterministically instead.
        {
            let orders = snapshot.orders();
            // Sized by an input-derived length, so reserved fallibly (issue
            // #164) rather than with the aborting `with_capacity`.
            let mut seen = std::collections::HashSet::new();
            crate::utils::alloc::try_reserve_set(
                &mut seen,
                orders.len(),
                CapacityResource::RestoreScratch,
            )?;
            for order in orders {
                if !seen.insert(order.id()) {
                    return Err(PriceLevelError::DuplicateOrderId(order.id().to_string()));
                }
            }
        }

        // Topology invariants (same rules `add_order` enforces): every order
        // must sit at the level's price and share a single side. A snapshot that
        // violates either would reconstruct a level that trades at the wrong
        // price or emits contradictory taker sides, so reject it here rather
        // than restore an incoherent level.
        let level_side: Option<Side> = {
            let level_price = snapshot.price().as_u128();
            let mut level_side = None;
            for order in snapshot.orders() {
                if order.price().as_u128() != level_price {
                    return Err(PriceLevelError::InvalidOperation {
                        message: format!(
                            "snapshot order price {} does not match level price {level_price}",
                            order.price().as_u128()
                        ),
                    });
                }
                match level_side {
                    None => level_side = Some(order.side()),
                    Some(side) if side != order.side() => {
                        return Err(PriceLevelError::InvalidOperation {
                            message: format!(
                                "snapshot order side {:?} is incompatible with the level side {side:?}",
                                order.side()
                            ),
                        });
                    }
                    Some(_) => {}
                }
            }
            level_side
        };

        let order_count = snapshot.orders().len();
        let visible_quantity = snapshot.visible_quantity().as_u64();
        let hidden_quantity = snapshot.hidden_quantity().as_u64();
        let price = snapshot.price().as_u128();
        // Clone the persisted statistics before consuming the snapshot's orders.
        let stats = (*snapshot.statistics()).clone();
        // Fallible (issue #165): uniqueness was validated above, and a fresh
        // queue cannot exhaust its sequence on a `Vec`, but any insertion
        // failure is propagated rather than silently dropping an order.
        let queue = OrderQueue::try_from(snapshot.into_orders())?;

        // Pin the restored side alongside the restored count in the topology word
        // (issue #126). An empty snapshot restores Unpinned; a non-empty one pins
        // the single side the validation above proved coherent. The count is
        // the snapshot's own vector length: validated against
        // `topology::MAX_COUNT` with a checked conversion (issue #163) rather
        // than assumed to fit, so an oversized count never reaches the tag bits.
        let side_tag = level_side.map_or(topology::TAG_UNPINNED, topology::tag_of);
        let topology_word = topology::try_pack(side_tag, order_count)?;

        Ok(Self {
            price,
            visible_quantity: AtomicU64::new(visible_quantity),
            hidden_quantity: AtomicU64::new(hidden_quantity),
            topology: AtomicU64::new(topology_word),
            topology_epoch: AtomicU64::new(0),
            orders: queue,
            stats: Arc::new(stats),
            fok_guard: FokGuard::new(),
            level_poisoned: AtomicBool::new(false),
            mutation_epoch: AtomicU64::new(0),
        })
    }

    /// Reconstructs a price level from a checksum-protected snapshot package.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::ChecksumMismatch`] if the package's embedded
    /// SHA-256 checksum does not match its payload (tampered or corrupted
    /// snapshot), [`PriceLevelError::SerializationError`] if re-encoding the
    /// payload to recompute that checksum fails,
    /// [`PriceLevelError::InvalidOperation`] if the package carries an
    /// unsupported snapshot format version, and propagates any
    /// [`PriceLevelError`] from rebuilding the level out of the validated
    /// snapshot.
    pub fn from_snapshot_package(
        package: PriceLevelSnapshotPackage,
    ) -> Result<Self, PriceLevelError> {
        let snapshot = package.into_snapshot()?;
        Self::from_snapshot(snapshot)
    }

    /// Restores a price level from its snapshot JSON representation.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::DeserializationError`] if `data` is not a
    /// valid snapshot-package JSON document, [`PriceLevelError::ChecksumMismatch`]
    /// if the decoded package's SHA-256 checksum does not match its payload,
    /// [`PriceLevelError::SerializationError`] if re-encoding the payload to
    /// recompute that checksum fails, [`PriceLevelError::InvalidOperation`]
    /// on an unsupported snapshot format version,
    /// [`PriceLevelError::DuplicateOrderId`] if the decoded snapshot's orders
    /// vector repeats an order id, and [`PriceLevelError::CapacityExceeded`]
    /// if a validation buffer cannot be reserved (issue #164; a refusal while
    /// decoding is a `DeserializationError`).
    pub fn from_snapshot_json(data: &str) -> Result<Self, PriceLevelError> {
        let package = PriceLevelSnapshotPackage::from_json(data)?;
        Self::from_snapshot_package(package)
    }
}

impl PriceLevel {
    /// Create a new, empty price level.
    ///
    /// Deterministic and clock-free (issue #171): the level's statistics start
    /// **unstamped** (`stats().first_arrival_time() == 0`), so two levels built
    /// from the same input snapshot to byte-identical packages. To record when
    /// tracking began, call
    /// [`PriceLevelStatistics::reset_at`] or
    /// [`PriceLevelStatistics::reset`] on `stats()` while the level is still
    /// quiescent.
    #[must_use]
    pub fn new(price: u128) -> Self {
        Self {
            price,
            visible_quantity: AtomicU64::new(0),
            hidden_quantity: AtomicU64::new(0),
            // Unpinned side, zero resting orders.
            topology: AtomicU64::new(topology::pack(topology::TAG_UNPINNED, 0)),
            topology_epoch: AtomicU64::new(0),
            orders: OrderQueue::new(),
            stats: Arc::new(PriceLevelStatistics::new()),
            fok_guard: FokGuard::new(),
            level_poisoned: AtomicBool::new(false),
            mutation_epoch: AtomicU64::new(0),
        }
    }

    /// Get the price of this level
    #[must_use]
    pub fn price(&self) -> u128 {
        self.price
    }

    /// Get the visible quantity, in quantity units.
    ///
    /// This is an **advisory, eventually-consistent** read: it loads a single
    /// atomic counter, which under concurrent `add_order` / `match_order` /
    /// `update_order` can briefly lead or lag the queue contents (it may not yet
    /// include an order already in the queue, or still count one just removed).
    /// The relative order of the counter update and the queue mutation is not a
    /// guaranteed cross-method invariant — different paths order them
    /// differently (e.g. iceberg replenishment in `match_order` adjusts the
    /// counters before pushing the refreshed tranche). Treat any single counter
    /// read as approximate; for a reading where the counters and the order list
    /// are guaranteed mutually consistent, take a [`Self::snapshot`] and read
    /// from it.
    #[must_use]
    pub fn visible_quantity(&self) -> u64 {
        // `Relaxed`: this counter is advisory / eventually-consistent (see the
        // doc above and issue #68). It carries NO happens-before relationship —
        // the `SkipMap` index and `DashMap` storage in `OrderQueue` carry the real
        // ordering between producers and consumers, and `snapshot()` is the
        // mutually-consistent view. Nothing is published or synchronized through
        // this load, so `Acquire` would buy nothing.
        self.visible_quantity.load(Ordering::Relaxed)
    }

    /// Get the hidden quantity, in quantity units.
    ///
    /// Advisory / eventually-consistent under concurrent mutation — see
    /// [`Self::visible_quantity`]; use [`Self::snapshot`] for a consistent view.
    #[must_use]
    pub fn hidden_quantity(&self) -> u64 {
        // `Relaxed`: advisory counter, no happens-before rides on it — see
        // `visible_quantity` for the full rationale.
        self.hidden_quantity.load(Ordering::Relaxed)
    }

    /// Get the total quantity (visible + hidden), in quantity units.
    ///
    /// Advisory / eventually-consistent under concurrent mutation (sums two
    /// independent atomic counters) — see [`Self::visible_quantity`]; use
    /// [`Self::snapshot`] for a consistent view.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::InvalidOperation`] if `visible + hidden`
    /// overflows `u64`.
    pub fn total_quantity(&self) -> Result<u64, PriceLevelError> {
        self.visible_quantity()
            .checked_add(self.hidden_quantity())
            .ok_or_else(|| PriceLevelError::InvalidOperation {
                message: "price level total quantity overflow".to_string(),
            })
    }

    /// Get the number of orders.
    ///
    /// Advisory / eventually-consistent under concurrent mutation — see
    /// [`Self::visible_quantity`]; use [`Self::snapshot`] for a consistent view.
    ///
    /// Width policy (issue #163): admission and snapshot restore cap the count
    /// at `usize::MAX` on every target (as well as at the 62-bit count field),
    /// so the stored `u64` count converts to `usize` exactly with a checked
    /// conversion — never a truncating cast. A count past that cap cannot be
    /// stored; the checked conversion's failure arm reports the cap itself.
    #[must_use]
    #[inline]
    pub fn order_count(&self) -> usize {
        // `Relaxed`: advisory read of the count half of the topology word, no
        // happens-before rides on it — see `visible_quantity` for the rationale.
        topology::count_usize(self.topology.load(Ordering::Relaxed)).unwrap_or(usize::MAX)
    }

    /// The side currently pinned at this level, or `None` if the level is empty
    /// (Unpinned). Advisory: a concurrent admission / drain can change it right
    /// after the read.
    #[must_use]
    fn pinned_side(&self) -> Option<Side> {
        topology::side_of_tag(topology::tag(self.topology.load(Ordering::Relaxed)))
    }

    /// Reserve one admission slot for a `side` order: pin the side (or verify it
    /// matches the pinned side) and increment the resting-order count, in a
    /// single compare-exchange (issue #126).
    ///
    /// Returns `Ok(true)` iff this call pinned a previously-empty level (the
    /// caller then bumps [`Self::topology_epoch`]), `Ok(false)` if it joined an
    /// already-pinned same-side level.
    ///
    /// Because the side and the count move together, two opposite-side
    /// admissions into an empty level serialize here: exactly one wins the CAS
    /// that pins the side, and the loser then observes a non-empty opposite-side
    /// level and is rejected. There is no separate "is the level empty" atomic to
    /// fall out of step with the side.
    ///
    /// # Errors
    ///
    /// [`PriceLevelError::InvalidOperation`] if `side` is incompatible with the
    /// pinned side of a non-empty level, or if the count would exceed
    /// [`topology::MAX_COUNT`] (the 62-bit field, capped at `usize::MAX`).
    fn topology_admit(&self, side: Side) -> Result<bool, PriceLevelError> {
        let my_tag = topology::tag_of(side);
        loop {
            let cur = self.topology.load(Ordering::Acquire);
            let tag = topology::tag(cur);
            let count = topology::count(cur);
            if count == 0 {
                // Empty level: establish this side with count 1.
                let next = topology::pack(my_tag, 1);
                if self
                    .topology
                    .compare_exchange_weak(cur, next, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    return Ok(true);
                }
            } else if tag == my_tag {
                // Same side: bump the count (checked — never wraps).
                let Some(new_count) = count.checked_add(1).filter(|c| *c <= topology::MAX_COUNT)
                else {
                    return Err(PriceLevelError::InvalidOperation {
                        message: "price level order count overflow on admission".to_string(),
                    });
                };
                let next = topology::pack(my_tag, new_count);
                if self
                    .topology
                    .compare_exchange_weak(cur, next, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    return Ok(false);
                }
            } else {
                // Non-empty level pinned to the opposite side: reject.
                let resting = topology::side_of_tag(tag);
                return Err(PriceLevelError::InvalidOperation {
                    message: format!(
                        "order side {side:?} is incompatible with the level's resting side {resting:?}"
                    ),
                });
            }
            // Lost the CAS to a concurrent mutation; reload and retry.
        }
    }

    /// Release one admission slot after removing an order: decrement the count
    /// and un-pin the side when it reaches zero, in a single compare-exchange
    /// (issue #126).
    ///
    /// Returns `true` iff this call brought the count to zero and un-pinned the
    /// level (the caller then bumps [`Self::topology_epoch`]). Because the un-pin
    /// rides the same CAS as the decrement, a concurrent admission either sees
    /// the still-pinned non-empty level (and joins / is rejected) or the drained
    /// Unpinned level (and establishes) — never an inconsistent in-between.
    ///
    /// The decrement is checked (issue #163). Callers validate with
    /// [`Self::topology_releasable`] inside the removal's per-entry critical
    /// section and commit this release through [`Self::release_after_removal`]
    /// after the removal, so a zero count is rejected with nothing mutated in
    /// the ordinary case.
    ///
    /// # Errors
    ///
    /// [`PriceLevelError::InvalidOperation`] if the count is already zero. The
    /// topology word is left unchanged (never wrapped, never silently kept as
    /// a no-op success).
    fn topology_release_one(&self) -> Result<bool, PriceLevelError> {
        loop {
            let cur = self.topology.load(Ordering::Acquire);
            let Some(new_count) = topology::count(cur).checked_sub(1) else {
                return Err(topology_underflow(self.price));
            };
            let next = if new_count == 0 {
                topology::pack(topology::TAG_UNPINNED, 0)
            } else {
                topology::pack(topology::tag(cur), new_count)
            };
            if self
                .topology
                .compare_exchange_weak(cur, next, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return Ok(new_count == 0);
            }
        }
    }

    /// Whether the release that follows a removal can commit (issue #163):
    /// the resting-order count is at least one.
    ///
    /// Callers evaluate this INSIDE the removal's per-entry critical section
    /// (the [`OrderQueue::remove_if`] check for cancels / price moves, the
    /// [`OrderQueue::match_front`] decision closure for a full consume), with
    /// the order to be removed resident and locked. Every resting order was
    /// counted by [`Self::topology_admit`] under its own entry lock before it
    /// was published, and is released only after its own removal, which needs
    /// that same lock. So while the caller holds it, this order's count is
    /// included and nobody else can release it: `false` means the count
    /// already disagrees with the queue, never a transient race with an
    /// admission or cancellation. The caller then rejects the removal with
    /// nothing mutated and builds the error after releasing the lock.
    ///
    /// Allocation-free and event-free, so it may run under a shard lock.
    #[inline]
    #[must_use]
    fn topology_releasable(&self) -> bool {
        // `Acquire`: pairs with the `AcqRel` admission / release CAS, so the
        // check observes every count change that happened-before this removal
        // (in particular this resident order's own admission, published under
        // the entry lock the caller now holds).
        topology::count(self.topology.load(Ordering::Acquire)) != 0
    }

    /// Commit the topology release for an order this call already removed
    /// from the queue (issue #163), bumping the topology epoch on an un-pin.
    ///
    /// The removal was validated by [`Self::topology_releasable`] under its
    /// entry lock, so a
    /// failure here is reachable only if the count disagreed with the queue
    /// before this call (see that method). The removal cannot be undone
    /// without re-exposing a stale queue position, so the level is poisoned
    /// (sticky fail-fast, reconstruct from a snapshot) and the typed error is
    /// returned — never a silent no-op. No event is emitted here: the caller
    /// logs after its bookkeeping (and after releasing the fill-or-kill guard,
    /// issue #172).
    ///
    /// # Errors
    ///
    /// [`PriceLevelError::InvalidOperation`] from [`Self::topology_release_one`].
    fn release_after_removal(&self) -> Result<(), PriceLevelError> {
        match self.topology_release_one() {
            Ok(true) => {
                self.bump_topology_epoch();
                Ok(())
            }
            Ok(false) => Ok(()),
            Err(err) => {
                self.trip_poison();
                Err(err)
            }
        }
    }

    /// Bump the topology epoch on a side pin / un-pin so a racing
    /// [`Self::snapshot`] retries a materialization that spanned the transition.
    /// Checked; see [`Self::bump_epoch`] for the exhaustion protocol.
    #[inline]
    fn bump_topology_epoch(&self) {
        Self::bump_epoch(&self.topology_epoch);
    }

    /// Bump the mutation epoch on a committed add / cancel / resize so a racing
    /// post-only depth scan retries (issue #130). `Release` so the queue mutation
    /// that precedes it happens-before a scanner's `Acquire` read of the epoch.
    /// Checked; see [`Self::bump_epoch`] for the exhaustion protocol.
    #[inline]
    fn bump_mutation_epoch(&self) {
        Self::bump_epoch(&self.mutation_epoch);
    }

    /// Checked `Release` increment of an epoch (issue #165).
    ///
    /// # Exhaustion protocol
    ///
    /// An epoch is a change detector: a reader compares two loads for
    /// equality. It never wraps (a wrapped epoch could repeat a value a reader
    /// already holds). The protocol has three parts:
    ///
    /// 1. **Sentinel.** [`EPOCH_EXHAUSTED`] (`u64::MAX`) is the last value an
    ///    epoch can take. Once there, a bump leaves it unchanged, and every
    ///    reader treats a load of the sentinel as "changed, unknown": the
    ///    snapshot walk always runs its structural single-side check, and the
    ///    post-only depth scan returns
    ///    [`PriceLevelError::CounterExhausted`] instead of a verdict. Readers
    ///    therefore never trust an epoch that stopped moving.
    /// 2. **Headroom.** Every mutator ([`Self::add_order`],
    ///    [`Self::update_order`]) and every sweep ([`Self::match_order`])
    ///    checks, before it changes anything, that both epochs are below
    ///    [`EPOCH_MUTATION_LIMIT`] (`2^32` below the sentinel) and refuses
    ///    with [`PriceLevelError::CounterExhausted`] otherwise. An accepted
    ///    operation bumps each epoch at most once (a sweep bumps the topology
    ///    epoch once per maker that drains the level), so the post-commit bump
    ///    only reaches the sentinel if more than `2^32` such bumps were in
    ///    flight past the check at once.
    /// 3. **Bump.** The increment itself is a checked CAS, so even in that
    ///    case it stops at the sentinel instead of wrapping, and part 1 keeps
    ///    the readers sound. A refused bump therefore needs no error path of
    ///    its own, which matters because it runs after the mutation committed
    ///    (and, for a side pin, under a queue shard lock where nothing may be
    ///    logged).
    ///
    /// Recovery is a rebuild: [`Self::from_snapshot`] starts both epochs at 0.
    #[inline]
    fn bump_epoch(epoch: &AtomicU64) {
        // `Release` on success, as before; `Relaxed` on the refused path,
        // which publishes nothing (the value stays at the sentinel).
        let _ = epoch.fetch_update(Ordering::Release, Ordering::Relaxed, |e| e.checked_add(1));
    }

    /// Refuse an operation, before it changes anything, when either epoch has
    /// no headroom left (issue #165; see [`Self::bump_epoch`]).
    #[inline]
    fn check_epoch_headroom(&self) -> Result<(), PriceLevelError> {
        if self.topology_epoch.load(Ordering::Relaxed) >= EPOCH_MUTATION_LIMIT {
            return Err(PriceLevelError::counter_exhausted(
                ExhaustedCounter::TopologyEpoch,
            ));
        }
        if self.mutation_epoch.load(Ordering::Relaxed) >= EPOCH_MUTATION_LIMIT {
            return Err(PriceLevelError::counter_exhausted(
                ExhaustedCounter::MutationEpoch,
            ));
        }
        Ok(())
    }

    /// Test-only seeding seam (issue #165): place both epochs at the given
    /// values so the exhaustion protocol can be exercised without `2^64`
    /// mutations.
    /// Test-only access to the bounded fill-or-kill dry run (issue #143), so
    /// its prediction can be compared with the retained reference model.
    #[cfg(test)]
    pub(crate) fn test_dry_run(
        &self,
        incoming_quantity: u64,
        taker_id: Id,
    ) -> Result<DryRun, PriceLevelError> {
        self.dry_run(incoming_quantity, taker_id, DryRunIsolation::FokExclusive)
    }

    #[cfg(test)]
    pub(crate) fn test_seed_epochs(&self, topology: u64, mutation: u64) {
        self.topology_epoch.store(topology, Ordering::Relaxed);
        self.mutation_epoch.store(mutation, Ordering::Relaxed);
    }

    /// Test-only: run one post-commit bump of each epoch (issue #165), to
    /// exercise the sentinel without passing the headroom check.
    #[cfg(test)]
    pub(crate) fn test_bump_epochs(&self) {
        self.bump_topology_epoch();
        self.bump_mutation_epoch();
    }

    /// Test-only read of `(topology_epoch, mutation_epoch)` (issue #165).
    #[cfg(test)]
    #[must_use]
    pub(crate) fn test_epochs(&self) -> (u64, u64) {
        (
            self.topology_epoch.load(Ordering::Relaxed),
            self.mutation_epoch.load(Ordering::Relaxed),
        )
    }

    /// Test-only access to the order queue (issue #165 sequence seams).
    #[cfg(test)]
    #[must_use]
    pub(crate) fn test_queue(&self) -> &OrderQueue {
        &self.orders
    }

    /// Record an order-event statistic after a committed mutation (issue
    /// #165). The statistics refuse to wrap an exhausted counter and mark
    /// themselves degraded; the committed mutation stands. Returns the error
    /// only when this call's own degraded-flag CAS performed the
    /// `false -> true` transition, so across any number of concurrent
    /// admissions / cancels exactly one caller logs the anomaly (after all of
    /// its bookkeeping), not one per event.
    #[inline]
    fn record_order_event(
        &self,
        record: fn(&PriceLevelStatistics) -> Result<(), OrderEventDrop>,
    ) -> Option<PriceLevelError> {
        match record(&self.stats) {
            Ok(()) => None,
            Err(drop) if drop.degraded_now => Some(drop.error),
            Err(_) => None,
        }
    }

    /// Log an order-event statistics drop reported by
    /// [`Self::record_order_event`]. `WARN`: the mutation committed; only the
    /// advisory counter is saturated, and the sticky degraded flag records it.
    #[cold]
    #[inline(never)]
    fn warn_order_event_dropped(&self, err: &PriceLevelError) {
        tracing::warn!(
            price = self.price,
            error = %err,
            "order-event statistic not recorded (counter exhausted); level stats marked degraded, mutation unaffected"
        );
    }

    /// Returns `true` if `orders` is empty or every order shares one side — the
    /// single-side coherence [`Self::from_snapshot`] requires. Used as the
    /// termination backstop for `snapshot`'s torn-topology retry (issue #126).
    fn is_single_side(orders: &[Arc<OrderType<()>>]) -> bool {
        let mut side = None;
        for order in orders {
            match side {
                None => side = Some(order.side()),
                Some(s) if s != order.side() => return false,
                Some(_) => {}
            }
        }
        true
    }

    /// Get the statistics for this price level
    #[must_use]
    pub fn stats(&self) -> Arc<PriceLevelStatistics> {
        self.stats.clone()
    }

    /// Acquire the fill-or-kill guard's **shared (read)** side — the mutator
    /// side. Multiple mutators proceed concurrently; a fill-or-kill match
    /// (holding the exclusive side) excludes them. A poisoned lock is recovered
    /// with `into_inner` (the guarded value is `()`), but the recovery trips the
    /// sticky [`Self::level_poisoned`] flag so the level then fails fast (issue
    /// #130): a poison means a holder panicked mid-operation.
    #[inline]
    fn fok_read(&self) -> std::sync::RwLockReadGuard<'_, ()> {
        self.fok_guard.read().unwrap_or_else(|poison| {
            self.mark_poisoned();
            poison.into_inner()
        })
    }

    /// Acquire the fill-or-kill guard's **exclusive (write)** side — held across
    /// a fill-or-kill dry-run + sweep so no mutator can change the matchable
    /// depth mid-decision. First yields, for a bounded budget, to mutators
    /// already blocked on the shared side (issue #206). A poisoned lock is
    /// recovered (see [`Self::fok_read`]).
    #[inline]
    fn fok_write(&self) -> std::sync::RwLockWriteGuard<'_, ()> {
        self.fok_guard.write().unwrap_or_else(|poison| {
            self.mark_poisoned();
            poison.into_inner()
        })
    }

    /// Trip the sticky poison flag when a [`Self::fok_guard`] poison is recovered
    /// (issue #130). Logs `ERROR` exactly once — on the `false -> true`
    /// transition decided by the `compare_exchange` — so a poisoned level is
    /// reported but not flooded.
    #[cold]
    fn mark_poisoned(&self) {
        if self.trip_poison() {
            tracing::error!(
                price = self.price,
                "price level poisoned by a panicked operation; matching and mutation are now refused — reconstruct the level from a snapshot"
            );
        }
    }

    /// Set the sticky poison flag WITHOUT emitting an event (issue #163), for
    /// a broken internal invariant detected after a committed removal. Returns
    /// `true` on the `false -> true` transition, so the caller can log once
    /// after its bookkeeping and outside the fill-or-kill guard (issue #172).
    /// `Relaxed`: the flag is advisory fail-fast state, like
    /// [`Self::mark_poisoned`]; no other field's visibility rides on it.
    #[cold]
    fn trip_poison(&self) -> bool {
        self.level_poisoned
            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    }

    /// Returns `true` if the level has been poisoned by a panicked guard holder
    /// (issue #130).
    #[inline]
    #[must_use]
    fn is_poisoned(&self) -> bool {
        self.level_poisoned.load(Ordering::Relaxed)
    }

    /// Fail-fast guard for the mutating public methods: `Err` once the level is
    /// poisoned (issue #130).
    #[inline]
    fn poison_check(&self) -> Result<(), PriceLevelError> {
        if self.is_poisoned() {
            Err(PriceLevelError::InvalidOperation {
                message: "price level poisoned by a panicked operation or a broken internal invariant; reconstruct it from a snapshot".to_string(),
            })
        } else {
            Ok(())
        }
    }

    /// Genuinely poison the fill-or-kill guard by panicking while holding its
    /// write side (issue #130 test seam). The panic is caught so the test
    /// process survives; the `RwLock` is left poisoned, so the NEXT guard
    /// acquisition recovers it and trips [`Self::level_poisoned`], exercising the
    /// real fail-fast path (not a directly-set flag).
    /// Force the topology word to `side` pinned with the maximum representable
    /// order count (issue #145 test seam), so the next same-side admission
    /// fails `topology_admit` with a count overflow and exercises the
    /// visible / hidden rollback branch of [`Self::add_order`]. Leaves the queue
    /// and quantity counters untouched; the level is only fit for asserting
    /// that rollback afterwards.
    #[cfg(test)]
    pub(crate) fn test_saturate_order_count(&self, side: Side) {
        self.topology.store(
            topology::pack(topology::tag_of(side), topology::MAX_COUNT),
            Ordering::Release,
        );
    }

    /// Overwrite the topology word with `side` pinned (or Unpinned for `None`)
    /// and resting-order `count` (issue #163 test seam), so a test can make the
    /// count disagree with the queue (e.g. zero while orders rest) and exercise
    /// the checked release. Queue and quantity counters are left untouched.
    #[cfg(test)]
    pub(crate) fn test_force_topology(&self, side: Option<Side>, count: u64) {
        let tag = side.map_or(topology::TAG_UNPINNED, topology::tag_of);
        self.topology.store(
            topology::pack(tag, count & topology::COUNT_MASK),
            Ordering::Release,
        );
    }

    /// Raw resting-order count from the topology word (issue #163 test seam).
    #[cfg(test)]
    #[must_use]
    pub(crate) fn test_topology_count(&self) -> u64 {
        topology::count(self.topology.load(Ordering::Acquire))
    }

    /// Whether the sticky poison flag is set (issue #163 test seam).
    #[cfg(test)]
    #[must_use]
    pub(crate) fn test_is_poisoned(&self) -> bool {
        self.is_poisoned()
    }

    /// `topology::MAX_COUNT` for boundary tests (issue #163 test seam).
    #[cfg(test)]
    pub(crate) const TEST_MAX_ORDER_COUNT: u64 = topology::MAX_COUNT;

    /// `topology::try_pack` for helper-level width tests (issue #163 test seam).
    #[cfg(test)]
    pub(crate) fn test_try_pack(side: Option<Side>, count: usize) -> Result<u64, PriceLevelError> {
        topology::try_pack(side.map_or(topology::TAG_UNPINNED, topology::tag_of), count)
    }

    /// Direct call of the checked release (issue #163 test seam).
    #[cfg(test)]
    pub(crate) fn test_topology_release_one(&self) -> Result<bool, PriceLevelError> {
        self.topology_release_one()
    }

    /// Direct call of the post-removal release commit (issue #163 test seam),
    /// standing in for a removal whose count was consumed after validation.
    #[cfg(test)]
    pub(crate) fn test_release_after_removal(&self) -> Result<(), PriceLevelError> {
        self.release_after_removal()
    }

    /// Resting ids grouped by order-storage shard, in snapshot-walk order
    /// (issue #162 test seam); see `OrderQueue::debug_shard_runs`.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn test_shard_runs(&self) -> Vec<Vec<Id>> {
        self.orders.debug_shard_runs()
    }

    /// Rest `order` in the queue WITHOUT admission validation and without
    /// reserving it on the quantity / topology counters (issue #169 test seam).
    /// Used to place an order `add_order` would reject (e.g. a reserve whose
    /// visible + hidden overflows `u64`) so a test can drive
    /// [`OrderType::match_against`]'s typed error through the real sweep and
    /// the fill-or-kill dry run. The counters then describe only the admitted
    /// orders; tests assert they are unchanged by the failing step.
    #[cfg(test)]
    pub(crate) fn test_rest_unadmitted(&self, order: OrderType<()>) -> Result<(), PriceLevelError> {
        self.orders.try_push(Arc::new(order))
    }

    /// The true FIFO front `(sequence, id)` of the queue (issue #206 test
    /// seam); see `OrderQueue::test_front`.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn test_front(&self) -> Option<(u64, Id)> {
        self.orders.test_front()
    }

    /// Mutators currently announced to the fill-or-kill hand-off (issue #206
    /// test seam).
    #[cfg(test)]
    #[must_use]
    pub(crate) fn test_fok_waiting_mutators(&self) -> usize {
        self.fok_guard.test_waiting_mutators()
    }

    /// Announce a phantom mutator to the fill-or-kill hand-off until the
    /// returned value drops (issue #206 test seam).
    #[cfg(test)]
    pub(crate) fn test_fok_announce(&self) -> impl Drop + '_ {
        self.fok_guard.test_announce()
    }

    #[cfg(test)]
    pub(crate) fn test_poison_guard(&self) {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = self.fok_write();
            panic!("intentional guard poison for test");
        }));
    }

    /// Add an order to this price level.
    ///
    /// Decides the order's id IDENTITY first, then reserves its visible /
    /// hidden quantity and one count slot on the atomic counters **atomically
    /// with publishing** it to the queue, so an admission that reuses the id of
    /// an order already resting here — or that would overflow any counter — is
    /// rejected with nothing mutated: the queue, the counters, the statistics,
    /// and therefore any snapshot are left exactly as they were. On success the
    /// returned `Arc` is the admitted order.
    ///
    /// # Duplicate ids
    ///
    /// Publication goes through
    /// [`OrderQueue::try_push_with`](crate::price_level::OrderQueue) under the
    /// id-keyed map's per-shard lock, which decides the id is free **before**
    /// the counter reservation runs. Several concurrent submissions of the same
    /// id therefore resolve to exactly one admission; the rest return
    /// [`PriceLevelError::DuplicateOrderId`] **without touching any counter** —
    /// a rejected duplicate never overwrites the live order (which would leave
    /// the map and the ordered index disagreeing) and never transiently inflates
    /// a level counter.
    ///
    /// # Error precedence
    ///
    /// The order's own `visible + hidden` overflow is checked first (a pure
    /// property of the order). Then, under the shard lock, a **duplicate id is
    /// decided before any counter is touched**: an admission that both reuses a
    /// live id and would overflow a counter reports
    /// [`PriceLevelError::DuplicateOrderId`], never the overflow. Only for a
    /// free id is capacity reserved: the visible and hidden quantities with a
    /// checked [`AtomicU64::fetch_update`] (`checked_add`) — an atomic
    /// compare-exchange loop, free of the check-then-`fetch_add` TOCTOU race two
    /// admissions near `u64::MAX` would hit — and THEN the side pin and order
    /// count together in one compare-exchange (`topology_admit`), which also
    /// serializes concurrent opposite-side admissions. If the quantity
    /// reservations or the pin fail, the earlier ones are rolled back (by the
    /// exact delta this call added — a commutative, concurrency-safe undo on
    /// these advisory counters), so `try_push_with` publishes nothing
    /// and no counter is left drifted.
    ///
    /// # Topology invariants
    ///
    /// A level holds orders at exactly one price and one side. The order's price
    /// must equal the level's price, and its side must match the side of the
    /// orders already resting here (the first admitted maker pins the side; a
    /// fully drained level accepts either side again). Both are checked before
    /// any counter is touched, so a rejected order leaves the level unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::InvalidOperation`] if the order's price does
    /// not match the level's, if its side is incompatible with the resting
    /// side, if the order's own visible + hidden total overflows `u64`, or if
    /// admitting it would overflow the level's visible-quantity,
    /// hidden-quantity, or order-count counter; or
    /// [`PriceLevelError::DuplicateOrderId`] if an order with the same id
    /// already rests at this level. A duplicate id takes precedence over a
    /// counter overflow. Returns [`PriceLevelError::CounterExhausted`] if the
    /// level's topology or mutation epoch has no headroom left, or if the
    /// queue has no fresh FIFO sequence left (issue #165); a duplicate id also
    /// takes precedence over an exhausted sequence. In every case the level is
    /// unchanged.
    ///
    /// An exhausted `orders_added` statistic does NOT reject the admission:
    /// the order is admitted, the counter stays at `usize::MAX`, and the
    /// statistics are marked degraded (see
    /// [`PriceLevelStatistics::record_order_added`]).
    pub fn add_order(&self, order: OrderType<()>) -> Result<Arc<OrderType<()>>, PriceLevelError> {
        // Hold the fill-or-kill guard's shared side for this admission so a
        // concurrent fill-or-kill match sees a stable depth (issue #112). This
        // is an uncontended shared acquisition in the common case (no FOK).
        let _fok = self.fok_read();
        // Fail fast if a prior panic poisoned the guard (or this very acquisition
        // just recovered one): the level may be half-mutated (issue #130).
        self.poison_check()?;
        // Refuse, with nothing touched, when an epoch has no headroom left for
        // this admission's bumps (issue #165).
        self.check_epoch_headroom()?;

        // -------- Admission topology invariants (cheapest checks, no mutation) --------
        //
        // A level holds orders at exactly one price and one side. Reject a
        // mismatch BEFORE reserving any counter capacity, so the level is left
        // completely unchanged. Price is the cheapest check (two `u128`s), so it
        // goes first; the side is derived from whatever is already resting.
        if order.price().as_u128() != self.price {
            return Err(PriceLevelError::InvalidOperation {
                message: format!(
                    "order price {} does not match level price {}",
                    order.price().as_u128(),
                    self.price
                ),
            });
        }
        // The level's side is pinned in the topology word (issue #126): the
        // first maker pins it, later same-side makers join, and the drain that
        // empties the level un-pins it so a drained level accepts either side
        // again. This is a cheap EARLY reject of an opposite-side order against a
        // non-empty level, so the common mismatch never reserves counter
        // capacity. It is only an optimization — the AUTHORITATIVE, race-free
        // side decision is the pin CAS ([`Self::topology_admit`]) run inside the
        // reservation closure below, which serializes concurrent admissions.
        let order_side = order.side();
        if let Some(resting_side) = self.pinned_side()
            && order_side != resting_side
        {
            return Err(PriceLevelError::InvalidOperation {
                message: format!(
                    "order side {order_side:?} is incompatible with the level's resting side {resting_side:?}"
                ),
            });
        }

        // Calculate quantities.
        let visible_qty = order.visible_quantity().as_u64();
        let hidden_qty = order.hidden_quantity().as_u64();

        // Reject an order whose OWN visible + hidden total is not representable
        // in `u64`, before touching any counter. The level tracks visible and
        // hidden in two independent `u64` counters, so an order with, say,
        // visible `u64::MAX` and hidden `u64::MAX` would clear the per-counter
        // reservations below yet leave the level holding an order whose total
        // quantity overflows. Worse, the match sweep's reserve replenishment
        // computes `new_visible + drawn_hidden` (see `OrderType::match_against`),
        // where `drawn_hidden` is capped by the order's hidden tranche; that sum
        // is `<= visible + hidden`, so it can only overflow `u64` when the
        // order's own total already does. Enforcing the invariant here makes that
        // replenish add provably overflow-free for every admitted order.
        if visible_qty.checked_add(hidden_qty).is_none() {
            return Err(PriceLevelError::InvalidOperation {
                message: "order total quantity overflows u64".to_string(),
            });
        }

        // Publish through `try_push_with`, which decides id IDENTITY FIRST under
        // the DashMap shard lock and only then runs the counter reservation
        // below — atomically with the publication. Consequences:
        //
        // * A duplicate id is rejected with `DuplicateOrderId` and NOTHING
        //   touched: the reservation closure never runs, so a rejected duplicate
        //   cannot transiently inflate a counter, and a duplicate submitted at
        //   counter capacity reports `DuplicateOrderId` (identity) rather than a
        //   spurious overflow (the error precedence documented above).
        // * The reservation runs only once the id is known free; the queue then
        //   publishes the order into the map + index while holding the shard
        //   lock, so a concurrent cancel + readmission can never split this id
        //   across two index entries.
        //
        // Inside the closure, capacity is reserved visible → hidden with checked
        // `fetch_update` (an atomic CAS loop, free of the check-then-`fetch_add`
        // TOCTOU race two admissions near `u64::MAX` would hit), and THEN the
        // side is pinned and the count bumped in one CAS via `topology_admit`.
        // `Relaxed` on the advisory visible / hidden RMWs (issue #68); the pin
        // CAS uses `AcqRel` because side coherence is a hard invariant, not an
        // advisory counter. The pin goes LAST so it is only mutated on the
        // success path: an incompatible side or a count overflow returns `Err`
        // after rolling back the visible + hidden reservations this call made
        // (a commutative, concurrency-safe undo), leaving the topology word
        // untouched and `try_push_with` publishing nothing.
        let order_arc = Arc::new(order);
        self.orders.try_push_with(order_arc.clone(), || {
            if self
                .visible_quantity
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |c| {
                    c.checked_add(visible_qty)
                })
                .is_err()
            {
                return Err(PriceLevelError::InvalidOperation {
                    message: "price level visible quantity overflow on admission".to_string(),
                });
            }

            // Skip the hidden reservation entirely for a zero delta (issue
            // #145): every standard / post-only / pegged / trailing-stop /
            // market-to-limit order lands here with `hidden_qty == 0`.
            // `checked_add(0)` can never fail, so the skip changes no error
            // outcome or precedence, and the counter value is identical either
            // way. The skipped RMW carried no publication or happens-before
            // responsibility: EVERY operation on `hidden_quantity` in this crate
            // is `Relaxed` (this reservation, its rollbacks, the match-path
            // replenish / strand decrements, and the `hidden_quantity()` load),
            // so there is no Release store heading a release sequence that this
            // RMW would have extended, and no Acquire reader that could
            // synchronize with it. Publication of the order is done by
            // `try_push_with` (DashMap shard lock + SkipMap insert), and the
            // side / count ordering by the `AcqRel` pin CAS in
            // `topology_admit` below, both of which still run unchanged.
            if hidden_qty != 0
                && self
                    .hidden_quantity
                    .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |c| {
                        c.checked_add(hidden_qty)
                    })
                    .is_err()
            {
                // Roll back the visible reservation this call made.
                self.visible_quantity
                    .fetch_sub(visible_qty, Ordering::Relaxed);
                return Err(PriceLevelError::InvalidOperation {
                    message: "price level hidden quantity overflow on admission".to_string(),
                });
            }

            // Pin the side and bump the count in one CAS. This is the
            // authoritative, race-free side decision: two opposite-side
            // admissions into an empty level serialize here, only one wins.
            match self.topology_admit(order_side) {
                Ok(established) => {
                    if established {
                        // Pinned a previously-empty level; bump the epoch BEFORE
                        // the publish that `try_push_with` does next, so a
                        // snapshot whose walk spans this transition sees the epoch
                        // move and retries.
                        self.bump_topology_epoch();
                    }
                }
                Err(err) => {
                    // Roll back the visible + hidden reservations this call made;
                    // the topology word was not mutated (pin goes last).
                    self.visible_quantity
                        .fetch_sub(visible_qty, Ordering::Relaxed);
                    // A zero hidden delta was never reserved above (issue
                    // #145), so there is nothing to undo; skip the no-op RMW.
                    if hidden_qty != 0 {
                        self.hidden_quantity
                            .fetch_sub(hidden_qty, Ordering::Relaxed);
                    }
                    return Err(err);
                }
            }

            Ok(())
        })?;

        // Signal the committed mutation so a racing post-only depth scan retries
        // (issue #130).
        self.bump_mutation_epoch();

        // Update statistics only after a committed admission. The admission
        // stands even if the advisory `orders_added` counter is exhausted: the
        // statistics refuse to wrap it and mark themselves degraded (issue
        // #165), and the first such drop is logged after all bookkeeping.
        if let Some(err) =
            self.record_order_event(PriceLevelStatistics::record_order_added_reporting)
        {
            self.warn_order_event_dropped(&err);
        }

        Ok(order_arc)
    }

    /// Creates a non-allocating iterator over current orders in this level.
    ///
    /// The iteration order is not guaranteed to be stable. Use [`Self::snapshot_orders`]
    /// when deterministic ordering is required.
    ///
    /// # Caller-supplied code
    ///
    /// The iterator borrows the order storage lazily, so it holds a `DashMap`
    /// shard **read** lock between `next()` calls, while the caller's loop body
    /// and adapter closures run. While it is alive the caller must not mutate
    /// this level from the same thread (`add_order`, `update_order`,
    /// `match_order` on an order in a locked shard deadlocks), and should keep
    /// the body short: writers to that shard, including the matcher, wait on it.
    /// A panic in the body unwinds without mutating the level (the read lock is
    /// released and not poisoned). Use [`Self::snapshot_orders`] to run
    /// arbitrary code over the orders with no lock held (issue #172).
    pub fn iter_orders(&self) -> impl Iterator<Item = Arc<OrderType<()>>> + '_ {
        self.orders.iter_orders()
    }

    /// Materializes a deterministic snapshot of orders sorted by timestamp
    /// (ties broken by insertion sequence).
    ///
    /// # Errors
    ///
    /// [`PriceLevelError::CapacityExceeded`] (resource
    /// [`CapacityResource::OrderSnapshot`]) if the vector or its sort buffer
    /// cannot be reserved (issue #164). The level is only read.
    pub fn snapshot_orders(&self) -> Result<Vec<Arc<OrderType<()>>>, PriceLevelError> {
        self.orders.snapshot_vec()
    }

    /// Materializes the resting orders in the exact order [`Self::match_order`]
    /// consumes them: ascending **insertion sequence** (the oldest order first).
    ///
    /// Use this to predict the sweep — e.g. a self-trade-prevention pre-scan that
    /// must walk orders in consumption order to compute how much a taker may
    /// safely fill. It differs from the other two views:
    /// - [`Self::snapshot_orders`] sorts by `(timestamp, sequence)`, which equals
    ///   the sweep order *only* when timestamps are monotonic with insertion
    ///   (client-supplied or modify-restamped timestamps break that); and
    /// - [`Self::iter_orders`] has no stable order.
    ///
    /// Like `snapshot_orders`, this is a point-in-time view: a concurrent
    /// mutation after the call can change the queue.
    ///
    /// # Errors
    ///
    /// [`PriceLevelError::CapacityExceeded`] (resource
    /// [`CapacityResource::OrderSnapshot`]) if the vector or its sort buffer
    /// cannot be reserved (issue #164). The level is only read.
    pub fn snapshot_by_insertion_seq(&self) -> Result<Vec<Arc<OrderType<()>>>, PriceLevelError> {
        self.orders.snapshot_by_seq()
    }

    /// Fill `out` with the resting orders in ascending **insertion sequence** —
    /// the buffer-reuse variant of [`Self::snapshot_by_insertion_seq`].
    ///
    /// On success `out` is cleared and then extended in place, yielding the exact same
    /// sequence [`Self::snapshot_by_insertion_seq`] returns — the order
    /// [`Self::match_order`] consumes resting orders. Reusing one scratch
    /// buffer across calls avoids the per-call allocation of the returned
    /// `Vec`, which matters for a downstream consumer that walks every level
    /// repeatedly (e.g. a self-trade-prevention pre-scan). Note that an
    /// internal `(sequence, order)` pairs buffer plus its sort is still paid
    /// per call, so the reuse saves only the output `Vec` allocation.
    ///
    /// Like `snapshot_by_insertion_seq`, this is a point-in-time view: a
    /// concurrent mutation after the call can change the queue.
    ///
    /// # Errors
    ///
    /// [`PriceLevelError::CapacityExceeded`] (resource
    /// [`CapacityResource::OrderSnapshot`]) if the internal buffer or `out`
    /// cannot grow (issue #164). Every reservation is taken before `out` is
    /// cleared, so on `Err` `out` is exactly as the caller passed it.
    pub fn snapshot_by_seq_into(
        &self,
        out: &mut Vec<Arc<OrderType<()>>>,
    ) -> Result<(), PriceLevelError> {
        self.orders.snapshot_by_seq_into(out)
    }

    /// Returns `true` if any resting order has matchable depth, i.e. a positive
    /// taker would cross at this level.
    ///
    /// Used by the post-only pre-check, which only needs to know whether *any*
    /// liquidity would be taken, not how much. Short-circuits on the first
    /// matchable order, so it is cheaper than `matchable_quantity`.
    ///
    /// Matchability is delegated to [`OrderType::is_matchable`] — the single
    /// source of truth shared with the fill-or-kill dry run — so the post-only
    /// verdict and the fill-or-kill prediction can never disagree about the same
    /// level. In particular a zero-visible iceberg (or auto-replenishing
    /// reserve) backed by hidden quantity counts as matchable depth, because the
    /// sweep will draw that hidden into visible and fill it.
    ///
    /// A resting maker sharing `taker_id` is ignored: the sweep skips it for
    /// self-trade prevention, so it is not liquidity this taker could take, and
    /// the post-only pre-check must agree.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::CounterExhausted`] (counter
    /// [`ExhaustedCounter::MutationEpoch`]) when the mutation epoch is at its
    /// terminal sentinel (issue #165): the scan can then no longer be
    /// linearized, so no verdict is produced.
    fn has_matchable_depth(&self, taker_id: Id) -> Result<bool, PriceLevelError> {
        // Linearize the non-atomic scan against concurrent mutation (issue #130):
        // read the mutation epoch, scan, re-read; retry if it moved. A stable
        // epoch across the scan means no add / cancel / resize committed during
        // it, so the verdict corresponds to a single queue state and has a
        // linearization point (at the scan). Unbounded retry, same liveness
        // argument as the statistics seqlock: mutators are finite and each commit
        // is a single `fetch_add`, so a scan converges as soon as mutation
        // quiesces (the post-only path is cold, so the retry cost is irrelevant).
        loop {
            let epoch_before = self.mutation_epoch.load(Ordering::Acquire);
            if epoch_before == EPOCH_EXHAUSTED {
                // An exhausted epoch no longer moves, so a stable read would
                // prove nothing (issue #165).
                return Err(PriceLevelError::counter_exhausted(
                    ExhaustedCounter::MutationEpoch,
                ));
            }
            let verdict = self
                .iter_orders()
                .any(|order| order.id() != taker_id && order.is_matchable());
            std::sync::atomic::fence(Ordering::Acquire);
            let epoch_after = self.mutation_epoch.load(Ordering::Relaxed);
            if epoch_before == epoch_after {
                return Ok(verdict);
            }
        }
    }

    /// Computes how much of `incoming_quantity` this level could actually fill
    /// for a taker, in quantity units, **without mutating the queue**.
    ///
    /// This is a deterministic dry run of the FIFO sweep: it replays
    /// [`OrderType::match_against`] over the resting queue in the same
    /// price-time (insertion-sequence) order the real sweep uses, including
    /// iceberg / reserve
    /// replenishment (a refreshed tranche is re-queued at the tail) and the
    /// removal of a non-replenishing reserve once its visible part is drained.
    /// It also models the sweep's **replenish-headroom abort** (issue
    /// #124/#130): it tracks the level's visible counter as the sweep would
    /// evolve it and stops at the exact maker where a replenish's net delta would
    /// overflow `u64`, because the real sweep sets that maker aside and
    /// terminates. The returned value is therefore exactly what
    /// [`Self::match_order`] would consume — never an over- or under-count —
    /// which is what fill-or-kill (all-or-nothing) correctly depends on: without
    /// modelling the abort, this could approve a fill-or-kill the sweep then
    /// aborts mid-fill, leaving a partial fill.
    ///
    /// `taker_id` must be the id of the taker this depth is being computed for:
    /// a resting maker sharing that id is skipped, exactly as the real sweep
    /// skips it for self-trade prevention, so the prediction and the sweep can
    /// never diverge.
    ///
    /// This public call takes no guard, so a concurrent admission, update or
    /// sweep can land while it runs and the value is then an advisory
    /// estimate. It collects the resting orders from the id-keyed order
    /// storage (one entry per maker), sorts them by insertion sequence and
    /// replays the sweep over them, so a maker re-sequenced during the call
    /// (a GTC replenishment or a demoting resize) is counted at most once:
    /// the estimate can be stale but never double counts. That costs
    /// `O(depth log depth)` per call.
    ///
    /// The fill-or-kill preflight inside [`Self::match_order`] runs the same
    /// replay under the level's exclusive guard, where nothing can
    /// re-sequence, and there it walks the insertion-sequence index lazily
    /// and stops as soon as the taker is covered (issue #143): a fill within
    /// `max(8, resting orders / 64)` makers neither visits nor copies the
    /// makers behind it. A longer walk finishes over one collection and sort
    /// of the remaining makers, `O(depth log depth)` like this call.
    ///
    /// Public so an order book composing this level can reuse the single
    /// upstream source of truth for per-level fill-or-kill (all-or-nothing)
    /// feasibility instead of re-deriving the sweep, which would risk drifting
    /// from the real `match_order` behavior.
    ///
    /// # Errors
    ///
    /// [`PriceLevelError::CapacityExceeded`] (resource
    /// [`CapacityResource::OrderSnapshot`]) if the order collection
    /// (`additional` = resting orders) or the buffer of replenished tranches
    /// (`additional` = 1, only when a replenished tranche must be revisited)
    /// cannot grow (issue #164). No prediction is produced then: a silent `0`
    /// would under-report depth the sweep can in fact take. The level is only
    /// read. A maker step the real sweep would stop at (issue #169 / #163) is
    /// not an error here: the value is the prefix the sweep would fill.
    pub fn matchable_quantity(
        &self,
        incoming_quantity: u64,
        taker_id: Id,
    ) -> Result<u64, PriceLevelError> {
        Ok(self
            .dry_run(incoming_quantity, taker_id, DryRunIsolation::Unguarded)?
            .filled)
    }

    /// The deterministic dry run behind [`Self::matchable_quantity`]: returns
    /// both the quantity the sweep would fill and the exact number of trades
    /// it would emit, so the fill-or-kill preflight can reserve the result's
    /// storage before the first maker is touched (issue #170 / #164 contract).
    ///
    /// # Errors
    ///
    /// [`PriceLevelError::CapacityExceeded`] if the bulk continuation or the
    /// replenished-tranche buffer cannot grow (issue #164); the level is only
    /// read.
    fn dry_run(
        &self,
        incoming_quantity: u64,
        taker_id: Id,
        isolation: DryRunIsolation,
    ) -> Result<DryRun, PriceLevelError> {
        let mut dry = DryRun {
            filled: 0,
            trades: 0,
            replenishes: 0,
            parks: 0,
            error: None,
        };
        if incoming_quantity == 0 {
            return Ok(dry);
        }

        // Walk the resting orders in ascending insertion sequence: the exact
        // order the real sweep pops them (`match_front` walks the
        // insertion-sequence index with the same liveness rule as the
        // `SeqWalk` below). Visiting in the sweep's own order makes the
        // prediction exact per STEP, not only in total, so the trade count
        // below equals what the sweep emits (issue #170).
        //
        // Bounded work (issue #143): nothing is materialized or sorted up
        // front. The loop pulls one maker at a time and stops as soon as the
        // taker is satisfied (or the sweep would stop), so a taker filled
        // within the walk's lazy budget (`lazy_walk_budget`) visits only the
        // prefix the real sweep consumes and never touches the makers behind
        // it. Every count below (fill, trades, replenishes, parks, count
        // projection, stop error) is computed over exactly that prefix. A
        // walk that outlives the budget continues over one sorted bulk
        // collection of the remaining makers (see `SeqWalk`), which visits
        // the same sequence.
        //
        // The sweep's two re-queue shapes are modelled without touching the
        // walk: a pure partial fill keeps its sequence, so its residual is
        // visited next (`front`); a replenished tranche is re-sequenced at
        // the tail, BEHIND every maker resting now, so it is visited only
        // after the walk is exhausted (`tail`, FIFO among themselves). A
        // residual is only recorded while the taker still has quantity to
        // fill: otherwise the loop ends and it would never be visited.
        //
        // Growth (issue #164): the bulk continuation and `tail` grow
        // fallibly; a refusal of either is the only error this function
        // returns. Residuals are held by value, so no `Arc` is allocated for
        // them.
        let mut front: Option<OrderType<()>> = None;
        let mut tail: std::collections::VecDeque<OrderType<()>> = std::collections::VecDeque::new();
        let mut parks: usize = 0;
        let mut remaining = incoming_quantity;
        let mut filled: u64 = 0;
        let mut trades: usize = 0;
        let mut replenishes: u64 = 0;

        // Track the level's visible counter as the sweep would evolve it, so the
        // dry run models the #124 replenish-headroom ABORT (issue #130). A
        // replenish step commits `visible -= consumed; visible += hidden_reduced`
        // with checked arithmetic under the entry lock, and if that net delta
        // would exceed `u64::MAX` the real sweep SETS THE MAKER ASIDE (no trade)
        // and TERMINATES the sweep. Starting from the live counter, this
        // simulation reproduces that stop point exactly. When the fill-or-kill
        // dry run holds the `fok_write` guard, mutators are frozen, so the live
        // value cannot drift and the projection is exact — a would-abort here
        // therefore correctly makes the fill-or-kill infeasible (killed) rather
        // than approving a taker the sweep would abort mid-fill (a partial fill).
        let mut projected_visible = self.visible_quantity();

        // Track the resting-order count the same way (issue #163): the real
        // sweep validates `count >= 1` under the entry lock before it removes a
        // fully consumed maker and stops with a typed error (maker untouched)
        // if it is not. Projecting the count lets the dry run stop at that same
        // maker, so a fill-or-kill is killed before its first mutation instead
        // of failing mid-sweep. Exact under the fill-or-kill exclusive guard.
        let mut projected_count = topology::count(self.topology.load(Ordering::Acquire));

        // The lazy budget scales with the resting count read just above. The
        // lazy phase walks the index, where a concurrent re-sequencing can
        // briefly expose one maker under two keys, so it is only sound while
        // nothing can re-sequence (issue #143 review): under the fill-or-kill
        // guard. An unguarded walk starts in the bulk phase, which collects
        // from the id-keyed map (one entry per maker) and cannot double count.
        let lazy_budget = match isolation {
            DryRunIsolation::FokExclusive => lazy_walk_budget(projected_count),
            DryRunIsolation::Unguarded => 0,
        };
        let mut resting = self.orders.seq_walk(lazy_budget);

        while remaining > 0 {
            // Next maker in sweep order: the kept-priority residual, then the
            // resting queue, then the re-sequenced tranches. Deferred
            // initialization lets `order` borrow whichever holder was filled
            // without moving the (large) order into a common enum each step.
            let resting_order: Arc<OrderType<()>>;
            let residual: OrderType<()>;
            let order: &OrderType<()> = if let Some(kept) = front.take() {
                residual = kept;
                &residual
            } else if let Some(next) = resting.try_next()? {
                resting_order = next;
                &resting_order
            } else if let Some(requeued) = tail.pop_front() {
                #[cfg(test)]
                DRY_RUN_TAIL_REVISITED.with(|cell| cell.set(true));
                residual = requeued;
                &residual
            } else {
                break;
            };
            // Self-trade prevention parity: the real sweep skips a maker sharing
            // the taker id (`SelfTradeSkipped`), so the dry run must skip it too,
            // or fill-or-kill would predict depth the sweep will not take.
            if order.id() == taker_id {
                // The sweep parks the skipped maker's sequence (issue #164).
                match count_park(parks) {
                    Ok(count) => parks = count,
                    Err(err) => {
                        dry.error = Some(err);
                        break;
                    }
                }
                dry.parks = parks;
                continue;
            }
            // A typed arithmetic failure (issue #169) stops the real sweep at
            // this maker before it is mutated, keeping the fills committed so
            // far. Stop here too and record it, so the prediction (prefix and
            // error) is exactly what `match_order` reports.
            let (consumed, updated_order, hidden_reduced, new_remaining) =
                match order.match_against(remaining) {
                    Ok(step) => step,
                    Err(err) => {
                        dry.error = Some(err);
                        break;
                    }
                };

            // No-progress safety guard, identical in shape to the real sweep
            // (see `match_order`): a front maker that consumes nothing, draws no
            // hidden, and leaves `remaining` unchanged while handing itself back
            // is set aside (dropped from `pending`) rather than re-queued at the
            // front, which would spin forever. Dropping it here is the dry-run
            // analogue of the real sweep setting it aside: it contributes
            // nothing to `filled`, and the makers behind it are still visited.
            // Keeping this logic identical to the real sweep is what guarantees
            // `matchable_quantity` predicts exactly what `match_order` consumes,
            // which fill-or-kill depends on.
            if consumed == 0
                && hidden_reduced == 0
                && new_remaining == remaining
                && updated_order.is_some()
            {
                // The sweep parks the no-progress maker (issue #164).
                match count_park(parks) {
                    Ok(count) => parks = count,
                    Err(err) => {
                        dry.error = Some(err);
                        break;
                    }
                }
                dry.parks = parks;
                continue;
            }

            // A full consume removes the maker and releases one count; the real
            // sweep rejects that removal (typed error, maker untouched) when the
            // count is already zero. Mirror it: stop here with the same error
            // before counting this maker (issue #163).
            if updated_order.is_none() {
                match projected_count.checked_sub(1) {
                    Some(next) => projected_count = next,
                    None => {
                        dry.error = Some(topology_underflow(self.price));
                        break;
                    }
                }
            }

            // Evolve the projected visible counter exactly as the sweep will, and
            // STOP on a would-abort so this maker (and everything behind it)
            // contributes nothing — the same terminal state the real sweep
            // reaches (issue #130).
            if hidden_reduced > 0 {
                // Replenish: checked net delta `- consumed + hidden_reduced`. A
                // failure is the #124 abort: the maker is set aside untouched and
                // the sweep ends, so do NOT count `consumed` and break.
                match projected_visible
                    .checked_sub(consumed)
                    .and_then(|v| v.checked_add(hidden_reduced))
                {
                    Some(next) => projected_visible = next,
                    None => break,
                }
            } else {
                // Pure consume: visible only decreases, so it cannot abort. Track
                // it (checked, never wraps: `consumed <= projected_visible`) so a
                // later replenish's headroom test is accurate; a defensive
                // underflow stops the dry run conservatively.
                let Some(next) = projected_visible.checked_sub(consumed) else {
                    break;
                };
                projected_visible = next;
            }

            // `consumed <= remaining <= incoming_quantity`, so this sum cannot
            // overflow `u64`; checked anyway per the no-saturate/no-wrap rule.
            filled = match filled.checked_add(consumed) {
                Some(total) => total,
                None => break,
            };
            // The real sweep emits a trade exactly when `consumed > 0`.
            if consumed > 0 {
                trades = match trades.checked_add(1) {
                    Some(count) => count,
                    None => break,
                };
            }
            // A replenished maker that stays resident is re-sequenced at the
            // tail: the sweep reserves one fresh FIFO sequence for it (issue
            // #165), so fill-or-kill can check the sequence headroom up front.
            if hidden_reduced > 0 && updated_order.is_some() {
                replenishes = match replenishes.checked_add(1) {
                    Some(count) => count,
                    None => break,
                };
            }
            dry.filled = filled;
            dry.trades = trades;
            dry.replenishes = replenishes;
            remaining = new_remaining;

            if let Some(updated) = updated_order
                && remaining > 0
            {
                if hidden_reduced > 0 {
                    // Replenished tranche loses time priority -> behind every
                    // maker resting now, exactly as the real sweep re-queues it.
                    try_push_back_deque(&mut tail, updated, CapacityResource::OrderSnapshot)?;
                } else {
                    // Pure partial fill keeps front position.
                    front = Some(updated);
                }
            }
        }

        Ok(dry)
    }

    /// Matches an incoming taker order against existing orders at this price level.
    ///
    /// The sweep consumes resting makers in strict price-time (FIFO) order until
    /// the taker is filled or the matchable depth is exhausted. Trades are
    /// generated for each successful match, fully-consumed makers are removed,
    /// and the visible / hidden quantity counters and statistics are updated in
    /// lockstep with each execution.
    ///
    /// # Taker time-in-force / kind semantics
    ///
    /// Unlike earlier versions, this method **honors the taker's**
    /// [`TimeInForce`] and [`TakerKind`]. Let `available` be the quantity this
    /// level can actually fill for the taker (see `matchable_quantity`),
    /// capped at `incoming_quantity`:
    ///
    /// - [`TakerKind::PostOnly`]: must never take liquidity. If `available > 0`
    ///   the match is **rejected** — zero trades, the full `incoming_quantity`
    ///   reported as remaining, and the resting queue left untouched
    ///   ([`MatchResult::was_rejected`]).
    /// - [`TimeInForce::Fok`]: all-or-nothing. If `available < incoming_quantity`
    ///   the taker is **killed** — zero trades, full remaining, queue untouched
    ///   ([`MatchResult::was_killed`]). Otherwise it fills completely.
    /// - [`TimeInForce::Ioc`]: fills `available` and discards the remainder.
    ///   The taker is never enqueued here (this layer never rests a taker), so
    ///   the remainder is simply reported and dropped by the caller.
    /// - [`TimeInForce::Gtc`] / [`TimeInForce::Gtd`] / [`TimeInForce::Day`]:
    ///   fills `available`; the remainder is reported in
    ///   [`MatchResult::remaining_quantity`] for the order book to rest.
    /// - [`TakerKind::MarketToLimit`]: fills `available`; the remainder is
    ///   reported for the order book to convert into a resting limit. At this
    ///   single-level layer it fills like a standard taker.
    ///
    /// A post-only rejection and a fill-or-kill kill both leave zero trades and
    /// the full remainder; use [`MatchResult::outcome`] /
    /// [`MatchResult::was_rejected`] / [`MatchResult::was_killed`] to tell them
    /// apart from "the level had no liquidity".
    ///
    /// Time-in-force EXPIRY of resting **makers** is still NOT enforced here: a
    /// resting maker's `Gtd` / `Day` expiry is not consulted, so an expired
    /// maker still matches. Evicting or skipping expired makers is the
    /// caller's / order book's responsibility, keeping the match path a pure,
    /// deterministic sweep over the resting queue.
    ///
    /// # Arguments
    ///
    /// * `incoming_quantity`: The quantity of the incoming taker order to match.
    /// * `taker_order_id`: The ID of the incoming order (the "taker" order).
    /// * `taker_tif`: The taker's [`TimeInForce`], which governs how an unfilled
    ///   remainder is treated (kill / discard / rest).
    /// * `taker_kind`: The taker's [`TakerKind`] (standard / post-only /
    ///   market-to-limit).
    /// * `timestamp`: The taker timestamp (milliseconds since epoch) stamped
    ///   onto every emitted [`Trade`] and used as the execution time for
    ///   statistics. It is threaded in from the caller so the match path never
    ///   reads the wall clock — guaranteeing a deterministic, replayable trade
    ///   stream for a fixed input.
    /// * `trade_id_generator`: The [`UuidGenerator`] trade ids are reserved
    ///   from (one checked sequence value per emitted trade; see the failure
    ///   contract for exhaustion). It may be shared across levels and threads.
    ///
    /// [`Trade`]: crate::execution::Trade
    /// [`TimeInForce`]: crate::orders::TimeInForce
    /// [`TakerKind`]: crate::execution::TakerKind
    /// [`MatchResult::was_rejected`]: crate::execution::MatchResult::was_rejected
    /// [`MatchResult::was_killed`]: crate::execution::MatchResult::was_killed
    /// [`MatchResult::outcome`]: crate::execution::MatchResult::outcome
    /// [`MatchResult::remaining_quantity`]: crate::execution::MatchResult::remaining_quantity
    ///
    /// # Returns
    ///
    /// A `MatchResult` carrying the generated trades, the remaining unmatched
    /// quantity, the completion flag, the fully-filled maker IDs, and the
    /// terminal [`MatchOutcome`](crate::execution::MatchOutcome).
    ///
    /// # Failure contract (#164)
    ///
    /// This method never returns a bare error and never drops a committed
    /// fill. When a fallible step fails the sweep stops and the returned
    /// result carries the typed failure in
    /// [`MatchResult::error`](crate::execution::MatchResult::error). The stop
    /// causes are (numbered for reference, not in check order):
    ///
    /// 1. the maker's matching arithmetic ([`OrderType::match_against`],
    ///    #169);
    /// 2. the resting-order count refusing a full consume
    ///    ([`PriceLevelError::InvalidOperation`], #163);
    /// 3. growing the result's trade / filled-id storage
    ///    ([`PriceLevelError::CapacityExceeded`], resource `Trades` /
    ///    `FilledOrderIds`, #170; reserved before the step);
    /// 4. reserving a trade id from an exhausted [`UuidGenerator`]
    ///    (`CapacityExceeded`, resource `IdSequence`, #168);
    /// 5. no FIFO sequence left to re-sequence a replenished maker
    ///    ([`PriceLevelError::CounterExhausted`], #165);
    /// 6. recording a parked maker (self-trade skip): the parked-sequence set
    ///    cannot grow (`CapacityExceeded`, resource `SweepScratch`, #164). A
    ///    single live parked maker uses an inline slot that frees itself when
    ///    its key goes stale (cancel, readmission, demotion), so this needs
    ///    two simultaneously live parks, which no current order shape
    ///    produces;
    /// 7. after a committed step, a resting-order count release that fails
    ///    (#163) or the defensive post-lock replenish counter transition being
    ///    refused (#128 fallback, #164; unreachable today): both
    ///    **poison the level** ([`PriceLevelError::InvalidOperation`]).
    ///
    /// A replenishment whose drawn tranche would overflow the level's visible
    /// counter also stops the sweep, with the maker untouched, but sets no
    /// error: that liquidity is simply unreachable until headroom frees up.
    ///
    /// Callers **must** check `result.error()` before resting a taker's
    /// remainder: a stopped sweep's remainder is not "no more liquidity", and
    /// resting it after a self-trade race can duplicate an id at this level.
    ///
    /// - **Non-fill-or-kill takers.** The storage for each step is reserved
    ///   *before* the step's maker mutation is committed, so a growth failure
    ///   stops the sweep between two makers: the result holds every committed
    ///   trade (a strict FIFO prefix of the unconstrained sweep), the filled
    ///   ids of the makers that prefix removed, and the taker's true remaining
    ///   quantity; for causes 1 to 6 the queue, `visible` / `hidden` counters,
    ///   order count, side topology and statistics agree with exactly those
    ///   trades. For cause 7 the result is still exact, but the counters are
    ///   known to disagree with the queue: the level is poisoned (later
    ///   mutators return `InvalidOperation` and matching is refused) and the
    ///   caller must treat it as failed and reconstruct it from a snapshot. If
    ///   recording fails *after* a commit (ruled out by construction: the slot
    ///   is pre-reserved and the fill is validated by the sweep), the step's
    ///   level bookkeeping is still completed, `remaining_quantity` reflects the
    ///   committed fill, the unrecordable trade is logged at `ERROR` with all
    ///   its fields, and the sweep stops with the error set.
    ///   The trade id of each trade-emitting step is reserved from
    ///   `trade_id_generator` while the step is still a pure decision (under
    ///   the maker's entry lock, before the maker mutation or any counter delta
    ///   is committed). If the generator is exhausted the maker is left
    ///   untouched and the sweep stops the same way: committed prefix,
    ///   true remainder, consistent level, error set. Every later call against
    ///   crossable depth with that generator stops at its first fill with no
    ///   trades.
    /// - **Fill-or-kill takers.** The dry run under the exclusive guard
    ///   materializes the queue (fallibly) and counts the exact number of
    ///   trades, replenishments and parks the sweep will perform; the dry
    ///   run's own stop error (causes 1 and 2), the FIFO sequence headroom,
    ///   the result storage, the park set and exactly that many trade ids (all
    ///   or nothing) are checked or reserved before the first maker is
    ///   touched. A failure there kills the taker
    ///   ([`MatchResult::was_killed`]) with no trades, the full remaining
    ///   quantity, the level unchanged, and the error set; a successful
    ///   reservation leaves the sweep no fallible growth and no id
    ///   reservation on the shared generator.
    ///
    /// Resource failures are reported as the allocation-free
    /// [`PriceLevelError::CapacityExceeded`] (resource
    /// [`CapacityResource::IdSequence`]
    /// for trade-id exhaustion). Every stop and every fill-or-kill kill with
    /// an error is logged at `ERROR`, after the fill-or-kill guard is released
    /// and outside every queue lock.
    ///
    /// # Concurrency
    ///
    /// Resting orders are consumed in strict price-time (FIFO) order, and a
    /// partially-filled maker keeps its position at the front of the queue.
    ///
    /// **A concurrent `cancel` of the order currently being matched is safe and
    /// linearizable** (issue #81). The sweep keeps each maker resident in the
    /// queue and applies its decision (full consume / partial fill in place /
    /// replenish) while the maker's per-entry lock is held — the same lock a
    /// `cancel`'s removal takes (the internal `OrderQueue::match_front` step).
    /// The two therefore serialize: a cancel either fully wins (removes the
    /// maker and decrements the counters before the match observes it) or fully
    /// loses (the match commits first and the cancel then removes the residual
    /// it left). A cancel is never silently lost, and the counters never
    /// double-count. This closes the prior "lost cancel" window where a cancel
    /// landing between the matcher's pop and its reinsert would no-op while the
    /// matcher re-rested the residual.
    ///
    /// This method is **not lock-free**. Every maker it fills is committed
    /// under that maker's `DashMap` shard write lock (the serialization point
    /// above), which also blocks admissions and updates of other orders in the
    /// same shard for that step; a `Fok` taker additionally holds the
    /// level-wide fill-or-kill guard exclusively (below). Only the ordered
    /// index and the atomic counters it touches are lock-free.
    ///
    /// This method still assumes a **single logical matcher per level at a
    /// time**: two concurrent `match_order` calls on the *same* level are NOT
    /// made safe here and must be serialized by the caller (an order book
    /// typically matches a level from a single thread). Concurrent `add_order`
    /// / `update_order` from other threads is supported alongside that one
    /// matcher, including the single-side topology invariant (the side is
    /// pinned atomically; see the type-level note on [`PriceLevel`]).
    ///
    /// ## PostOnly and fill-or-kill are atomic with the sweep (issue #112)
    ///
    /// Both the post-only and the fill-or-kill decisions are made
    /// all-or-nothing with respect to concurrent `add_order` / `update_order`,
    /// by two different mechanisms:
    ///
    /// * **PostOnly never enters the sweep.** A positive post-only taker either
    ///   crosses resting depth (rejected) or does not (rests) — in NEITHER case
    ///   is any resting order consumed, and in neither case does it sweep.
    ///   Because there is no sweep, no concurrently-added maker can be turned
    ///   into a trade: "post-only emits zero trades" is a *structural* property
    ///   that holds under every interleaving, needing no lock. The verdict is
    ///   linearized at the `has_matchable_depth` read — depth committed before
    ///   it is crossed (reject); depth committed after it is ordered behind this
    ///   taker (rest), the correct decision at the read instant. A non-crossing
    ///   post-only taker leaves the level completely untouched; the caller
    ///   re-admits the residual via `add_order`. The post-only decision is
    ///   evaluated *before* the [`TimeInForce`] branch, so a post-only taker
    ///   never takes liquidity even when its TIF is `Fok` or `Ioc` — the
    ///   never-cross kind dominates the fill-or-kill / immediate-or-cancel
    ///   semantics.
    /// * **Fill-or-kill holds an exclusive guard across its dry-run and sweep.**
    ///   A positive fill-or-kill taker takes the write side of the level's
    ///   fill-or-kill guard (the `fok_guard` note on [`PriceLevel`]) before its
    ///   feasibility dry-run and holds it through the sweep, while `add_order` /
    ///   `update_order` take the shared side. No mutator can then change the
    ///   matchable depth between the dry-run and the sweep, so with a stable
    ///   queue the sweep consumes exactly what the dry-run predicted: a
    ///   fill-or-kill taker either fills in full or is killed with the queue and
    ///   counters untouched — never a partial fill. The guard is acquired only
    ///   for fill-or-kill; the ordinary (`Gtc` / `Ioc` / `Gtd` / `Day`) sweep
    ///   does not take this level-wide guard, but it still takes the per-maker
    ///   shard lock described above.
    ///
    /// # Statistics
    ///
    /// Per-level execution statistics are recorded **all-or-nothing** and can
    /// never fail the match: the trade is already committed when
    /// `PriceLevelStatistics::record_execution` runs. If recording overflows a
    /// statistics counter, that execution's contribution is dropped atomically
    /// (it advances every aggregate or none), the drop is logged at `WARN` (a
    /// recoverable anomaly, not an aborted match), and the level's
    /// `PriceLevelStatistics::stats_degraded` flag is set (sticky,
    /// snapshot-persisted) so the under-count is observable. The emitted trades
    /// and the `MatchResult` are unaffected (issue #117).
    ///
    /// # Caller-supplied code: `tracing` subscriber
    ///
    /// The level's payload is `()`, so matching runs no caller payload code.
    /// The only external code it can reach is the process-installed `tracing`
    /// subscriber, which runs synchronously on the calling thread at the
    /// `debug!` / `warn!` / `error!` events this method emits. The subscriber
    /// **must not panic** and must not call back into this level. The library
    /// arranges that no event is emitted while a `DashMap` shard lock is held
    /// or between a step's queue commit and its counter bookkeeping, and the
    /// fill-or-kill kill event is emitted after the exclusive guard is released
    /// (issue #172). It does **not** promise to recover from a subscriber
    /// panic: an event emitted mid-sweep unwinds with earlier steps' trades
    /// committed to the queue but the `MatchResult` reporting them lost, and a
    /// panic during a `Fok` sweep poisons the level (the guard is still held).
    /// It installs no panic hook and does not catch the unwind. See
    /// `doc/panic-boundaries.md`.
    pub fn match_order(
        &self,
        incoming_quantity: u64,
        taker_order_id: Id,
        taker_tif: TimeInForce,
        taker_kind: TakerKind,
        timestamp: TimestampMs,
        trade_id_generator: &UuidGenerator,
    ) -> MatchResult {
        // -------- Fail-fast on a poisoned level (issue #130) --------
        //
        // If a guard holder panicked mid-operation the level may be half-mutated
        // and cannot be trusted to match. `match_order` returns [`MatchResult`],
        // not `Result`, so it cannot surface `InvalidOperation` the way
        // `add_order` / `update_order` do; instead it REFUSES to match — an empty
        // result (no trades, full remaining) — which is the safe outcome (the
        // taker takes no liquidity from a corrupt level). The one-time `ERROR`
        // log was already emitted when the poison was first recovered.
        if self.is_poisoned() {
            return MatchResult::new(taker_order_id, Quantity::new(incoming_quantity));
        }

        // -------- Self-match is terminal (issue #126, tightening #120) --------
        //
        // If the taker's own id already rests at this level, the taker cannot
        // take liquidity here: matching would either self-trade (forbidden) or,
        // via the in-sweep skip, walk PAST its own resting order to trade with
        // OTHER makers — but issue #120's acceptance is that a self-match attempt
        // emits NO trades and leaves the level byte-identical. So reject
        // terminally, before any sweep, for EVERY TIF and kind. This check
        // precedes and therefore dominates the post-only / fill-or-kill
        // pre-checks below (a self-match `Fok` is Rejected, not Killed); the
        // post-only behaviour for a taker that does NOT rest here is unchanged.
        // The lookup is an O(1) id probe. The in-sweep `SelfTradeSkipped` path is
        // retained as documented defense-in-depth for the narrow race where the
        // taker's resting order is admitted AFTER this probe but during the
        // sweep — even then it must never self-trade.
        if incoming_quantity > 0 && self.orders.find(taker_order_id).is_some() {
            tracing::debug!(
                taker_order_id = %taker_order_id,
                incoming_quantity,
                price = self.price,
                "taker rejected: own order id already rests at this level (self-match)"
            );
            let mut result = MatchResult::new(taker_order_id, Quantity::new(incoming_quantity));
            result.mark_rejected(incoming_quantity);
            return result;
        }

        // -------- Taker TIF / kind pre-checks (before any queue mutation) --------
        //
        // PostOnly: must NEVER take liquidity (issue #112). A positive PostOnly
        // taker either crosses (reject) or does not (rest) — and in NEITHER case
        // does it enter the sweep. Not entering the sweep is what makes "PostOnly
        // emits zero trades" hold under *every* interleaving: no concurrent
        // `add_order` can turn a resting decision into a trade, because there is
        // no sweep to consume the newly-added depth. The verdict is linearized at
        // the `has_matchable_depth` check, which re-scans under the mutation epoch
        // so a concurrent add / cancel / resize racing the scan is DETECTED and
        // the scan retried (issue #130) — a stable epoch across the scan gives the
        // verdict a single-queue-state linearization point rather than a torn
        // read. Depth committed before the linearized scan is crossed (reject);
        // depth committed after it is ordered "later" (behind this taker), so
        // resting is correct at the scan instant. No guard is needed.
        if taker_kind.is_post_only() && incoming_quantity > 0 {
            let crossable = match self.has_matchable_depth(taker_order_id) {
                Ok(crossable) => crossable,
                Err(err) => {
                    // No linearizable verdict (issue #165). Rejecting is the
                    // safe side: the taker neither takes liquidity nor is told
                    // it may rest; the typed error says why.
                    tracing::error!(
                        taker_order_id = %taker_order_id,
                        incoming_quantity,
                        price = self.price,
                        error = %err,
                        "post-only taker rejected: depth scan cannot be linearized"
                    );
                    let mut result =
                        MatchResult::new(taker_order_id, Quantity::new(incoming_quantity));
                    result.mark_rejected(incoming_quantity);
                    result.set_error(err);
                    return result;
                }
            };
            // Deterministic race seam (issue #130): fires BETWEEN the depth
            // decision and the commit below so a test can inject an `add_order`
            // in that exact window and confirm PostOnly still emits zero trades
            // (there is no sweep to consume the added depth). No-op in production.
            #[cfg(test)]
            fire_post_only_decision_hook();
            if crossable {
                tracing::debug!(
                    taker_order_id = %taker_order_id,
                    incoming_quantity,
                    price = self.price,
                    "post-only taker rejected: would take liquidity"
                );
                let mut result = MatchResult::new(taker_order_id, Quantity::new(incoming_quantity));
                result.mark_rejected(incoming_quantity);
                return result;
            }
            // Not crossable: rest (report no trade) WITHOUT sweeping. The caller
            // re-admits the residual via `add_order`.
            return MatchResult::new(taker_order_id, Quantity::new(incoming_quantity));
        }

        // Fill-or-kill: all-or-nothing w.r.t. concurrent mutators (issue #112).
        // Take the fill-or-kill guard's EXCLUSIVE side and hold it across BOTH
        // the feasibility dry-run and the sweep below, so no `add_order` /
        // `update_order` can change the matchable depth between the two: with a
        // stable queue the sweep consumes exactly what the dry-run predicted, so
        // an FOK either fills in full or (insufficient depth) is killed with the
        // queue and counters untouched — never a partial fill. `_fok_guard` is
        // `Some` only for a positive FOK taker; it drops at the end of the
        // method (after the sweep). The non-FOK paths take no fill-or-kill guard.
        let is_fok = matches!(taker_tif, TimeInForce::Fok) && incoming_quantity > 0;

        // Epoch headroom (issue #165): a sweep bumps the topology epoch when it
        // drains the level, so it is refused BEFORE any maker is touched once
        // an epoch has no headroom left (see `bump_epoch`). Fill-or-kill is
        // killed and every other taker reports no trades; both carry the typed
        // error, and the level is unchanged.
        if incoming_quantity > 0
            && let Err(err) = self.check_epoch_headroom()
        {
            tracing::error!(
                taker_order_id = %taker_order_id,
                incoming_quantity,
                price = self.price,
                error = %err,
                "match refused before the sweep: level epoch exhausted; level untouched"
            );
            let mut result = MatchResult::new(taker_order_id, Quantity::new(incoming_quantity));
            if is_fok {
                result.mark_killed(incoming_quantity);
            }
            result.set_error(err);
            return result;
        }

        // Steps the fill-or-kill sweep will emit trades for, from the exact dry
        // run below (only meaningful when `is_fok`).
        let mut fok_trades: usize = 0;
        // The sweep's parked-sequence set (see the no-progress guard below).
        // Declared here so fill-or-kill can reserve it during its preflight;
        // `HashSet::new` does not allocate.
        let mut set_aside = ParkedSeqs::new();
        let fok_guard = if is_fok {
            #[cfg(test)]
            fire_pre_fok_lock_hook();
            let guard = self.fok_write();
            // Acquiring the write guard may have just recovered a poison; refuse
            // to match a half-mutated level rather than sweep it (issue #130).
            if self.is_poisoned() {
                return MatchResult::new(taker_order_id, Quantity::new(incoming_quantity));
            }
            #[cfg(test)]
            fire_fok_locked_hook();
            let dry = match self.dry_run(
                incoming_quantity,
                taker_order_id,
                DryRunIsolation::FokExclusive,
            ) {
                Ok(dry) => dry,
                Err(err) => {
                    // The dry run's working buffer could not be reserved
                    // (issue #164): no prediction, so kill before any
                    // mutation with the typed error. Guard released before
                    // logging, as for the sibling kills (issue #172).
                    drop(guard);
                    tracing::error!(
                        taker_order_id = %taker_order_id,
                        incoming_quantity,
                        price = self.price,
                        error = %err,
                        "fill-or-kill taker killed: dry-run working buffer could not be reserved; level untouched"
                    );
                    let mut result =
                        MatchResult::new(taker_order_id, Quantity::new(incoming_quantity));
                    result.mark_killed(incoming_quantity);
                    result.set_error(err);
                    return result;
                }
            };
            let available = dry.filled;
            fok_trades = dry.trades;
            if let Some(err) = dry.error {
                // The sweep would stop at a maker whose matching arithmetic
                // fails (issue #169) or whose full consume would underflow
                // the resting-order count (issue #163). The fill cannot be
                // complete, so kill the
                // taker BEFORE any mutation and report the typed error: level
                // untouched (#164 contract). Guard released before logging,
                // as below (issue #172).
                drop(guard);
                tracing::error!(
                    taker_order_id = %taker_order_id,
                    incoming_quantity,
                    available,
                    price = self.price,
                    error = %err,
                    "fill-or-kill taker killed: dry run stopped at a failing maker step; level untouched"
                );
                let mut result = MatchResult::new(taker_order_id, Quantity::new(incoming_quantity));
                result.mark_killed(incoming_quantity);
                result.set_error(err);
                return result;
            }
            if available < incoming_quantity {
                // Release the exclusive guard BEFORE emitting the event (issue
                // #172): the kill verdict is already decided and nothing was
                // mutated, so the guard protects nothing further. Logging under
                // it would run the process-installed `tracing` subscriber while
                // every mutator is excluded, and a panicking subscriber would
                // poison `fok_guard`, permanently refusing a level whose state
                // is in fact intact.
                drop(guard);
                tracing::debug!(
                    taker_order_id = %taker_order_id,
                    incoming_quantity,
                    available,
                    price = self.price,
                    "fill-or-kill taker killed: insufficient depth"
                );
                let mut result = MatchResult::new(taker_order_id, Quantity::new(incoming_quantity));
                result.mark_killed(incoming_quantity);
                return result;
            }
            // Fill-or-kill preflight order, all under the exclusive guard and
            // before any maker is touched: epoch headroom (above, #165),
            // dry-run stop error — the per-maker `match_against` error (#169)
            // or the projected resting-order count underflow on a full
            // consume (#163), whichever maker the dry run reaches first in
            // FIFO order — then depth, FIFO sequence
            // headroom for the dry run's replenishments (#165, here), exact
            // result storage (#170) and the trade-id block (#168, below).
            //
            // Every replenishment the sweep performs re-sequences a maker at
            // the tail and needs a fresh FIFO sequence (issue #165). Under the
            // exclusive guard no admission or update can take one, so the dry
            // run's count is exact: if the queue cannot supply that many, kill
            // now, before the first maker is touched, instead of stopping the
            // sweep part-way (which would be a partial fill-or-kill).
            if dry.replenishes > self.orders.seq_headroom() {
                drop(guard);
                let err = PriceLevelError::counter_exhausted(ExhaustedCounter::QueueSequence);
                tracing::error!(
                    taker_order_id = %taker_order_id,
                    incoming_quantity,
                    replenishes = dry.replenishes,
                    price = self.price,
                    error = %err,
                    "fill-or-kill taker killed: queue sequence exhausted; level untouched"
                );
                let mut result = MatchResult::new(taker_order_id, Quantity::new(incoming_quantity));
                result.mark_killed(incoming_quantity);
                result.set_error(err);
                return result;
            }
            // Every maker the sweep parks (self-trade skip, no-progress guard)
            // inserts one sequence into `set_aside` (issue #164). The count
            // can be nonzero: a concurrent mutator can admit a maker sharing
            // the taker id after the self-match lookup above and before this
            // guard was taken, and the dry run then parks it. Under the
            // exclusive guard the count is exact, so reserve it now: the
            // sweep's park then never has to grow the set, and a refusal kills
            // the taker here with the level untouched instead of stopping the
            // sweep part-way. The first park uses the set's inline slot, so
            // zero or one predicted park (the only counts current order shapes
            // produce) reserves nothing and does not allocate.
            if let Err(err) = set_aside.try_reserve(dry.parks) {
                drop(guard);
                tracing::error!(
                    taker_order_id = %taker_order_id,
                    incoming_quantity,
                    parks = dry.parks,
                    price = self.price,
                    error = %err,
                    "fill-or-kill taker killed: park set could not be reserved; level untouched"
                );
                let mut result = MatchResult::new(taker_order_id, Quantity::new(incoming_quantity));
                result.mark_killed(incoming_quantity);
                result.set_error(err);
                return result;
            }
            Some(guard)
        } else {
            None
        };

        // A single sweep emits at most one trade and at most one filled-order
        // id per resting order it actually consumes. Two independent upper
        // bounds hold: every emitted trade reduces `remaining` by at least one
        // unit (a `consumed == 0` maker is set aside without a trade), so the
        // sweep emits at most `incoming_quantity` trades; and it can touch at
        // most `order_count` resting orders. The tighter of the two pre-sizes
        // both vectors to cut per-fill reallocations on the hot path WITHOUT
        // reserving the whole level depth for a tiny taker (issue #106): a qty-1
        // taker against a deep level no longer reserves a multi-MB buffer it
        // immediately frees. The bound is advisory — `order_count` is read
        // `Relaxed` and both `Vec`s still grow if a concurrent `add_order` lands
        // mid-sweep — so it is a hint, not a cap.
        //
        // Both vectors deliberately share the one estimate (issue #148). A
        // partial fill or a replenishment emits a trade without a filled id,
        // but whether a step fully consumes is only known under the entry
        // lock, so a separate, smaller filled-id estimate would need a
        // lock-release-and-retry on the first full fill. Measured (BENCH.md,
        // "MatchResult capacity"): that saves one allocation on partial /
        // replenish fills (about -3% there) but costs about +13% on every
        // single full fill, the common path, so it was rejected.
        //
        // Allocation is fallible (issue #170, #164 contract):
        //
        // * Fill-or-kill reserves EXACTLY the number of trades the dry run
        //   predicted, before the first maker mutation. Under the exclusive
        //   guard the queue is frozen and the dry run replays the sweep step
        //   for step in the sweep's own (insertion-sequence) order, so the
        //   sweep emits exactly `fok_trades` trades and at most that many
        //   filled ids: every per-step reservation below is then a no-op and
        //   the sweep has no fallible growth left. A reservation failure here
        //   kills the taker with the level untouched and the error reported.
        // * Every other taker treats the pre-size as a hint: if it cannot be
        //   reserved the sweep starts from an empty result and the per-step
        //   reservation below is authoritative.
        // Trade ids pre-reserved for a fill-or-kill sweep (issue #168); `None`
        // for every other taker, which reserves one id per trade-emitting step.
        let mut fok_ids: Option<IdBlock> = None;
        let mut result = if is_fok {
            let mut result = MatchResult::new(taker_order_id, Quantity::new(incoming_quantity));
            if let Err(err) = result.try_reserve_exact(fok_trades) {
                // Nothing was mutated: release the guard before logging (the
                // same #172 rule as the insufficient-depth kill above).
                drop(fok_guard);
                tracing::error!(
                    taker_order_id = %taker_order_id,
                    incoming_quantity,
                    trades = fok_trades,
                    price = self.price,
                    error = %err,
                    "fill-or-kill taker killed: result storage could not be reserved; level untouched"
                );
                result.mark_killed(incoming_quantity);
                result.set_error(err);
                return result;
            }
            // Reserve EXACTLY `fok_trades` trade ids, all or nothing, before the
            // first maker mutation (issue #168). The sweep then draws each id
            // from this block instead of the shared generator, so an exhausted
            // (or nearly exhausted) generator kills the taker here with the
            // level untouched, rather than stopping a fill-or-kill sweep midway.
            // Reserved after the result storage: if that failed, no id was taken.
            match trade_id_generator.try_reserve_block(fok_trades) {
                Ok(block) => fok_ids = Some(block),
                Err(err) => {
                    drop(fok_guard);
                    tracing::error!(
                        taker_order_id = %taker_order_id,
                        incoming_quantity,
                        trades = fok_trades,
                        price = self.price,
                        error = %err,
                        "fill-or-kill taker killed: trade ids could not be reserved; level untouched"
                    );
                    result.mark_killed(incoming_quantity);
                    result.set_error(err);
                    return result;
                }
            }
            result
        } else {
            let capacity = sweep_capacity_hint(incoming_quantity, self.order_count());
            MatchResult::try_with_capacity(
                taker_order_id,
                Quantity::new(incoming_quantity),
                capacity,
            )
            .unwrap_or_else(|_| MatchResult::new(taker_order_id, Quantity::new(incoming_quantity)))
        };
        let mut remaining = incoming_quantity;
        // The failure that stops the sweep early (#164 contract), with the
        // committed trade that could not be recorded, if that is what failed.
        // Logged and stored on the result only after the sweep, once the step
        // bookkeeping is complete and the fill-or-kill guard is released.
        let mut sweep_error: Option<(PriceLevelError, Option<Trade>)> = None;

        // No-progress safety guard. A maker that yields no progress
        // (`consumed == 0`, re-queued unchanged, `remaining` not decreased)
        // must not be re-selected this sweep, or the loop would spin forever on
        // the same front order. Because such a dead order sits at the FRONT
        // (FIFO), simply breaking would starve any matchable makers behind it.
        // [`OrderQueue::match_front`] leaves a `SetAside` maker untouched in the
        // queue and parks its insertion sequence in `set_aside` so the sweep
        // advances to the maker behind it without re-selecting it. The maker is
        // never modified, never traded against, and its counters are never
        // touched, so the queue and the atomic counters stay exactly as if it
        // had been skipped — keeping counter <-> queue consistency intact and
        // preserving its price-time position for the next sweep.
        //
        // No current `OrderType` value can trigger this guard: for a positive
        // remainder `match_against` always consumes, draws hidden, or removes
        // the maker (pinned by `tests/parked_prefix.rs`, issue #155). It is
        // defense-in-depth against a future zero-progress shape. The only
        // parking that fires today is the self-trade skip. Id-keyed storage
        // limits it to one LIVE parked entry; stale parked keys left by a
        // cancel racing a readmission are dropped by `match_front` on first
        // encounter, so re-scanning from the front costs at most one extra
        // visit per step plus one per stale key (see BENCH.md).
        //
        // `set_aside` is declared above the fill-or-kill preflight. Its growth
        // is fallible (issue #164): `match_front` reserves a slot before it
        // parks, and a refusal (`FrontOutcome::ParkRefused`) stops the sweep
        // with the committed prefix and the typed error, since an unrecorded
        // park would re-select the same maker.

        // Per-step bookkeeping carried out of the locked decision closure. The
        // trade / stats / counter work is done AFTER the closure returns so it
        // is not performed while the per-entry lock is held; correctness vs a
        // concurrent cancel rides on the `FrontAction` the queue committed under
        // the lock (see `OrderQueue::match_front`), not on when these counters
        // move (they are advisory — issue #68).
        struct StepData {
            consumed: u64,
            hidden_reduced: u64,
            fully_consumed: bool,
            maker_id: Id,
            maker_side: crate::orders::Side,
            maker_price: u128,
            maker_timestamp: u64,
            /// Hidden quantity stranded by a full consume with no replenishment
            /// (drained reserve / leftover iceberg hidden), to subtract from the
            /// hidden counter.
            hidden_stranded: u64,
            /// The taker's remaining quantity after this maker is matched.
            new_remaining: u64,
            /// `true` when this step's level-counter deltas were already applied
            /// INSIDE the locked decision closure (the replenish path, issue
            /// #128). The post-lock body then skips re-applying them so the
            /// counters move exactly once.
            counters_committed: bool,
            /// The trade-id sequence value reserved for this step's trade
            /// (issue #168): `Some` exactly when `consumed > 0`, i.e. when the
            /// step emits a trade. Reserved under the entry lock BEFORE the
            /// maker mutation is committed.
            trade_seq: Option<u64>,
        }

        // Either the maker progressed (carrying `StepData`), was parked
        // (`SetAside` for the no-progress guard, `SelfTradeSkipped` for a maker
        // whose id equals the taker's), or forced the sweep to abort because
        // committing its replenishment would overflow the level's visible
        // counter (`Abort`). The parked / abort variants thread the maker's id
        // (and, where relevant, its insertion seq) OUT of the locked decision
        // closure so the caller's `warn!` / `debug!` can name the maker without
        // logging inside the per-entry lock.
        enum StepResult {
            Progressed(StepData),
            SetAside {
                maker_id: Id,
                seq: u64,
            },
            SelfTradeSkipped {
                maker_id: Id,
                seq: u64,
            },
            /// The FIFO-front maker would replenish, but moving the drawn hidden
            /// tranche into the level's visible counter would take it past
            /// `u64::MAX` — a depth the level cannot represent. The maker is left
            /// byte-identical (the queue action is `SetAside`, a pure no-op that
            /// mutates nothing) and the sweep terminates immediately, so no
            /// younger maker trades past this front and no counter wraps.
            Abort {
                maker_id: Id,
            },
            /// [`OrderType::match_against`] returned a typed arithmetic error
            /// for the FIFO-front maker (issue #169). The queue action is
            /// `SetAside` (a no-op that mutates nothing), so the maker rests
            /// unchanged; the sweep stops with the committed prefix and the
            /// error (#164 contract).
            Failed {
                maker_id: Id,
                error: PriceLevelError,
            },
            /// The FIFO-front maker would be fully consumed, but the
            /// under-lock resting-order count check refused its removal
            /// (issue #163). Same `SetAside` no-op and stop as `Failed`; the
            /// error is constructed after the entry lock is released.
            TopologyUnderflow {
                maker_id: Id,
            },
            /// The step would emit a trade but no trade id could be reserved
            /// (the generator is exhausted, issue #168). Detected BEFORE any
            /// mutation of the step: the maker is left byte-identical (queue
            /// action `SetAside`, no counter moved) and the sweep stops with the
            /// error set on the result.
            IdsExhausted {
                error: PriceLevelError,
            },
            /// The FIFO-front maker would replenish, but the queue has no fresh
            /// FIFO sequence left to re-sequence it at the tail (issue #165).
            /// The sequence is reserved BEFORE any counter moves, so the maker
            /// is left byte-identical (`SetAside`, a no-op) and the sweep stops
            /// with the committed prefix, per the #164 failure contract.
            SequenceExhausted {
                maker_id: Id,
                error: PriceLevelError,
            },
        }

        #[cfg(test)]
        if fok_guard.is_none() {
            fire_sweep_start_hook();
        }

        while remaining > 0 {
            // Reserve this step's trade + filled-id slots BEFORE `match_front`
            // commits any maker mutation (issue #170). With spare capacity (the
            // common case: the pre-size above) this is a length/capacity
            // compare and never allocates. When the pre-size is used up and
            // the queue is already drained, stop without growing: the next
            // `match_front` would report `Empty` anyway, and growing first
            // would add an allocation to every "taker larger than the level"
            // sweep. On a reservation failure nothing of this step has
            // happened yet, so stopping here leaves queue, counters and result
            // in agreement: the result reports exactly the fills committed so
            // far and the true remainder.
            if !result.has_step_capacity() {
                if self.orders.is_empty() {
                    break;
                }
                if let Err(err) = result.try_reserve(1) {
                    sweep_error = Some((err, None));
                    break;
                }
            }
            let outcome = self.orders.match_front(&mut set_aside, |seq, order_arc| {
                // Self-trade prevention, DEFENSE-IN-DEPTH (issue #126). The
                // common case is already handled terminally before the sweep: if
                // the taker id rests here, `match_order` returns `Rejected` with
                // no trades. This in-sweep skip covers only the narrow race where
                // the taker's own order is admitted AFTER that pre-check but
                // before the sweep reaches its slot. Deterministic in every build
                // profile (not a debug-only assert): a resting maker must never
                // trade against a taker carrying the same id. Skip it — park its
                // sequence like a no-progress maker so the sweep advances to the
                // makers behind it — rather than emit a self-trade. The maker is
                // left untouched (no trade, counters and queue unchanged).
                //
                // Scope: this is ORDER-ID identity — an order can never match
                // *itself*. It is NOT account/owner-level self-trade prevention:
                // two distinct order ids owned by the same `user_id` will still
                // trade here. Account-level STP is the composing order book's
                // responsibility (it knows the owner relationships this level
                // does not).
                if order_arc.id() == taker_order_id {
                    return (
                        FrontAction::SetAside,
                        StepResult::SelfTradeSkipped {
                            maker_id: order_arc.id(),
                            seq,
                        },
                    );
                }

                let (consumed, updated_order, hidden_reduced, new_remaining) =
                    match order_arc.match_against(remaining) {
                        Ok(step) => step,
                        Err(error) => {
                            return (
                                FrontAction::SetAside,
                                StepResult::Failed {
                                    maker_id: order_arc.id(),
                                    error,
                                },
                            );
                        }
                    };

                // Detect a non-progressing maker: nothing consumed, no hidden
                // drawn, the taker's remaining unchanged, and the maker handed
                // back to us to re-queue. Park it and advance. Thread the maker
                // id + seq out so the caller can name it in the no-progress
                // `warn!` without logging under the per-entry lock.
                if consumed == 0
                    && hidden_reduced == 0
                    && new_remaining == remaining
                    && updated_order.is_some()
                {
                    return (
                        FrontAction::SetAside,
                        StepResult::SetAside {
                            maker_id: order_arc.id(),
                            seq,
                        },
                    );
                }

                let maker_id = order_arc.id();
                let maker_side = order_arc.side();
                let maker_price = order_arc.price().as_u128();
                let maker_timestamp = order_arc.timestamp().as_u64();

                // Hidden stranded by a full consume that does not replenish:
                // an iceberg / reserve whose visible was fully taken but whose
                // hidden is dropped (non-auto reserve, or a leftover the
                // `match_against` chose not to refresh). Identical condition to
                // the pre-#81 sweep's full-consume cleanup branch.
                let hidden_stranded = if updated_order.is_none() && hidden_reduced == 0 {
                    match order_arc {
                        OrderType::IcebergOrder {
                            hidden_quantity, ..
                        }
                        | OrderType::ReserveOrder {
                            hidden_quantity, ..
                        } if hidden_quantity.as_u64() > 0 => hidden_quantity.as_u64(),
                        _ => 0,
                    }
                } else {
                    0
                };

                let fully_consumed = updated_order.is_none();

                // Pre-mutation check order for one step (issues #169, #163,
                // #168, #165, #124). Every check below runs under the entry
                // lock before this step commits anything, in this fixed order,
                // so the reported stop cause for a given queue state is
                // deterministic:
                //
                // 1. self-trade skip (parks, sweep continues);
                // 2. `match_against` error (#169) -> `Failed`;
                // 3. no-progress guard (parks, sweep continues);
                // 4. full-consume topology release validation: the
                //    resting-order count must be >= 1 before the removal
                //    (#163) -> `Failed`;
                // 5. trade-id reservation when `consumed > 0` (#168) ->
                //    `IdsExhausted`;
                // 6. FIFO sequence reservation when the maker replenishes and
                //    stays resident (#165) -> `SequenceExhausted`;
                // 7. replenish visible-counter headroom (#124) -> `Abort`.
                //
                // Steps 2, 4, 5, 6 and 7 each return `SetAside` (a no-op) and
                // stop the sweep with the committed prefix (#164 contract). A
                // value reserved by 5 or 6 for a step that a later check stops
                // is skipped, never reissued. The pure checks (1-4) come first,
                // so a count failure consumes no trade id or sequence; the two
                // reservations then precede the only in-closure counter RMW (7),
                // which is the first mutation of the step. Steps 4 and 6 are
                // mutually exclusive (a full consume never replenishes).
                //
                // Step 4: a full consume removes the maker and then releases one
                // resting-order count. A zero count means the count already
                // disagrees with the queue, so reject the step while it is still
                // a pure decision. The check runs here, in the same per-entry
                // critical section that then commits `FrontAction::Remove`, with
                // this maker resident and locked, so it cannot be misled by a
                // concurrent admission / cancellation (see
                // `topology_releasable`); the error is built after the lock is
                // released. A fill-or-kill never reaches this: its dry run
                // projects the same count and kills the taker before the first
                // mutation.
                if fully_consumed && !self.topology_releasable() {
                    return (
                        FrontAction::SetAside,
                        StepResult::TopologyUnderflow { maker_id },
                    );
                }

                // Reserve this step's trade id BEFORE anything of the step is
                // committed (issue #168): the replenish branch below publishes
                // counter deltas and the returned action mutates the maker, so
                // an exhausted generator must be detected here, while the step
                // is still a pure decision. Only a trade-emitting step
                // (`consumed > 0`) takes an id, so parked / non-trading steps
                // never consume sequence values. The reservation is one
                // allocation-free CAS (or a plain block take for fill-or-kill);
                // no event is emitted under the entry lock. A value reserved
                // here for a step that then aborts on visible-counter overflow
                // is skipped, never reissued (uniqueness is preserved). For a
                // fill-or-kill taker that stops early, the unused remainder of
                // its pre-reserved block is skipped the same way.
                let trade_seq = if consumed > 0 {
                    let reserved = match fok_ids.as_mut().and_then(IdBlock::take) {
                        Some(value) => Ok(value),
                        // Non-fill-or-kill steps (and, defensively, a
                        // fill-or-kill block the dry run under-counted) reserve
                        // from the shared generator.
                        None => trade_id_generator.try_reserve_one(),
                    };
                    match reserved {
                        Ok(value) => Some(value),
                        Err(error) => {
                            return (FrontAction::SetAside, StepResult::IdsExhausted { error });
                        }
                    }
                } else {
                    None
                };

                // Compute the action. For a replenishment, PUBLISH this step's
                // level-counter transition HERE — under the maker's entry lock,
                // before returning the action (issue #128) — so a concurrent
                // `UpdateQuantity` that next locks this same entry observes the
                // level counters already consistent with the replenished queue.
                // Applying it only after the entry released would leave the
                // counter transiently BELOW the queue's visible sum, and the
                // update's `old -> new` decrease could then underflow (`0 - 100`
                // wrap). `counters_committed` tells the post-lock body to skip
                // re-applying this step's deltas so the counters move exactly once.
                let mut counters_committed = false;
                let action = match updated_order {
                    None => FrontAction::Remove,
                    Some(updated) => {
                        if hidden_reduced > 0 {
                            // The refreshed tranche is re-sequenced at the tail:
                            // reserve that sequence FIRST (checked, issue #165),
                            // before any counter moves, so exhaustion leaves the
                            // maker and the counters untouched and stops the
                            // sweep with the committed prefix.
                            let reserved = match self.orders.try_reserve_seq() {
                                Ok(reserved) => reserved,
                                Err(error) => {
                                    return (
                                        FrontAction::SetAside,
                                        StepResult::SequenceExhausted { maker_id, error },
                                    );
                                }
                            };
                            // Replenishment: a fresh tranche moves hidden ->
                            // visible. Apply the visible NET delta
                            // (`- consumed + hidden_reduced`) as ONE checked RMW
                            // plus the hidden decrement, atomically visible before
                            // the entry lock releases. The checked `fetch_update`
                            // supersedes the old load-only `fits` pre-check: even
                            // when every resting order's own total fits `u64`, the
                            // level's visible SUM can exceed `u64::MAX` once hidden
                            // depth converts to visible, so a net delta that would
                            // overflow ABORTS the step (`SetAside` mutates nothing,
                            // emits no trade, ends the sweep) — no younger maker
                            // trades past this FIFO front and the counter never
                            // wraps. The stuck depth is unreachable until a cancel
                            // / downsize frees headroom.
                            let net_ok = self
                                .visible_quantity
                                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |c| {
                                    c.checked_sub(consumed)
                                        .and_then(|v| v.checked_add(hidden_reduced))
                                })
                                .is_ok();
                            if !net_ok {
                                return (FrontAction::SetAside, StepResult::Abort { maker_id });
                            }
                            self.hidden_quantity
                                .fetch_sub(hidden_reduced, Ordering::Relaxed);
                            counters_committed = true;
                            // Refreshed tranche loses priority.
                            FrontAction::ReplaceAtTail(Arc::new(updated), reserved)
                        } else {
                            // Pure partial fill: keep priority in place.
                            FrontAction::KeepInPlace(Arc::new(updated))
                        }
                    }
                };

                let data = StepData {
                    consumed,
                    hidden_reduced,
                    fully_consumed,
                    maker_id,
                    maker_side,
                    maker_price,
                    maker_timestamp,
                    hidden_stranded,
                    new_remaining,
                    counters_committed,
                    trade_seq,
                };

                (action, StepResult::Progressed(data))
            });

            // A refused park (issue #164) is only material for a step that
            // would continue the sweep past the parked maker; a terminal step
            // stops anyway and keeps its own error.
            let (step, mut park_error) = match outcome {
                FrontOutcome::Empty => break,
                FrontOutcome::Matched { result } => (result, None),
                FrontOutcome::ParkRefused { result, error } => (result, Some(error)),
            };
            {
                {
                    let data = match step {
                        StepResult::SetAside { .. } | StepResult::SelfTradeSkipped { .. }
                            if park_error.is_some() =>
                        {
                            // The maker was left untouched (a `SetAside` no-op)
                            // but its sequence could not be parked: continuing
                            // would re-select it. Stop with the committed
                            // prefix and the queue's original typed error;
                            // queue, counters and result agree.
                            if let Some(error) = park_error.take() {
                                sweep_error = Some((error, None));
                            }
                            break;
                        }
                        StepResult::SetAside { maker_id, seq } => {
                            // Parked by the queue; advance to the maker behind it.
                            // The id + seq were threaded out of the locked
                            // decision closure so we can name the parked maker
                            // here, outside the per-entry lock.
                            tracing::warn!(
                                price = self.price,
                                remaining,
                                order_id = %maker_id,
                                seq,
                                "match sweep: front maker made no progress; set aside to avoid re-pop"
                            );
                            continue;
                        }
                        StepResult::Abort { maker_id } => {
                            // The FIFO-front maker's replenishment would overflow
                            // the level's visible counter. The queue committed a
                            // no-op (`SetAside`), so the maker rests unchanged and
                            // no counter moved. Terminate the sweep here WITHOUT
                            // decrementing `remaining`: no trade is emitted for
                            // this maker, and stopping (rather than advancing to a
                            // younger maker) preserves strict FIFO — liquidity
                            // behind this front stays unreachable until a cancel /
                            // downsize frees enough headroom to represent the
                            // replenished depth.
                            tracing::error!(
                                price = self.price,
                                remaining,
                                order_id = %maker_id,
                                "match sweep aborted: replenishment would overflow the level visible counter; front maker left intact, sweep terminated"
                            );
                            break;
                        }
                        StepResult::Failed { maker_id, error } => {
                            // The front maker's matching arithmetic failed before
                            // it was mutated (issue #169): no trade, no counter
                            // moved, maker left in place. Stop with the committed
                            // prefix and report the error (#164 contract); it is
                            // logged after the sweep with the other stop causes.
                            tracing::debug!(
                                price = self.price,
                                remaining,
                                order_id = %maker_id,
                                "match sweep: front maker step failed before mutation; sweep stopped"
                            );
                            sweep_error = Some((error, None));
                            break;
                        }
                        StepResult::TopologyUnderflow { maker_id } => {
                            // The front maker's full consume failed the
                            // under-lock resting-order count check (issue #163):
                            // no trade, no counter moved, maker left in place.
                            // The error is built here, outside the entry lock,
                            // and reported with the committed prefix (#164).
                            tracing::debug!(
                                price = self.price,
                                remaining,
                                order_id = %maker_id,
                                "match sweep: front maker removal refused by the resting-order count; sweep stopped"
                            );
                            sweep_error = Some((topology_underflow(self.price), None));
                            break;
                        }
                        StepResult::SequenceExhausted { maker_id, error } => {
                            // No fresh FIFO sequence for the front maker's
                            // replenishment (issue #165). The queue committed a
                            // no-op and no counter moved, so queue, counters and
                            // result agree on the committed prefix. Stop here
                            // (#164 contract): advancing past this front would
                            // break FIFO. Logged with the result after the sweep.
                            tracing::debug!(
                                price = self.price,
                                remaining,
                                order_id = %maker_id,
                                "match sweep stopping: no FIFO sequence left to re-sequence the replenished front maker"
                            );
                            sweep_error = Some((error, None));
                            break;
                        }
                        StepResult::SelfTradeSkipped { maker_id, seq } => {
                            // Self-trade prevention: the front maker shares the
                            // taker's id. Skip it (parked like a set-aside maker)
                            // and advance to the makers behind it; no trade is
                            // emitted and the maker is left untouched.
                            tracing::debug!(
                                price = self.price,
                                remaining,
                                order_id = %maker_id,
                                seq,
                                "match sweep: front maker shares the taker id; skipped to prevent self-trade"
                            );
                            continue;
                        }
                        StepResult::IdsExhausted { error } => {
                            // No trade id could be reserved for this step. The
                            // queue committed a no-op (`SetAside`) and no counter
                            // moved, so the result already describes the level
                            // exactly: stop and report (logged after the sweep).
                            sweep_error = Some((error, None));
                            break;
                        }
                        StepResult::Progressed(data) => data,
                    };
                    let new_remaining = data.new_remaining;

                    // A statistics drop to report for this step. The event is
                    // emitted only AFTER every counter / topology delta of the
                    // step has been applied (issue #172): `tracing` dispatches
                    // synchronously into the process-installed subscriber, which
                    // is caller-supplied code, so logging mid-bookkeeping would
                    // let a panicking subscriber unwind with the maker already
                    // removed from the queue but `order_count` / the hidden
                    // counter not yet adjusted.
                    let mut stats_drop = None;

                    if let Some(trade_seq) = data.trade_seq {
                        // Update visible quantity counter. `Relaxed`: advisory
                        // counter (issue #68); the queue mutation committed
                        // inside `match_front` carries the real happens-before,
                        // not this RMW. The delta is keyed off the committed
                        // action so it never double-counts with a concurrent
                        // cancel (which decrements only the residual it removes).
                        // Skipped for a replenish step: its visible net delta
                        // (already including `- consumed`) was applied under the
                        // entry lock in the decision closure (issue #128).
                        if !data.counters_committed {
                            self.visible_quantity
                                .fetch_sub(data.consumed, Ordering::Relaxed);
                        }

                        // The id was reserved under the entry lock before the
                        // maker mutation; building its UUID is pure.
                        let trade_id = Id::from_uuid(trade_id_generator.uuid_for(trade_seq));

                        // A resting maker can never be the taker here: a maker
                        // sharing the taker id is skipped (`SelfTradeSkipped`)
                        // before it reaches this point, so no self-trade is ever
                        // emitted — deterministically, in every build profile.

                        let trade = Trade::with_timestamp(
                            trade_id,
                            taker_order_id,
                            data.maker_id,
                            Price::new(self.price),
                            Quantity::new(data.consumed),
                            data.maker_side.opposite(),
                            timestamp,
                        );

                        // The maker mutation is committed. Record the fill;
                        // the slots were reserved before `match_front`, so this
                        // cannot fail on growth, and the id / quantity checks
                        // hold by construction (the trade carries this taker's
                        // id and `consumed <= remaining`, with `remaining` kept
                        // in lockstep with the result). If it fails anyway we do
                        // NOT abandon the step: the bookkeeping below still runs
                        // so counters / topology / statistics match the queue,
                        // then the sweep stops with the error set (issue #170;
                        // this replaces a bare `break` that skipped it all).
                        match result.add_trade(trade) {
                            Ok(()) => {
                                if data.fully_consumed
                                    && let Err(err) = result.add_filled_order_id(data.maker_id)
                                {
                                    sweep_error = Some((err, None));
                                }
                            }
                            Err(err) => sweep_error = Some((err, Some(trade))),
                        }

                        // The trade is already committed (added to `result` and
                        // the queue mutated) — statistics recording cannot fail
                        // it retroactively. Recording is all-or-nothing (issue
                        // #117): on overflow the execution's contribution is
                        // dropped atomically and a sticky `stats_degraded` flag
                        // is set on the level's statistics (observable via
                        // `PriceLevelStatistics::stats_degraded`), leaving the
                        // trade stream unaffected. Log the drop rather than
                        // discard it silently.
                        // Read the degraded flag BEFORE recording: under the
                        // single-matcher-per-level model this is the faithful
                        // witness of the `false -> true` transition (issue #129).
                        // `record_execution` sets the flag atomically via
                        // `compare_exchange`; we log the WARN only on the FIRST
                        // drop that transitions it, so a burst of dropped
                        // executions logs once, not once per drop — the sticky
                        // flag remains the durable signal for the rest.
                        let was_degraded = self.stats.stats_degraded();
                        if let Err(err) = self.stats.record_execution(
                            data.consumed,
                            data.maker_price,
                            data.maker_timestamp,
                            timestamp.as_u64(),
                        ) && !was_degraded
                        {
                            stats_drop = Some(err);
                        }
                    }

                    remaining = new_remaining;

                    if data.fully_consumed {
                        // Maker fully consumed and removed inside `match_front`.
                        // Decrement the count and un-pin if this drained the level
                        // (issue #126); the removal already happened-before here.
                        // The release was validated before the removal (issue
                        // #163); if it still fails, the count disagreed with the
                        // queue beforehand: the level is poisoned and the sweep
                        // stops after this step's bookkeeping with the committed
                        // prefix (this fill included) and the typed error.
                        if let Err(err) = self.release_after_removal()
                            && sweep_error.is_none()
                        {
                            sweep_error = Some((err, None));
                        }
                        if data.hidden_stranded > 0 {
                            self.hidden_quantity
                                .fetch_sub(data.hidden_stranded, Ordering::Relaxed);
                        }
                    } else if data.hidden_reduced > 0 && !data.counters_committed {
                        // Replenishment: a fresh tranche moved from hidden into
                        // visible. The maker stayed resident (re-sequenced in
                        // place by `match_front`), so only the counters move.
                        // As of issue #128 the replenish path commits this
                        // transition under the entry lock (`counters_committed`),
                        // so this post-lock branch is unreachable today. It is
                        // kept as a defensive fallback that applies the
                        // transition when it fits and, when it does not, logs
                        // at ERROR, poisons the level and stops the sweep
                        // (below); it is not a no-op.
                        //
                        // Both transitions are checked (issue #165): unlike the
                        // proven committed deltas elsewhere in the sweep, this
                        // branch has no lock-held reservation behind it, so a
                        // move that does not fit is refused rather than
                        // wrapped (the hidden decrement is undone if the
                        // visible increment cannot land).
                        //
                        // A refusal is NOT silent (issue #164, following the
                        // #163 failed-rollback rule): the maker was already
                        // re-sequenced with its new split, so counters that
                        // could not follow no longer describe the queue. The
                        // level is poisoned and the sweep stops with the
                        // committed prefix (this fill included) and a typed
                        // error. The ERROR event below runs after this step's
                        // counter bookkeeping and outside every lock (the
                        // entry lock was released by `match_front`).
                        let hidden_moved = self
                            .hidden_quantity
                            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |h| {
                                h.checked_sub(data.hidden_reduced)
                            })
                            .is_ok();
                        let visible_moved = hidden_moved
                            && self
                                .visible_quantity
                                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
                                    v.checked_add(data.hidden_reduced)
                                })
                                .is_ok();
                        if hidden_moved && !visible_moved {
                            // Undo the hidden half. It re-adds units this step
                            // just took, but it is checked too: a refusal only
                            // deepens the disagreement the poison reports.
                            let _restored = self.hidden_quantity.fetch_update(
                                Ordering::Relaxed,
                                Ordering::Relaxed,
                                |h| h.checked_add(data.hidden_reduced),
                            );
                        }
                        if !visible_moved {
                            self.trip_poison();
                            tracing::error!(
                                price = self.price,
                                taker_order_id = %taker_order_id,
                                maker_order_id = %data.maker_id,
                                hidden_reduced = data.hidden_reduced,
                                hidden_moved,
                                "post-lock replenish counter transition refused; level poisoned — reconstruct it from a snapshot"
                            );
                            if sweep_error.is_none() {
                                sweep_error = Some((replenish_counter_failure(self.price), None));
                            }
                        }
                    }
                    // Pure partial fill (KeepInPlace, hidden_reduced == 0):
                    // visible already decremented by `consumed` above; the maker
                    // stays resident with its residual. Nothing else to do.

                    if let Some(err) = stats_drop {
                        // WARN, not ERROR: the match is not aborted — this is a
                        // recoverable observability anomaly flagged by the sticky
                        // degraded flag (the trade is committed). Emitted here,
                        // after the step's bookkeeping, per the note above.
                        tracing::warn!(
                            price = self.price,
                            taker_order_id = %taker_order_id,
                            maker_order_id = %data.maker_id,
                            consumed = data.consumed,
                            error = %err,
                            "execution statistics dropped (all-or-nothing); level stats marked degraded — trade unaffected"
                        );
                    }

                    if remaining == 0 || sweep_error.is_some() {
                        break;
                    }
                }
            }
        }

        result.finalize(Quantity::new(remaining));

        if let Some((err, lost_trade)) = sweep_error {
            // Release the fill-or-kill guard before running the subscriber
            // (issue #172); the level is already consistent.
            drop(fok_guard);
            match lost_trade {
                // A committed fill the result could not hold. Unreachable by
                // construction (see above); logged with every trade field so
                // it is never silent. `remaining` already reflects it.
                Some(trade) => tracing::error!(
                    price = self.price,
                    taker_order_id = %taker_order_id,
                    trade_id = %trade.trade_id(),
                    maker_order_id = %trade.maker_order_id(),
                    quantity = trade.quantity().as_u64(),
                    remaining,
                    error = %err,
                    "match sweep stopped: committed fill could not be recorded in the result"
                ),
                None => tracing::error!(
                    price = self.price,
                    taker_order_id = %taker_order_id,
                    trades = result.trades().len(),
                    remaining,
                    error = %err,
                    "match sweep stopped early; committed fills reported"
                ),
            }
            result.set_error(err);
        } else {
            drop(fok_guard);
        }

        result
    }

    /// Create a coherent snapshot of the current price level state.
    ///
    /// # What "coherent" means
    ///
    /// A returned snapshot is **coherent**: its `visible_quantity`,
    /// `hidden_quantity` and `order_count` are exactly the checked sums and the
    /// length of its own `orders` vector, every order's own visible + hidden
    /// total fits `u64`, and all orders rest on one side. The aggregates are
    /// folded from the collected vector, never read from the live atomic
    /// counters, so they can never disagree with the orders they describe.
    ///
    /// Coherent is **not** linearizable. The orders are collected by walking
    /// the `DashMap` shards one at a time with no transaction over the whole
    /// level, so under concurrent same-side admissions, cancels and resizes the
    /// vector may combine orders observed at different instants: each captured
    /// order is a real committed state of that order, but the set as a whole
    /// need not match any single instant of the level. Only a concurrent
    /// fill-or-kill match is excluded as a whole (see below). A caller that
    /// needs a point-in-time view must quiesce mutators first.
    ///
    /// # Recollection policy (bounded)
    ///
    /// A collected vector is rejected and recollected when either
    ///
    /// - a side transition raced the walk and left a mixed-side view (issue
    ///   #126), or
    /// - its aggregates cannot be represented: a same-side quantity transfer
    ///   between two shards (resize one order down, then another up, while the
    ///   walk sits between them) can capture both large values, so the collected
    ///   visible or hidden sum overflows `u64` although every committed level
    ///   state fits (issue #162).
    ///
    /// The walk is attempted at most `SNAPSHOT_MAX_ATTEMPTS` (8) times. Under
    /// sustained mutation that keeps defeating the walk, the call does not loop
    /// indefinitely: it returns [`PriceLevelError::InvalidOperation`] and the
    /// caller decides whether to retry later. Once such mutation pauses, the
    /// next attempt collects a coherent vector. A failed call has no side
    /// effects on the level; a successful one never substitutes a live counter
    /// or any other value for an aggregate.
    ///
    /// # Fill-or-kill exclusion
    ///
    /// Every attempt runs under the fill-or-kill guard's shared side, so a
    /// multi-maker fill-or-kill match is observed either entirely before or
    /// entirely after the snapshot, never mid-sweep. The guard is not
    /// poison-checked here: a snapshot stays available on a poisoned level for
    /// diagnostics and reconstruction.
    ///
    /// # Order
    ///
    /// The `orders` vector is materialized in **queue-consumption order**
    /// (ascending insertion sequence — the exact order [`Self::match_order`]
    /// consumes resting orders), not the `(timestamp, sequence)` display order
    /// of [`Self::snapshot_orders`]. Because [`Self::from_snapshot`] re-enqueues
    /// in vector order, a restore reproduces the live queue's priority exactly —
    /// including the "sizing an order up loses time priority" demotion, where an
    /// order that was moved to the back of the queue keeps its original
    /// admission timestamp. Using the timestamp view here would let such an
    /// order sort back to its old position and wrongly regain front priority on
    /// restore.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::InvalidOperation`] if no attempt within the
    /// bounded recollection policy above collected a coherent vector (a
    /// mixed-side view or an aggregate that overflows `u64` on every attempt),
    /// or [`PriceLevelError::CapacityExceeded`] (resource
    /// [`CapacityResource::OrderSnapshot`]) at once, without recollecting, if
    /// the orders vector cannot be reserved (issue #164). The level is left
    /// unchanged.
    pub fn snapshot(&self) -> Result<PriceLevelSnapshot, PriceLevelError> {
        // Hold the fill-or-kill guard's SHARED side across every attempt (issue
        // #130) so a snapshot can never capture a multi-maker fill-or-kill
        // mid-transaction: the FOK holds the EXCLUSIVE side across its dry-run and
        // sweep, so this read waits for it to fully commit or is excluded before
        // it starts — the snapshot sees the pre- or post-FOK state, never a
        // partial sweep. Ordinary mutators (`add_order` / `update_order`) also
        // take the shared side, so they run concurrently with this read (read vs
        // read) and are handled by the bounded recollection below.
        // `snapshot` intentionally does NOT poison-check: it stays available on a
        // poisoned level for diagnostics / reconstruction.
        let _fok = self.fok_read();

        let mut last_rejection: Option<PriceLevelError> = None;

        for attempt in 1..=SNAPSHOT_MAX_ATTEMPTS {
            #[cfg(test)]
            crate::price_level::order_queue::snapshot_hook::fire(
                crate::price_level::order_queue::snapshot_hook::SnapshotHookEvent::AttemptStart,
            );

            // Materialize the orders in queue-consumption (insertion sequence)
            // order so a snapshot round-trip re-enqueues them in identical
            // priority order.
            //
            // Guard against a TORN topology (issue #126): a walk that spans a
            // drain-then-re-admit to the opposite side could capture old-side and
            // new-side orders together, producing a checksummed snapshot that
            // `from_snapshot` would reject for mixed sides. `topology_epoch` is
            // bumped on every side pin / un-pin, so a stable epoch implies a
            // single-side walk; a moved epoch is only rejected when the walk
            // actually came back mixed-side.
            let epoch_before = self.topology_epoch.load(Ordering::Acquire);
            //
            // The materialization is fallible (issue #164). A refused
            // reservation is not a torn view, so it is not recollected: it
            // is returned at once, with nothing logged (the subscriber could
            // allocate) and the level untouched.
            let orders = self.snapshot_by_insertion_seq()?;
            let epoch_after = self.topology_epoch.load(Ordering::Acquire);
            // An exhausted epoch no longer moves (issue #165), so treat it as
            // "moved": the structural single-side check then decides.
            let epoch_moved = epoch_before != epoch_after || epoch_before == EPOCH_EXHAUSTED;
            if epoch_moved && !Self::is_single_side(&orders) {
                tracing::debug!(
                    price = self.price,
                    attempt,
                    "snapshot walk captured a mixed-side view across a side transition; recollecting"
                );
                last_rejection = Some(snapshot_mixed_side());
                continue;
            }

            // Every aggregate is folded from this same vector with checked
            // arithmetic (issue #162). The walk has no transaction over the whole
            // level, so a same-side quantity transfer between two shards can make
            // the collected sums overflow although every committed state fits:
            // reject that vector and recollect instead of asserting or
            // substituting a live counter.
            match SnapshotAggregates::from_orders(&orders) {
                Ok(aggregates) => {
                    // Persist the per-level statistics alongside the aggregates
                    // so the snapshot round-trip reproduces the recorded
                    // execution history. The clone reads the atomic counters
                    // (best-effort, like every other read path); statistics are
                    // independent counters, not part of the order aggregates
                    // this snapshot guarantees coherent.
                    return Ok(PriceLevelSnapshot::from_raw_parts_with_stats(
                        Price::new(self.price),
                        aggregates.visible_quantity,
                        aggregates.hidden_quantity,
                        aggregates.order_count,
                        orders,
                        (*self.stats).clone(),
                    ));
                }
                Err(err) => {
                    tracing::debug!(
                        price = self.price,
                        attempt,
                        error = %err,
                        "snapshot walk collected aggregates that do not fit u64; recollecting"
                    );
                    last_rejection = Some(err);
                }
            }
        }

        let err = snapshot_attempts_exhausted(self.price, last_rejection);
        tracing::warn!(
            price = self.price,
            attempts = SNAPSHOT_MAX_ATTEMPTS,
            error = %err,
            "snapshot could not collect a coherent view; level unchanged"
        );
        Err(err)
    }

    /// Serialize the current price level state into a checksum-protected snapshot package.
    ///
    /// The checksum stage streams the snapshot's canonical JSON straight into
    /// SHA-256 (issues #149 / #164): it builds no payload buffer and no
    /// order-reference vector. Capturing the snapshot still allocates: the
    /// ordered walk collects a temporary `(sequence, Arc)` pairs buffer, sorts
    /// it, and copies it into the snapshot's output `Arc` vector, so both are
    /// live at the capture peak.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::InvalidOperation`] if [`Self::snapshot`]
    /// cannot collect a coherent view within its bounded recollection policy,
    /// [`PriceLevelError::CapacityExceeded`] if the orders vector or the hex
    /// checksum cannot be reserved (issue #164), or
    /// [`PriceLevelError::SerializationError`] if encoding the snapshot
    /// payload to compute its SHA-256 checksum fails.
    pub fn snapshot_package(&self) -> Result<PriceLevelSnapshotPackage, PriceLevelError> {
        PriceLevelSnapshotPackage::new(self.snapshot()?)
    }

    /// Serialize the current price level state to JSON, including checksum metadata.
    ///
    /// # Serialization passes (issue #149)
    ///
    /// This still serializes the snapshot **twice**: once streamed into
    /// SHA-256 to compute the checksum (no temporary payload buffer), then
    /// again into the returned package JSON. Streaming the hash removed the
    /// temporary checksum buffer, not the second pass. Because the package
    /// fields are written in `version`, `snapshot`, `checksum` order, a
    /// single-pass encoder could forward the snapshot bytes to both the output
    /// and SHA-256 and append the checksum afterwards without changing the
    /// package bytes; it is not implemented here and still needs a separate
    /// compatibility review.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::InvalidOperation`] if [`Self::snapshot`]
    /// cannot collect a coherent view within its bounded recollection policy,
    /// [`PriceLevelError::CapacityExceeded`] if the orders vector, the hex
    /// checksum or the JSON output cannot be reserved (issue #164), or
    /// [`PriceLevelError::SerializationError`] if the package cannot be
    /// encoded to JSON.
    pub fn snapshot_to_json(&self) -> Result<String, PriceLevelError> {
        self.snapshot_package()?.to_json()
    }
}

impl PriceLevel {
    /// Apply an update to an existing order at this price level.
    ///
    /// # Quantity-update priority policy
    ///
    /// For [`OrderUpdate::UpdateQuantity`] (and the same-price branch of
    /// [`OrderUpdate::UpdatePriceAndQuantity`]) this method follows the
    /// conventional exchange price-time-priority rules:
    ///
    /// - **Decrease or unchanged total quantity** keeps the maker's queue
    ///   position. The stored order is updated *in place* at its existing
    ///   insertion sequence, so it is consumed at the same point in FIFO order
    ///   as before. Reducing size never forfeits time priority.
    /// - **Increase in total quantity** demotes the order to the *back* of the
    ///   queue (it is assigned a fresh insertion sequence). Sizing an order up
    ///   loses time priority, matching standard exchange behaviour.
    ///
    /// Total quantity is `visible + hidden`; the branch is chosen by comparing
    /// the order's total before and after the update. Every order variant is
    /// resized by [`OrderType::with_reduced_quantity`] (single-quantity
    /// variants rewrite their `quantity`; two-tranche variants rewrite the
    /// visible tranche and keep hidden), so the branch reflects the real size
    /// change rather than a silent no-op.
    ///
    /// # Applied to the live maker (issue #115)
    ///
    /// The resize, the priority decision, and the level-counter update are all
    /// derived from the order **currently resident in the queue**, computed and
    /// committed together under a single per-entry lock — never from a pre-read.
    /// A concurrent match or replenishment that commits first is therefore fully
    /// reflected: an update applies `new_quantity` to the *live* visible tranche
    /// and preserves the *live* hidden depth, so it can never resurrect executed
    /// or cancelled quantity, and the increase-vs-decrease policy is chosen from
    /// the live total (never stale). The level counters are validated with
    /// checked math before the queue mutates, so an update that would overflow a
    /// level counter is rejected with the level left unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::InvalidOperation`] if an
    /// [`OrderUpdate::UpdatePrice`] / [`OrderUpdate::Replace`] would not move
    /// the order to a different price level, if computing an order's total
    /// quantity overflows `u64`, or if an [`OrderUpdate::UpdateQuantity`] would
    /// overflow the level's visible- or hidden-quantity counter (the maker and
    /// its queue position are left unchanged in that case). Returns
    /// [`PriceLevelError::CounterExhausted`] if the level's topology or
    /// mutation epoch has no headroom left, or if a quantity increase needs a
    /// fresh FIFO sequence and none is left (issue #165); the level is
    /// unchanged in both cases.
    ///
    /// An exhausted `orders_removed` statistic does NOT fail a committed
    /// removal: the counter stays at `usize::MAX` and the statistics are
    /// marked degraded (see [`PriceLevelStatistics::record_order_removed`]).
    #[must_use = "the updated order (or None when the order is absent) must be handled"]
    pub fn update_order(
        &self,
        update: OrderUpdate,
    ) -> Result<Option<Arc<OrderType<()>>>, PriceLevelError> {
        // Hold the fill-or-kill guard's shared side for the whole update so a
        // concurrent fill-or-kill match cannot observe the depth shrink (cancel
        // / down-size) or grow mid-decision (issue #112). Uncontended in the
        // common case (no FOK running). Acquired exactly ONCE here: the
        // same-price branches of `UpdatePriceAndQuantity` / `Replace` delegate
        // to the guard-free `update_order_inner` rather than recursing into this
        // method, because std `RwLock` reads are NOT reentrant. A recursive
        // `read()` with a writer queued between the two acquisitions (the lock
        // is writer-preferring, so the queued writer blocks the second reader)
        // would deadlock against a `fok_write` waiting on the first reader.
        let _fok = self.fok_read();
        // Fail fast on a poisoned level (issue #130).
        self.poison_check()?;
        // Refuse, with nothing touched, when an epoch has no headroom left for
        // this update's bumps (issue #165).
        self.check_epoch_headroom()?;
        let mut stats_drop = None;
        let result = self.update_order_inner(update, &mut stats_drop);
        // A committed mutation (`Ok(Some(_))` — the order was found and
        // cancelled / resized / moved) bumps the mutation epoch so a racing
        // post-only depth scan retries (issue #130). `Ok(None)` (not found) and
        // `Err` change nothing, so they do not bump.
        if matches!(result, Ok(Some(_))) {
            self.bump_mutation_epoch();
        }
        // An exhausted `orders_removed` statistic never fails the committed
        // removal (issue #165); its first drop is logged after the bookkeeping.
        if let Some(err) = stats_drop {
            self.warn_order_event_dropped(&err);
        }
        result
    }

    /// Remove a resting order for a cancel / price-moving update and release
    /// its level accounting (issue #163).
    ///
    /// Protocol (event boundaries):
    ///
    /// 1. **Select + validate + remove, one critical section.**
    ///    [`OrderQueue::remove_if`] selects the occupied entry and, while
    ///    holding that entry's shard write lock, checks that the resting-order
    ///    count is at least one ([`Self::topology_releasable`]) and removes the
    ///    entry. The check and the removal observe the same state: this order
    ///    is resident, so its own admission count is included (admission
    ///    counts before it publishes) and nobody else can release it (every
    ///    remover of this id needs this lock). A concurrent admission of the
    ///    same id on an empty level therefore either publishes before the
    ///    selection (the cancel finds it, with its count) or after it (the
    ///    cancel reports `Ok(None)`) — never a spurious count error. An
    ///    absent id returns `Ok(None)` without evaluating the count or
    ///    constructing an error.
    /// 2. **Counters and release, after the lock.** The quantity counters move
    ///    and the release commits through [`Self::release_after_removal`].
    /// 3. **Events, last.** Any `warn!` / `error!` is emitted after the entry
    ///    lock is released; nothing is logged inside `remove_if`.
    ///
    /// The removal stays the single per-entry `DashMap` removal of issue
    /// #119, so a concurrent match or cancel of the same id still resolves to
    /// exactly one winner.
    ///
    /// Returns `Ok(None)` when the id is not resting here (nothing changes).
    ///
    /// # Errors
    ///
    /// [`PriceLevelError::InvalidOperation`] when the count is zero while the
    /// order rests (nothing mutated), or — reachable only if the count
    /// disagreed with the queue before the call and a concurrent removal of a
    /// DIFFERENT order consumed the last count after the check — when the
    /// post-removal release fails; the removal is then committed, the level
    /// is poisoned (fail fast, reconstruct from a snapshot) and the error is
    /// returned rather than a success.
    fn remove_resting(&self, order_id: Id) -> Result<Option<Arc<OrderType<()>>>, PriceLevelError> {
        let order = match self
            .orders
            .remove_if(order_id, |_resident| self.topology_releasable())
        {
            RemoveOutcome::Absent => return Ok(None),
            RemoveOutcome::Refused => {
                // Built and logged after the entry lock was released.
                let err = topology_underflow(self.price);
                tracing::warn!(
                    price = self.price,
                    order_id = %order_id,
                    error = %err,
                    "removal rejected before mutation: resting-order count disagrees with the queue"
                );
                return Err(err);
            }
            RemoveOutcome::Removed(order) => order,
        };

        // Update atomic counters from the order actually removed from the
        // queue above. `Relaxed` on both: advisory counters (issue #68); the
        // `OrderQueue::remove` carries the happens-before, not these counters.
        self.visible_quantity
            .fetch_sub(order.visible_quantity().as_u64(), Ordering::Relaxed);
        self.hidden_quantity
            .fetch_sub(order.hidden_quantity().as_u64(), Ordering::Relaxed);

        // Decrement the count and un-pin if this drained the level (issue
        // #126); the `remove` above happened-before.
        if let Err(err) = self.release_after_removal() {
            tracing::error!(
                price = self.price,
                order_id = %order_id,
                error = %err,
                "resting-order count underflow after a committed removal; level poisoned — reconstruct it from a snapshot"
            );
            return Err(err);
        }

        Ok(Some(order))
    }

    /// Guard-free body of [`Self::update_order`].
    ///
    /// The caller MUST already hold the fill-or-kill shared guard
    /// ([`Self::fok_read`]); this method never acquires it. That is what lets
    /// the same-price `UpdatePriceAndQuantity` / `Replace` branches re-enter it
    /// without taking a second, non-reentrant [`std::sync::RwLock`] read
    /// (issue #112).
    ///
    /// `stats_drop` receives the first order-event statistics drop (issue
    /// #165) so the caller logs it after its own bookkeeping.
    fn update_order_inner(
        &self,
        update: OrderUpdate,
        stats_drop: &mut Option<PriceLevelError>,
    ) -> Result<Option<Arc<OrderType<()>>>, PriceLevelError> {
        match update {
            OrderUpdate::UpdatePrice {
                order_id,
                new_price,
            } => {
                // If price changes, this order needs to be moved to a different price level
                // So we remove it from this level and return it for re-insertion elsewhere
                if new_price != Price::new(self.price) {
                    // Validated, removed and released as one unit (issue #163).
                    let order = self.remove_resting(order_id)?;

                    if order.is_some() {
                        // Update statistics (checked, issue #165).
                        if stats_drop.is_none() {
                            *stats_drop = self.record_order_event(
                                PriceLevelStatistics::record_order_removed_reporting,
                            );
                        } else {
                            let _ = self.record_order_event(
                                PriceLevelStatistics::record_order_removed_reporting,
                            );
                        }
                    }

                    Ok(order)
                } else {
                    // If price is the same, this is a no-op at the price level
                    // (Should be handled at the order book level)
                    Err(PriceLevelError::InvalidOperation {
                        message: "Cannot update price to the same value".to_string(),
                    })
                }
            }

            OrderUpdate::UpdateQuantity {
                order_id,
                new_quantity,
            } => {
                // Level-counter reservation is an `UpdatePlan` of checked
                // `CounterDelta`s (issue #163): BOTH directions use a checked
                // `fetch_update` and reject before any queue mutation — an
                // increase must not overflow `u64`, and a decrease must not
                // underflow it (issue #128 defense). `Relaxed`: advisory counters
                // (issue #68).
                //
                // The underflow guard is a belt-and-suspenders backstop. It is
                // unreachable in practice because issue #128's structural fix
                // publishes the match sweep's replenish counter transition UNDER
                // the maker's entry lock: by the time this `UpdateQuantity` holds
                // that same entry lock and reads `old` from the live maker, the
                // level counter already includes this maker's full `old`
                // contribution, so `counter >= old >= old - new` and the subtract
                // cannot go negative. The check simply refuses to wrap if that
                // invariant were ever violated, leaving the level untouched.
                let visible_counter = &self.visible_quantity;
                let hidden_counter = &self.hidden_quantity;
                let mut rollback_failed = false;

                // Two phases under the entry lock (issues #115, #163):
                //
                // 1. `decide` derives the resized order and the priority policy
                //    against the LIVE stored order and returns the counter plan
                //    as pure data. Nothing is read before the lock, so a
                //    concurrent match / replenish that committed first is fully
                //    reflected: the update can never resurrect executed or
                //    cancelled visible / hidden quantity, and the policy is
                //    chosen from the live total, not a stale pre-read.
                // 2. After the queue has validated the decision (the decided
                //    order keeps its stored id), `reserve` applies the plan with
                //    checked math; the commit that follows cannot fail. So an
                //    invalid decision is rejected with NO reservation taken, and
                //    an update that would overflow a level counter is rejected
                //    with the level (and queue) untouched.
                let queue = &self.orders;
                let decide = |live: &OrderType<()>| {
                    let old_visible = live.visible_quantity().as_u64();
                    let old_hidden = live.hidden_quantity().as_u64();
                    let live_total = old_visible.checked_add(old_hidden).ok_or_else(|| {
                        PriceLevelError::InvalidOperation {
                            message: "order total quantity overflow".to_string(),
                        }
                    })?;

                    // `with_reduced_quantity` sets the visible/main tranche to
                    // exactly `new_quantity` for every variant and preserves the
                    // LIVE hidden depth (never restored from a pre-read).
                    let new_order = live.with_reduced_quantity(new_quantity.as_u64());
                    let new_visible = new_order.visible_quantity().as_u64();
                    let new_hidden = new_order.hidden_quantity().as_u64();
                    let new_total = new_visible.checked_add(new_hidden).ok_or_else(|| {
                        PriceLevelError::InvalidOperation {
                            message: "order total quantity overflow".to_string(),
                        }
                    })?;

                    // A demotion needs a fresh tail sequence: reserve it here,
                    // inside the decision (checked, issue #165), before the
                    // queue's id validation and before `reserve` moves any level
                    // counter, so an exhausted sequence rejects the update with
                    // nothing to roll back and the maker keeps its place. A
                    // sequence reserved for a decision the id validation then
                    // rejects is skipped, never reissued.
                    let demote = if new_total > live_total {
                        Some(queue.try_reserve_seq()?)
                    } else {
                        None
                    };

                    // The counter plan is data only; `reserve` applies it once the
                    // queue has validated this decision.
                    let plan = UpdatePlan::new(old_visible, new_visible, old_hidden, new_hidden);

                    let arc = Arc::new(new_order);
                    // Test-only injection point (issue #163): may substitute an
                    // order with a different id to exercise the queue's
                    // pre-commit id validation.
                    #[cfg(test)]
                    let arc = apply_update_decision_hook(arc);
                    let decision = match demote {
                        Some(reserved) => UpdateDecision::ReplaceAtTail(arc, reserved),
                        None => UpdateDecision::KeepInPlace(arc),
                    };
                    Ok((decision, plan))
                };
                let reserve = |plan: UpdatePlan| {
                    plan.reserve(visible_counter, hidden_counter, &mut rollback_failed)
                };
                let outcome = self.orders.update_entry_with(order_id, decide, reserve);

                if rollback_failed {
                    // The rollback of a partial reservation could not be applied:
                    // the counters no longer describe the queue. Unreachable while
                    // every counter covers its resting orders (see
                    // `UpdatePlan::reserve`); fail fast rather than report a clean
                    // rejection (issue #163). Logged here, after the entry lock.
                    if self.trip_poison() {
                        tracing::error!(
                            price = self.price,
                            order_id = %order_id,
                            "update counter rollback failed; level poisoned — reconstruct it from a snapshot"
                        );
                    }
                }

                match outcome {
                    None => Ok(None), // Order not found / concurrently removed.
                    Some(result) => result.map(Some),
                }
            }

            OrderUpdate::UpdatePriceAndQuantity {
                order_id,
                new_price,
                new_quantity,
            } => {
                // If price changes, remove the order and let the order book handle re-insertion
                if new_price != Price::new(self.price) {
                    // Validated, removed and released as one unit (issue #163).
                    let order = self.remove_resting(order_id)?;

                    if order.is_some() {
                        // Update statistics (checked, issue #165).
                        if stats_drop.is_none() {
                            *stats_drop = self.record_order_event(
                                PriceLevelStatistics::record_order_removed_reporting,
                            );
                        } else {
                            let _ = self.record_order_event(
                                PriceLevelStatistics::record_order_removed_reporting,
                            );
                        }
                    }
                    Ok(order)
                } else {
                    // If price is the same, just update the quantity (reuse
                    // logic). Call the guard-free inner body — we already hold
                    // the fill-or-kill shared guard, and a second `fok_read`
                    // here would be a non-reentrant recursive read (issue #112).
                    self.update_order_inner(
                        OrderUpdate::UpdateQuantity {
                            order_id,
                            new_quantity,
                        },
                        stats_drop,
                    )
                }
            }

            OrderUpdate::Cancel { order_id } => {
                // Remove the order: validated, removed and released as one
                // unit (issue #163).
                let order = self.remove_resting(order_id)?;

                if order.is_some() {
                    // Update statistics (checked, issue #165).
                    if stats_drop.is_none() {
                        *stats_drop = self.record_order_event(
                            PriceLevelStatistics::record_order_removed_reporting,
                        );
                    } else {
                        let _ = self.record_order_event(
                            PriceLevelStatistics::record_order_removed_reporting,
                        );
                    }
                }

                Ok(order)
            }

            OrderUpdate::Replace {
                order_id,
                price,
                quantity,
                side: _,
            } => {
                // For replacement, check if the price is changing
                if price != Price::new(self.price) {
                    // If price is different, remove the order and let order book handle re-insertion
                    // Validated, removed and released as one unit (issue #163).
                    let order = self.remove_resting(order_id)?;

                    if order.is_some() {
                        // Update statistics (checked, issue #165).
                        if stats_drop.is_none() {
                            *stats_drop = self.record_order_event(
                                PriceLevelStatistics::record_order_removed_reporting,
                            );
                        } else {
                            let _ = self.record_order_event(
                                PriceLevelStatistics::record_order_removed_reporting,
                            );
                        }
                    }

                    Ok(order)
                } else {
                    // If price is the same, just update the quantity. Call the
                    // guard-free inner body — we already hold the fill-or-kill
                    // shared guard, and a second `fok_read` here would be a
                    // non-reentrant recursive read (issue #112).
                    self.update_order_inner(
                        OrderUpdate::UpdateQuantity {
                            order_id,
                            new_quantity: quantity,
                        },
                        stats_drop,
                    )
                }
            }
        }
    }
}

/// Serializable representation of a price level for easier data transfer and storage.
///
/// The `orders` vector is materialized in **queue-consumption order**
/// (ascending insertion sequence — exactly as `match_order` sweeps), and
/// [`TryFrom<PriceLevelData>`](PriceLevel#impl-TryFrom<PriceLevelData>-for-PriceLevel)
/// re-admits in vector order, so a `PriceLevelData` round-trip preserves
/// price-time (FIFO) priority just like the checksum-protected snapshot
/// package. Unlike the package, this plain representation carries no checksum
/// and no statistics — prefer [`PriceLevel::snapshot_package`] for
/// persistence.
///
/// Every collection grows fallibly (issue #164): building one from a level
/// ([`TryFrom<&PriceLevel>`](PriceLevelData#impl-TryFrom<%26PriceLevel>-for-PriceLevelData))
/// and decoding its `orders` array both report
/// [`PriceLevelError::CapacityExceeded`] instead of aborting on a refused
/// reservation.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PriceLevelData {
    /// The price of this level
    pub price: u128,
    /// Total visible quantity at this price level
    pub visible_quantity: u64,
    /// Total hidden quantity at this price level
    pub hidden_quantity: u64,
    /// Number of orders at this price level
    pub order_count: usize,
    /// Orders at this price level
    #[serde(deserialize_with = "deserialize_plain_orders")]
    pub orders: Vec<OrderType<()>>,
}

/// Materializes a level's plain data. Fallible since v0.10 (issue #164; this
/// replaces an infallible `From<&PriceLevel>` whose `collect` grew its
/// vectors infallibly).
impl TryFrom<&PriceLevel> for PriceLevelData {
    type Error = PriceLevelError;

    /// # Errors
    ///
    /// [`PriceLevelError::CapacityExceeded`] (resource
    /// [`CapacityResource::OrderSnapshot`]) if the order vectors cannot be
    /// reserved. The level is only read.
    fn try_from(price_level: &PriceLevel) -> Result<Self, Self::Error> {
        // Counters are read before the walk, as the former `From` did.
        let price = price_level.price();
        let visible_quantity = price_level.visible_quantity();
        let hidden_quantity = price_level.hidden_quantity();
        let order_count = price_level.order_count();
        // Consumption (insertion-sequence) order, NOT the unordered DashMap
        // iteration: `TryFrom<PriceLevelData>` re-admits in vector order,
        // so this is what makes the round-trip preserve price-time / FIFO
        // priority (issue #131) — the same contract the snapshot package
        // has kept since issue #109.
        let shared = price_level.snapshot_by_insertion_seq()?;
        let mut orders = Vec::new();
        try_reserve_exact_vec(&mut orders, shared.len(), CapacityResource::OrderSnapshot)?;
        orders.extend(shared.iter().map(|order_arc| **order_arc));
        Ok(Self {
            price,
            visible_quantity,
            hidden_quantity,
            order_count,
            orders,
        })
    }
}

impl TryFrom<&PriceLevelSnapshot> for PriceLevel {
    type Error = PriceLevelError;

    /// Rebuilds a price level from a borrowed snapshot.
    ///
    /// Fallible on purpose (this replaced an infallible `From` in v0.9): the old
    /// `From` swallowed [`PriceLevelSnapshot::refresh_aggregates`] errors and
    /// built the queue with keep-first duplicate handling, so a snapshot whose
    /// orders repeated an id would restore counters computed over every copy
    /// while the queue kept only one — a level whose counters silently disagreed
    /// with its contents. This clones the snapshot and delegates to
    /// [`PriceLevel::from_snapshot`], which rejects a repeated id and propagates
    /// the aggregate-overflow / per-order-total errors instead of hiding them.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::DuplicateOrderId`] if the snapshot's orders
    /// vector repeats an id, or [`PriceLevelError::InvalidOperation`] if a
    /// per-order or level aggregate overflows `u64` — see
    /// [`PriceLevel::from_snapshot`].
    fn try_from(value: &PriceLevelSnapshot) -> Result<Self, Self::Error> {
        // Fallible owned copy (issue #164): the orders vector is reserved
        // through `try_reserve_exact`, not the aborting derived `Clone`.
        PriceLevel::from_snapshot(value.try_clone()?)
    }
}

impl TryFrom<PriceLevelData> for PriceLevel {
    type Error = PriceLevelError;

    fn try_from(data: PriceLevelData) -> Result<Self, Self::Error> {
        let price_level = PriceLevel::new(data.price);

        // Add orders to the price level. Propagate an admission overflow rather
        // than panicking while reconstructing from external data.
        for order in data.orders {
            price_level.add_order(order)?;
        }

        Ok(price_level)
    }
}

// Implement custom serialization for the atomic types
impl Serialize for PriceLevel {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // Serialize the `PriceLevelData` shape without copying the orders:
        // the same struct name, field names and order as the derived
        // `PriceLevelData` impl, so the bytes are identical, while the orders
        // are serialized borrowed from the one fallible materialization
        // (issue #164). Counters are read before the walk, as before.
        use serde::ser::SerializeStruct;

        let price = self.price();
        let visible_quantity = self.visible_quantity();
        let hidden_quantity = self.hidden_quantity();
        let order_count = self.order_count();
        let orders = self
            .snapshot_by_insertion_seq()
            .map_err(serde::ser::Error::custom)?;
        let mut state = serializer.serialize_struct("PriceLevelData", 5)?;
        state.serialize_field("price", &price)?;
        state.serialize_field("visible_quantity", &visible_quantity)?;
        state.serialize_field("hidden_quantity", &hidden_quantity)?;
        state.serialize_field("order_count", &order_count)?;
        state.serialize_field("orders", &BorrowedOrders(&orders))?;
        state.end()
    }
}

/// Parses the text written by `Display`:
/// `PriceLevel:price=..;...;orders=[<order>,<order>,...]`.
///
/// Only `price` (required) and `orders` (optional) are read; other
/// `key=value` pairs are ignored and a repeated key keeps its last value.
/// The orders section runs from the first `orders=[` to the next `]`; it is
/// split at top-level `,` (not nested in `(...)` / `[...]`) and each order
/// is parsed from a borrowed slice and admitted in text order.
///
/// # Errors
///
/// [`PriceLevelError::ParseError`] for a missing prefix, an unclosed orders
/// bracket, a missing / invalid price, an unparsable order, unbalanced
/// `(` / `)` / `[` inside the orders section, or nesting deeper than 128
/// levels; [`PriceLevelError::CapacityExceeded`] (resource `Text`) if a temporary buffer cannot
/// be allocated; and any admission error from [`PriceLevel::add_order`].
impl FromStr for PriceLevel {
    type Err = PriceLevelError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        use std::borrow::Cow;

        let content = s
            .strip_prefix("PriceLevel:")
            .ok_or_else(|| PriceLevelError::ParseError {
                message: "Invalid format: missing 'PriceLevel:' prefix".to_string(),
            })?;

        let mut price_str = None;
        let mut orders_str = None;
        let remaining_content: Cow<str>;

        if let Some((before_orders, from_orders)) = content.split_once("orders=[") {
            let (orders, after_orders) =
                from_orders
                    .split_once(']')
                    .ok_or_else(|| PriceLevelError::ParseError {
                        message: "Invalid format: unclosed orders bracket".to_string(),
                    })?;
            orders_str = Some(orders);

            // The text around the orders section is re-joined before the
            // `key=value` scan (a pair may straddle the section), reserving
            // through the fallible allocator API.
            let joined_len = before_orders
                .len()
                .checked_add(after_orders.len())
                .ok_or_else(|| PriceLevelError::InvalidOperation {
                    message: "price level text length overflow".to_string(),
                })?;
            let mut joined = String::new();
            try_reserve_str(&mut joined, joined_len)?;
            joined.push_str(before_orders);
            joined.push_str(after_orders);
            remaining_content = Cow::Owned(joined);
        } else {
            remaining_content = Cow::Borrowed(content);
        }

        for part in remaining_content.split(';').filter(|s| !s.is_empty()) {
            if let Some((key, value)) = part.split_once('=') {
                match key {
                    "price" => price_str = Some(value),
                    "orders" => orders_str = Some(value),
                    _ => {}
                }
            }
        }

        let price = price_str
            .and_then(|v| v.parse::<u128>().ok())
            .ok_or_else(|| PriceLevelError::ParseError {
                message: "Missing or invalid price".to_string(),
            })?;

        let price_level = PriceLevel::new(price);

        if let Some(orders_part) = orders_str
            && !orders_part.is_empty()
        {
            let segments = TopLevelSplit::new(
                orders_part,
                b',',
                b"([",
                b")]",
                MAX_TEXT_NESTING_DEPTH_INSIDE_LIST,
            );
            for segment in segments {
                let segment = segment.map_err(|e| match e {
                    NestingError::TooDeep { .. } => {
                        NestingError::too_deep_error(MAX_TEXT_NESTING_DEPTH)
                    }
                    NestingError::UnmatchedClose => PriceLevelError::ParseError {
                        message: "Invalid format: unmatched closing delimiter in orders"
                            .to_string(),
                    },
                    NestingError::Unclosed => PriceLevelError::ParseError {
                        message: "Invalid format: unclosed delimiter in orders".to_string(),
                    },
                })?;
                // A separator-terminated segment is parsed even when empty
                // (and rejected); an empty final segment (trailing `,`) is
                // skipped.
                if segment.is_last && segment.text.is_empty() {
                    continue;
                }
                let order = OrderType::<()>::from_str(segment.text).map_err(|e| {
                    PriceLevelError::ParseError {
                        message: format!("Order parse error: {e}"),
                    }
                })?;
                price_level.add_order(order)?;
            }
        }

        Ok(price_level)
    }
}

impl<'de> Deserialize<'de> for PriceLevel {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Deserialize into the data representation
        let data = PriceLevelData::deserialize(deserializer)?;

        // Convert to PriceLevel
        PriceLevel::try_from(data).map_err(serde::de::Error::custom)
    }
}

impl PartialEq for PriceLevel {
    fn eq(&self, other: &Self) -> bool {
        self.price == other.price
    }
}

impl Eq for PriceLevel {}

impl PartialOrd for PriceLevel {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PriceLevel {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.price.cmp(&other.price)
    }
}

impl std::fmt::Debug for PriceLevel {
    /// Snapshot-then-write (issue #172): the atomics are loaded and the queue is
    /// formatted through [`OrderQueue`]'s own materializing `Debug`, and the
    /// `fok_guard` is deliberately omitted — the `RwLock`'s `Debug` would hold a
    /// read guard while writing into the caller's formatter, which could
    /// deadlock a destination that re-enters this level's fill-or-kill path.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let visible_quantity = self.visible_quantity.load(Ordering::Relaxed);
        let hidden_quantity = self.hidden_quantity.load(Ordering::Relaxed);
        let topology = self.topology.load(Ordering::Relaxed);
        let topology_epoch = self.topology_epoch.load(Ordering::Relaxed);
        let level_poisoned = self.level_poisoned.load(Ordering::Relaxed);
        let mutation_epoch = self.mutation_epoch.load(Ordering::Relaxed);
        f.debug_struct("PriceLevel")
            .field("price", &self.price)
            .field("visible_quantity", &visible_quantity)
            .field("hidden_quantity", &hidden_quantity)
            .field("topology", &topology)
            .field("topology_epoch", &topology_epoch)
            .field("orders", &self.orders)
            .field("stats", &self.stats)
            .field("level_poisoned", &level_poisoned)
            .field("mutation_epoch", &mutation_epoch)
            .finish_non_exhaustive()
    }
}

/// Writes `PriceLevel:price=..;visible_quantity=..;hidden_quantity=..;
/// order_count=..;orders=[<order>,...]` (orders in timestamp order).
///
/// If the order materialization cannot be reserved (issue #164) the orders
/// section is written as `orders=!<error>`: `Display` must not report a
/// `fmt::Error` of its own (`to_string` would panic), and [`FromStr`] rejects
/// the marker, so a failed rendering is never parsed back as an empty level.
impl Display for PriceLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "PriceLevel:price={};visible_quantity={};hidden_quantity={};order_count={};orders=",
            self.price(),
            self.visible_quantity(),
            self.hidden_quantity(),
            self.order_count()
        )?;

        let orders = match self.snapshot_orders() {
            Ok(orders) => orders,
            Err(err) => return write!(f, "!{err}"),
        };
        write!(f, "[")?;
        let mut first = true;
        for order in orders {
            if !first {
                write!(f, ",")?;
            }
            write!(f, "{order}")?;
            first = false;
        }

        write!(f, "]")
    }
}
