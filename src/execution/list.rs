use crate::errors::{CapacityResource, PriceLevelError};
use crate::execution::trade::Trade;
use crate::utils::alloc::{MergePlan, merge_planned, plan_merge};
use crate::utils::text::{
    MAX_TEXT_NESTING_DEPTH, MAX_TEXT_NESTING_DEPTH_INSIDE_LIST, NestingError, TopLevelSplit,
    try_push,
};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// A wrapper for a vector of trades to implement custom serialization.
///
/// The inner collection is private to enforce append-only semantics
/// during matching. Use [`Self::add`] to append and [`Self::as_vec`]
/// or [`Self::into_vec`] to read.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TradeList {
    /// Ordered collection of trades.
    trades: Vec<Trade>,
}

impl TradeList {
    /// Create a new empty trade list
    #[must_use]
    pub fn new() -> Self {
        Self { trades: Vec::new() }
    }

    /// Create a new empty trade list with space reserved for `n` trades.
    ///
    /// Pre-allocates the backing vector to avoid repeated reallocations when
    /// the number of trades produced by a single match sweep is known or
    /// estimable (e.g. bounded by the resting order count at a level).
    ///
    /// Allocation is fallible: the list starts from `Vec::new` and reserves
    /// with `try_reserve_exact`, so an unrepresentable `n` (for example
    /// `usize::MAX`, whose byte size exceeds `isize::MAX`) or an allocator
    /// refusal returns a typed error instead of panicking. `n == 0` never
    /// allocates.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::CapacityExceeded`] with resource
    /// [`CapacityResource::Trades`] if the storage cannot be reserved.
    pub fn try_with_capacity(n: usize) -> Result<Self, PriceLevelError> {
        let mut list = Self::new();
        list.try_reserve_exact(n)?;
        Ok(list)
    }

    /// Create a trade list from an existing vector
    #[must_use]
    pub fn from_vec(trades: Vec<Trade>) -> Self {
        Self { trades }
    }

    /// Reserves room for at least `additional` more trades (amortized growth).
    ///
    /// A no-op that never allocates when the spare capacity already suffices.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::CapacityExceeded`] with resource
    /// [`CapacityResource::Trades`] if the storage cannot grow; the list is
    /// left unchanged.
    #[inline]
    pub fn try_reserve(&mut self, additional: usize) -> Result<(), PriceLevelError> {
        #[cfg(test)]
        test_seam::check(self.trades.len(), additional)?;
        self.trades
            .try_reserve(additional)
            .map_err(|_| PriceLevelError::capacity_exceeded(CapacityResource::Trades, additional))
    }

    /// Exact-growth variant of [`Self::try_reserve`].
    pub(crate) fn try_reserve_exact(&mut self, additional: usize) -> Result<(), PriceLevelError> {
        #[cfg(test)]
        test_seam::check(self.trades.len(), additional)?;
        self.trades
            .try_reserve_exact(additional)
            .map_err(|_| PriceLevelError::capacity_exceeded(CapacityResource::Trades, additional))
    }

    /// Returns the number of trades the list can hold without reallocating.
    #[must_use]
    #[inline]
    pub fn capacity(&self) -> usize {
        self.trades.capacity()
    }

    /// Append a trade to the list.
    ///
    /// Growth is fallible: room for the trade is reserved with `try_reserve`
    /// before the push, so the push itself can never reallocate or panic.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::CapacityExceeded`] with resource
    /// [`CapacityResource::Trades`] if the list cannot grow; the list is left
    /// unchanged.
    pub fn add(&mut self, trade: Trade) -> Result<(), PriceLevelError> {
        self.try_reserve(1)?;
        self.push_reserved(trade);
        Ok(())
    }

    /// Pushes into capacity the caller already reserved (via
    /// [`Self::try_reserve`]). With spare capacity `Vec::push` never
    /// reallocates, so this cannot fail or panic.
    #[inline]
    pub(crate) fn push_reserved(&mut self, trade: Trade) {
        self.trades.push(trade);
    }

    /// The allocation-free way to put `other`'s trades behind this list's,
    /// if any (issue #219; see [`plan_merge`]).
    #[inline]
    #[must_use]
    pub(crate) fn plan_merge(&self, other: &TradeList) -> Option<MergePlan> {
        plan_merge(
            self.trades.len(),
            self.trades.capacity(),
            other.trades.len(),
            other.trades.capacity(),
        )
    }

    /// Moves every trade of `other` behind this list's, leaving `other`
    /// empty, per a `plan` from [`Self::plan_merge`] (or
    /// [`MergePlan::Append`] after [`Self::try_reserve`] of `other.len()`).
    /// Never allocates, so it cannot fail or panic (issue #219).
    #[inline]
    pub(crate) fn merge_planned(&mut self, other: &mut TradeList, plan: MergePlan) {
        merge_planned(&mut self.trades, &mut other.trades, plan);
    }

    /// Clones the list without an infallible allocation.
    ///
    /// `Clone` is still derived (it aborts / panics like any `Vec` clone on
    /// allocation failure); use this where the policy requires a typed failure.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::CapacityExceeded`] with resource
    /// [`CapacityResource::Trades`] if the copy cannot be allocated.
    pub fn try_clone(&self) -> Result<Self, PriceLevelError> {
        let mut copy = Self::try_with_capacity(self.trades.len())?;
        // `Trade: Copy`; the exact reservation above covers every element.
        copy.trades.extend_from_slice(&self.trades);
        Ok(copy)
    }

    /// Get a reference to the underlying vector
    #[must_use]
    pub fn as_vec(&self) -> &Vec<Trade> {
        &self.trades
    }

    /// Convert into a vector of trades
    #[must_use]
    pub fn into_vec(self) -> Vec<Trade> {
        self.trades
    }

    /// Returns `true` when the list does not contain any trades.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.trades.is_empty()
    }

    /// Returns the number of trades in the list.
    #[must_use]
    pub fn len(&self) -> usize {
        self.trades.len()
    }
}

impl Default for TradeList {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for TradeList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Trades:[")?;

        for (i, trade) in self.trades.iter().enumerate() {
            if i > 0 {
                write!(f, ",")?;
            }
            write!(f, "{trade}")?;
        }

        write!(f, "]")
    }
}

/// Parses the `Trades:[<trade>,<trade>,...]` text written by `Display`.
///
/// The list is split at top-level `,` separators and each segment is parsed
/// with [`Trade::from_str`] directly from a borrowed slice of the input — no
/// per-trade copy (issue #152). Empty segments (`,,`, a leading or trailing
/// `,`) are skipped. A `,` nested inside `[...]` belongs to its segment.
///
/// # Errors
///
/// - [`PriceLevelError::InvalidFormat`] if the `Trades:[` prefix or the final
///   `]` is missing, or a `[` / `]` inside the list is unbalanced.
/// - [`PriceLevelError::ParseError`] if brackets nest deeper than 128 levels
///   (the enclosing list bracket included).
/// - [`PriceLevelError::CapacityExceeded`] (resource `Text`) if the trade vector cannot grow (#164).
/// - Any error returned by [`Trade::from_str`] for a malformed segment,
///   reported for the first malformed segment in input order.
impl FromStr for TradeList {
    type Err = PriceLevelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let content = s
            .strip_prefix("Trades:[")
            .and_then(|rest| rest.strip_suffix(']'))
            .ok_or(PriceLevelError::InvalidFormat)?;

        if content.is_empty() {
            return Ok(TradeList::new());
        }

        // The enclosing `[` of the list counts toward the nesting limit, so
        // segments may nest one level less than the limit.
        let limit = MAX_TEXT_NESTING_DEPTH_INSIDE_LIST;
        let mut trades = Vec::new();
        for segment in TopLevelSplit::new(content, b',', b"[", b"]", limit) {
            let segment = segment.map_err(|e| match e {
                NestingError::TooDeep { .. } => {
                    NestingError::too_deep_error(MAX_TEXT_NESTING_DEPTH)
                }
                NestingError::UnmatchedClose | NestingError::Unclosed => {
                    PriceLevelError::InvalidFormat
                }
            })?;
            if !segment.text.is_empty() {
                try_push(&mut trades, Trade::from_str(segment.text)?)?;
            }
        }

        Ok(TradeList { trades })
    }
}

impl From<Vec<Trade>> for TradeList {
    fn from(trades: Vec<Trade>) -> Self {
        Self::from_vec(trades)
    }
}

impl From<TradeList> for Vec<Trade> {
    fn from(list: TradeList) -> Self {
        list.into_vec()
    }
}

/// Test-only capacity limiter for the execution results (issue #170).
///
/// A thread-local element cap: while armed, any fallible reservation that would
/// make a [`TradeList`] hold more than `limit` trades fails with the same typed
/// [`PriceLevelError::CapacityExceeded`] a real allocator refusal produces. This
/// lets tests drive the "result growth failed mid-sweep" path deterministically
/// without exhausting process memory. Compiled only under `cfg(test)`; there is
/// no production-visible knob.
#[cfg(test)]
pub(crate) mod test_seam {
    use crate::errors::{CapacityResource, PriceLevelError};
    use std::cell::Cell;

    thread_local! {
        static TRADE_LIMIT: Cell<Option<usize>> = const { Cell::new(None) };
    }

    /// Restores the previous limit on drop.
    pub(crate) struct TradeLimitGuard(Option<usize>);

    impl Drop for TradeLimitGuard {
        fn drop(&mut self) {
            crate::utils::test_tls::cell_set(&TRADE_LIMIT, self.0);
        }
    }

    /// Caps every `TradeList` on this thread at `limit` trades until the guard
    /// drops.
    pub(crate) fn limit_trades(limit: usize) -> TradeLimitGuard {
        TradeLimitGuard(crate::utils::test_tls::cell_replace(
            &TRADE_LIMIT,
            Some(limit),
            None,
        ))
    }

    /// `true` while a limit is armed on this thread.
    pub(crate) fn armed() -> bool {
        crate::utils::test_tls::cell_get(&TRADE_LIMIT, None).is_some()
    }

    pub(super) fn check(len: usize, additional: usize) -> Result<(), PriceLevelError> {
        let over = crate::utils::test_tls::cell_get(&TRADE_LIMIT, None).is_some_and(|limit| {
            len.checked_add(additional)
                .is_none_or(|wanted| wanted > limit)
        });
        if over {
            Err(PriceLevelError::capacity_exceeded(
                CapacityResource::Trades,
                additional,
            ))
        } else {
            Ok(())
        }
    }
}
