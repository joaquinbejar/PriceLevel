use crate::errors::{CapacityResource, PriceLevelError};
use crate::orders::{Id, OrderType, Side};
use crate::price_level::statistics::PriceLevelStatistics;
use crate::utils::alloc::{
    FallibleWriter, capacity_error, try_copy_str, try_push_vec, try_reserve_exact_vec,
    try_reserve_string, try_reserve_vec,
};
use crate::utils::dedup::first_repeat_position;
use crate::utils::text::{Fields, split_exactly_once};
use crate::utils::{Price, Quantity};
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

/// A snapshot of a price level in the order book. This struct provides a summary of the state of a specific price level
/// at a given point in time, including the price, visible and hidden quantities, order count, the orders
/// at that level, and the per-level execution statistics.
///
/// # Copies and allocation (issue #164)
///
/// The derived [`Clone`] copies the orders vector with an infallible
/// allocation: the standard trait cannot report a refused allocation, so on
/// allocator failure it aborts the process. It is kept for ergonomic use in
/// tests and tooling. Code that must survive a refused allocation uses
/// [`PriceLevelSnapshot::try_clone`], which reserves through
/// `try_reserve_exact` and returns [`PriceLevelError::CapacityExceeded`]. The
/// derived [`Default`] allocates nothing (empty vector, zeroed statistics).
#[derive(Debug, Default, Clone)]
pub struct PriceLevelSnapshot {
    /// The price of this level, in price ticks.
    price: Price,
    /// Total visible quantity at this level, in quantity units.
    visible_quantity: Quantity,
    /// Total hidden quantity at this level, in quantity units.
    hidden_quantity: Quantity,
    /// Number of orders at this level.
    order_count: usize,
    /// Orders at this level.
    orders: Vec<Arc<OrderType<()>>>,
    /// Per-level execution statistics captured at snapshot time.
    ///
    /// Carries the eight counters (orders added / removed / executed, quantity
    /// and value executed, last-execution and first-arrival timestamps, and the
    /// waiting-time sum) so a restored level resumes with its recorded history
    /// rather than a zeroed set. Persisted via [`PriceLevelStatistics`]'s own
    /// serde shape and covered by the package SHA-256 checksum.
    statistics: PriceLevelStatistics,
}

impl PriceLevelSnapshot {
    /// Create a new empty snapshot at the given price.
    #[must_use]
    pub fn new(price: Price) -> Self {
        Self {
            price,
            visible_quantity: Quantity::ZERO,
            hidden_quantity: Quantity::ZERO,
            order_count: 0,
            orders: Vec::new(),
            statistics: PriceLevelStatistics::new(),
        }
    }

    /// Creates a snapshot populated with orders, computing aggregates automatically.
    ///
    /// The statistics are initialized empty and unstamped (deterministic, no
    /// clock read); use [`Self::with_orders_and_stats`]
    /// to carry recorded execution statistics into the snapshot.
    ///
    /// The `orders` vector order is significant: it is the queue-consumption
    /// order a restore reproduces, since
    /// [`crate::price_level::PriceLevel::from_snapshot`] re-enqueues in vector
    /// order. Pass orders in ascending insertion-sequence (sweep) order to
    /// preserve price-time priority across a round-trip.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::InvalidOperation`] if summing the per-order
    /// visible / hidden quantities overflows `u64`.
    pub fn with_orders(
        price: Price,
        orders: Vec<Arc<OrderType<()>>>,
    ) -> Result<Self, PriceLevelError> {
        Self::with_orders_and_stats(price, orders, PriceLevelStatistics::new())
    }

    /// Creates a snapshot populated with orders and statistics, computing
    /// aggregates automatically.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::InvalidOperation`] if summing the per-order
    /// visible / hidden quantities overflows `u64`.
    pub fn with_orders_and_stats(
        price: Price,
        orders: Vec<Arc<OrderType<()>>>,
        statistics: PriceLevelStatistics,
    ) -> Result<Self, PriceLevelError> {
        let mut snapshot = Self {
            price,
            visible_quantity: Quantity::ZERO,
            hidden_quantity: Quantity::ZERO,
            order_count: 0,
            orders,
            statistics,
        };
        snapshot.refresh_aggregates()?;
        Ok(snapshot)
    }

    /// Returns the price of this level, in price ticks.
    #[must_use]
    pub fn price(&self) -> Price {
        self.price
    }

    /// Returns the total visible quantity, in quantity units.
    #[must_use]
    pub fn visible_quantity(&self) -> Quantity {
        self.visible_quantity
    }

    /// Returns the total hidden quantity, in quantity units.
    #[must_use]
    pub fn hidden_quantity(&self) -> Quantity {
        self.hidden_quantity
    }

    /// Returns the number of orders.
    #[must_use]
    pub fn order_count(&self) -> usize {
        self.order_count
    }

    /// Returns a reference to the per-level statistics captured in this snapshot.
    #[must_use]
    pub fn statistics(&self) -> &PriceLevelStatistics {
        &self.statistics
    }

    /// Returns a reference to the orders in this snapshot.
    ///
    /// The vector order is significant: it is the queue-consumption order a
    /// restore reproduces, since [`crate::price_level::PriceLevel::from_snapshot`]
    /// re-enqueues the orders in vector order.
    #[must_use]
    pub fn orders(&self) -> &[Arc<OrderType<()>>] {
        &self.orders
    }

    /// Consumes the snapshot and returns the inner orders vector.
    #[must_use]
    pub fn into_orders(self) -> Vec<Arc<OrderType<()>>> {
        self.orders
    }

    /// Fallible owned copy (issue #164): the orders vector is reserved with
    /// `try_reserve_exact` before anything is copied (the copy itself only
    /// clones `Arc` pointers), and the statistics are copied without heap
    /// allocation.
    ///
    /// Prefer this over [`Clone::clone`], which aborts the process if the
    /// allocator refuses the vector.
    ///
    /// # Errors
    ///
    /// [`PriceLevelError::CapacityExceeded`] (resource
    /// [`CapacityResource::OrderSnapshot`]) if the orders vector cannot be
    /// reserved; `self` is only read.
    pub fn try_clone(&self) -> Result<Self, PriceLevelError> {
        let mut orders = Vec::new();
        try_reserve_exact_vec(
            &mut orders,
            self.orders.len(),
            CapacityResource::OrderSnapshot,
        )?;
        orders.extend(self.orders.iter().cloned());
        Ok(Self {
            price: self.price,
            visible_quantity: self.visible_quantity,
            hidden_quantity: self.hidden_quantity,
            order_count: self.order_count,
            orders,
            statistics: self.statistics.clone(),
        })
    }

    /// Constructs a snapshot with pre-computed aggregates and empty statistics.
    ///
    /// This is intended for internal crate use where the caller has already
    /// computed the aggregate values (e.g., from atomic counters) and does not
    /// carry execution statistics. Use [`Self::from_raw_parts_with_stats`] to
    /// persist recorded statistics alongside the aggregates.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn from_raw_parts(
        price: Price,
        visible_quantity: Quantity,
        hidden_quantity: Quantity,
        order_count: usize,
        orders: Vec<Arc<OrderType<()>>>,
    ) -> Self {
        Self::from_raw_parts_with_stats(
            price,
            visible_quantity,
            hidden_quantity,
            order_count,
            orders,
            PriceLevelStatistics::new(),
        )
    }

    /// Constructs a snapshot with pre-computed aggregates and recorded statistics.
    ///
    /// This is intended for internal crate use where the caller has already
    /// computed the aggregate values and wants to preserve the level's
    /// execution statistics through the snapshot. The caller is responsible for
    /// passing aggregates that agree with `orders`;
    /// [`crate::price_level::PriceLevel::snapshot`] derives them from `orders`
    /// with [`SnapshotAggregates::from_orders`].
    #[must_use]
    pub(crate) fn from_raw_parts_with_stats(
        price: Price,
        visible_quantity: Quantity,
        hidden_quantity: Quantity,
        order_count: usize,
        orders: Vec<Arc<OrderType<()>>>,
        statistics: PriceLevelStatistics,
    ) -> Self {
        Self {
            price,
            visible_quantity,
            hidden_quantity,
            order_count,
            orders,
            statistics,
        }
    }

    /// Get the total quantity (visible + hidden) at this price level.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::InvalidOperation`] if `visible + hidden`
    /// overflows `u64`.
    pub fn total_quantity(&self) -> Result<Quantity, PriceLevelError> {
        self.visible_quantity
            .as_u64()
            .checked_add(self.hidden_quantity.as_u64())
            .map(Quantity::new)
            .ok_or_else(|| PriceLevelError::InvalidOperation {
                message: "snapshot total quantity overflow".to_string(),
            })
    }

    /// Get an iterator over the orders in this snapshot
    pub fn iter_orders(&self) -> impl Iterator<Item = &Arc<OrderType<()>>> {
        self.orders.iter()
    }

    /// Recomputes aggregate fields (`visible_quantity`, `hidden_quantity`, and `order_count`) based on current orders.
    ///
    /// Transactional: every replacement value is computed with checked
    /// arithmetic first, and the three fields are committed together only once
    /// all of them succeeded. On error the snapshot is left exactly as it was;
    /// no field is partially refreshed.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::InvalidOperation`] if any single order's own
    /// visible + hidden total overflows `u64`, or if summing the per-order
    /// visible or hidden quantities across the level overflows `u64`.
    pub fn refresh_aggregates(&mut self) -> Result<(), PriceLevelError> {
        let aggregates = SnapshotAggregates::from_orders(&self.orders)?;
        self.visible_quantity = aggregates.visible_quantity;
        self.hidden_quantity = aggregates.hidden_quantity;
        self.order_count = aggregates.order_count;
        Ok(())
    }
}

/// The three aggregate fields of a [`PriceLevelSnapshot`], derived from one
/// orders slice with checked arithmetic.
///
/// Shared by [`PriceLevelSnapshot::refresh_aggregates`] and
/// [`crate::price_level::PriceLevel::snapshot`], so a live snapshot and a
/// refreshed or restored one validate their orders with exactly the same
/// rules: a snapshot the level returns always passes the package's refresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SnapshotAggregates {
    /// Sum of the per-order visible quantities.
    pub(crate) visible_quantity: Quantity,
    /// Sum of the per-order hidden quantities.
    pub(crate) hidden_quantity: Quantity,
    /// Number of orders in the slice.
    pub(crate) order_count: usize,
}

impl SnapshotAggregates {
    /// Folds `orders` into their aggregates. Pure: nothing is mutated, so a
    /// failure leaves every caller-owned value untouched.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::InvalidOperation`] if any single order's own
    /// visible + hidden total overflows `u64`, or if the visible or hidden sum
    /// across `orders` overflows `u64`.
    pub(crate) fn from_orders(orders: &[Arc<OrderType<()>>]) -> Result<Self, PriceLevelError> {
        let mut fold = AggregateFold::default();
        for order in orders {
            fold.push(order)?;
        }
        Ok(fold.finish(orders.len()))
    }
}

/// Running checked fold of per-order visible / hidden quantities behind
/// [`SnapshotAggregates::from_orders`], which the refresh, the live snapshot
/// and the restore validation
/// ([`PriceLevelSnapshot::into_validated_restore`], issue #150) all use, so
/// they reject the same order with the same error.
#[derive(Debug, Default)]
struct AggregateFold {
    /// Sum of the visible quantities folded so far.
    visible_total: u64,
    /// Sum of the hidden quantities folded so far.
    hidden_total: u64,
}

impl AggregateFold {
    /// Folds one order. On error the fold is left unchanged.
    ///
    /// # Errors
    ///
    /// [`PriceLevelError::InvalidOperation`] if the order's own visible +
    /// hidden total overflows `u64` (checked first), or if adding it to the
    /// running visible or hidden sum overflows `u64` (in that order).
    #[inline]
    fn push(&mut self, order: &OrderType<()>) -> Result<(), PriceLevelError> {
        let visible = order.visible_quantity().as_u64();
        let hidden = order.hidden_quantity().as_u64();

        // Reject any order whose OWN visible + hidden total is not
        // representable in `u64`. `PriceLevel::add_order` enforces this same
        // per-order invariant at admission, and the match sweep's reserve
        // replenishment relies on it (a refreshed tranche is
        // `new_visible + drawn_hidden <= visible + hidden`, which overflows
        // only if the order's own total already does). Restoring such an
        // order would smuggle in a state admission rejects, so the restore
        // path validates it too rather than trusting the serialized bytes.
        if visible.checked_add(hidden).is_none() {
            return Err(aggregate_overflow("order total quantity overflows u64"));
        }

        let visible_total = self
            .visible_total
            .checked_add(visible)
            .ok_or_else(|| aggregate_overflow("snapshot visible quantity overflow"))?;
        let hidden_total = self
            .hidden_total
            .checked_add(hidden)
            .ok_or_else(|| aggregate_overflow("snapshot hidden quantity overflow"))?;

        self.visible_total = visible_total;
        self.hidden_total = hidden_total;
        Ok(())
    }

    /// The folded aggregates for `order_count` orders.
    #[inline]
    fn finish(self, order_count: usize) -> SnapshotAggregates {
        SnapshotAggregates {
            visible_quantity: Quantity::new(self.visible_total),
            hidden_quantity: Quantity::new(self.hidden_total),
            order_count,
        }
    }
}

/// A snapshot whose orders passed every restore check, carrying what
/// [`crate::price_level::PriceLevel::from_snapshot`] needs to build the level
/// (issue #150). Produced only by
/// [`PriceLevelSnapshot::into_validated_restore`].
#[derive(Debug)]
pub(crate) struct ValidatedRestore {
    /// The level price, in price ticks.
    pub(crate) price: Price,
    /// Aggregates recomputed from `orders` (the snapshot's stored aggregate
    /// fields are not trusted).
    pub(crate) aggregates: SnapshotAggregates,
    /// The single side every order shares; `None` for an empty snapshot.
    pub(crate) side: Option<Side>,
    /// The orders, in snapshot (queue-consumption) order, unchanged.
    pub(crate) orders: Vec<Arc<OrderType<()>>>,
    /// The persisted statistics, moved out of the snapshot, with their
    /// private seqlock sequence restarted at 0.
    pub(crate) statistics: PriceLevelStatistics,
}

/// Error for an order price that differs from the level price.
#[cold]
#[inline(never)]
fn topology_price_error(order_price: u128, level_price: u128) -> PriceLevelError {
    PriceLevelError::InvalidOperation {
        message: format!(
            "snapshot order price {order_price} does not match level price {level_price}"
        ),
    }
}

/// Error for an order side that differs from the level side.
#[cold]
#[inline(never)]
fn topology_side_error(order_side: Side, level_side: Side) -> PriceLevelError {
    PriceLevelError::InvalidOperation {
        message: format!(
            "snapshot order side {order_side:?} is incompatible with the level side {level_side:?}"
        ),
    }
}

/// Error for an order id repeated in the snapshot.
#[cold]
#[inline(never)]
fn duplicate_id_error(id: Id) -> PriceLevelError {
    PriceLevelError::DuplicateOrderId(id.to_string())
}

impl PriceLevelSnapshot {
    /// Validates the orders for restoration and returns the validated parts
    /// (issue #150).
    ///
    /// Before #150 restore walked the orders three times (aggregate refresh,
    /// duplicate ids, topology), each walk returning its first error. #150
    /// fused the last two; the pre-release hardening split them again so the
    /// duplicate check needs no hasher:
    ///
    /// 1. The allocation-free checked aggregate fold
    ///    ([`SnapshotAggregates::from_orders`]). It runs to completion before
    ///    anything is allocated, so a snapshot rejected here costs no
    ///    scratch memory, whatever the position of the failing order.
    /// 2. A hasher-free duplicate scan (the ids copied into one fallibly
    ///    reserved scratch vector and sorted), then a topology pass.
    ///
    /// The error precedence is the pre-#150 one, unchanged:
    ///
    /// 1. Aggregates (per-order total, then the running visible and hidden
    ///    sums): the first failing order in vector order.
    /// 2. [`PriceLevelError::CapacityExceeded`] (resource
    ///    [`CapacityResource::RestoreScratch`]) if the duplicate-id scratch
    ///    cannot be reserved.
    /// 3. [`PriceLevelError::DuplicateOrderId`] for the first id that
    ///    repeats, reported at its second occurrence.
    /// 4. Topology: the first order whose price differs from the level price
    ///    or whose side differs from the first order's side (price checked
    ///    before side for the same order).
    ///
    /// Because the duplicate scan covers the whole vector before topology is
    /// looked at, a duplicate anywhere wins over a topology violation
    /// anywhere. Duplicates are always an error: the queue's keep-first
    /// behaviour is never relied upon.
    ///
    /// # Errors
    ///
    /// As ranked above; the snapshot is consumed either way.
    #[inline(never)]
    pub(crate) fn into_validated_restore(self) -> Result<ValidatedRestore, PriceLevelError> {
        // Walk 1, rank 1: allocation-free.
        let aggregates = SnapshotAggregates::from_orders(&self.orders)?;

        // Rank 2. Sized by an input-derived length, so reserved fallibly
        // (issue #164). Hasher-free (pre-release hardening): the former
        // `HashSet` built a `RandomState`, which can panic on OS RNG failure
        // or during thread-local destruction on some platforms. Sorting the
        // ids yields the earliest repeat position, which is exactly where the
        // former left-to-right `insert` walk returned.
        let first_repeat = first_repeat_position(
            self.orders.iter().map(|order| order.id()),
            CapacityResource::RestoreScratch,
        )?;

        // Rank 3: a duplicate outranks every topology violation.
        if let Some(order) = first_repeat.and_then(|position| self.orders.get(position)) {
            return Err(duplicate_id_error(order.id()));
        }

        // Walk 2, rank 4.
        let level_price = self.price.as_u128();
        let mut side: Option<Side> = None;
        for order in &self.orders {
            let order_price = order.price().as_u128();
            if order_price != level_price {
                return Err(topology_price_error(order_price, level_price));
            }
            match side {
                None => side = Some(order.side()),
                Some(level_side) if level_side != order.side() => {
                    return Err(topology_side_error(order.side(), level_side));
                }
                Some(_) => {}
            }
        }

        // The statistics are moved, not cloned; restart their private seqlock
        // sequence as the former clone did, so a restore still rebuilds a
        // level whose sequence was near exhaustion (issue #165).
        let mut statistics = self.statistics;
        statistics.restart_seq_exclusive();
        Ok(ValidatedRestore {
            price: self.price,
            aggregates,
            side,
            orders: self.orders,
            statistics,
        })
    }
}

/// Builds the typed error for a snapshot aggregate that does not fit `u64`.
#[cold]
fn aggregate_overflow(message: &str) -> PriceLevelError {
    PriceLevelError::InvalidOperation {
        message: message.to_string(),
    }
}

/// Format version for checksum-enabled price level snapshots. New packages are
/// written at this version.
///
/// - **Version 1** carried no statistics — rejected by
///   [`PriceLevelSnapshotPackage::validate`] with a version mismatch.
/// - **Version 2** (issue #63) persists per-level [`PriceLevelStatistics`] as an
///   8-field statistics payload (no `stats_degraded`).
/// - **Version 3** (issue #129) owns the optional 9th `stats_degraded`
///   statistics field. A degraded level (which serializes that field) is a v3
///   payload, so it is not mislabelled v2 where an old 8-field-only reader would
///   choke on the unknown field.
/// - **Version 4** (issue #140) is the current shape: statistics
///   `value_executed` is a `u128` (it was `u64`). A v4 payload may carry a value
///   above `u64::MAX` that a v3 reader cannot represent, so new packages are
///   labelled v4. A pre-0.10 reader rejects every v4 package, but not always
///   by version: it deserializes the whole package before `validate` checks the
///   version, so a v4 package whose value fits in `u64` fails with a version
///   mismatch, while one whose value exceeds `u64::MAX` already fails to decode
///   with a deserialization error. Either way the old reader returns an error
///   and never restores wrong statistics.
///
/// [`PriceLevelSnapshotPackage::validate`] accepts v2 (legacy, 8-field,
/// `stats_degraded` defaults `false`), v3 and v4, so old snapshots keep
/// restoring; v1 is still rejected. Checksum recomputation is version-agnostic:
/// the statistics field set and the JSON encoding of a `value_executed` that
/// fits in `u64` are unchanged, so a legacy v2 / v3 package re-serializes to the
/// same bytes and its SHA-256 still matches.
pub const SNAPSHOT_FORMAT_VERSION: u32 = 4;

/// The set of snapshot format versions [`PriceLevelSnapshotPackage::validate`]
/// accepts on restore: the current [`SNAPSHOT_FORMAT_VERSION`] (v4) and the
/// legacy v2 (issue #129) and v3 (issue #140). v1 (statistics-less) is not
/// accepted.
const SUPPORTED_SNAPSHOT_VERSIONS: &[u32] = &[2, 3, 4];

/// Serialized representation of a price level snapshot including checksum validation metadata.
///
/// All fields are private to protect checksum integrity.
/// Use the provided accessor methods to read package data.
///
/// As with [`PriceLevelSnapshot`], the derived [`Clone`] allocates
/// infallibly (the trait cannot report a refused allocation);
/// [`PriceLevelSnapshotPackage::try_clone`] is the fallible copy (issue
/// #164).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceLevelSnapshotPackage {
    /// Version of the serialized snapshot schema to support future migrations.
    version: u32,
    /// Captured snapshot data.
    snapshot: PriceLevelSnapshot,
    /// Hex-encoded checksum used to validate the snapshot integrity.
    #[serde(deserialize_with = "deserialize_checksum")]
    checksum: String,
}

impl PriceLevelSnapshotPackage {
    /// Returns the schema version of this package.
    #[must_use]
    pub fn version(&self) -> u32 {
        self.version
    }

    /// Returns a reference to the contained snapshot.
    #[must_use]
    pub fn snapshot(&self) -> &PriceLevelSnapshot {
        &self.snapshot
    }

    /// Returns the hex-encoded checksum.
    #[must_use]
    pub fn checksum(&self) -> &str {
        &self.checksum
    }

    /// Fallible owned copy (issue #164): the snapshot is copied with
    /// [`PriceLevelSnapshot::try_clone`] and the checksum string through
    /// `try_reserve_exact`.
    ///
    /// # Errors
    ///
    /// [`PriceLevelError::CapacityExceeded`] (resource
    /// [`CapacityResource::OrderSnapshot`] or
    /// [`CapacityResource::SerializationBuffer`]) if a buffer cannot be
    /// reserved; `self` is only read.
    pub fn try_clone(&self) -> Result<Self, PriceLevelError> {
        Ok(Self {
            version: self.version,
            snapshot: self.snapshot.try_clone()?,
            checksum: try_copy_str(&self.checksum, CapacityResource::SerializationBuffer)?,
        })
    }
}

impl PriceLevelSnapshotPackage {
    /// Creates a new snapshot package computing the checksum for the provided snapshot.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::InvalidOperation`] if refreshing the snapshot
    /// aggregates overflows a quantity, [`PriceLevelError::CapacityExceeded`]
    /// (resource [`CapacityResource::SerializationBuffer`]) if the hex checksum
    /// cannot be reserved (issue #164), or
    /// [`PriceLevelError::SerializationError`] if the snapshot payload cannot
    /// be encoded while computing its SHA-256 checksum.
    pub fn new(mut snapshot: PriceLevelSnapshot) -> Result<Self, PriceLevelError> {
        snapshot.refresh_aggregates()?;

        let checksum = Self::compute_checksum(&snapshot)?;

        Ok(Self {
            version: SNAPSHOT_FORMAT_VERSION,
            snapshot,
            checksum,
        })
    }

    /// Serializes the package to JSON.
    ///
    /// This is a full serialization pass of the snapshot, separate from the
    /// hashing pass [`Self::new`] already made: building a package and then
    /// calling this (as [`crate::PriceLevel::snapshot_to_json`] does) encodes
    /// the snapshot twice (issue #149). Only this pass writes bytes to a
    /// buffer; the hashing pass streams into SHA-256.
    ///
    /// The output buffer grows through `try_reserve` (issue #164): a refused
    /// reservation stops the encoding and is reported as the fixed-size
    /// [`PriceLevelError::CapacityExceeded`], not as an allocation abort.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::CapacityExceeded`] (resource
    /// [`CapacityResource::SerializationBuffer`]) if the output buffer cannot
    /// grow, and [`PriceLevelError::SerializationError`] if the package cannot
    /// otherwise be encoded to a JSON string.
    pub fn to_json(&self) -> Result<String, PriceLevelError> {
        let mut writer = FallibleWriter::try_with_capacity(JSON_INITIAL_CAPACITY)?;
        if let Err(error) = serde_json::to_writer(&mut writer, self) {
            // A refused reservation is reported as its typed, allocation-free
            // error; `serde_json`'s own wrapper of the `io::Error` is dropped.
            if let Some(failure) = writer.take_failure() {
                return Err(failure);
            }
            return Err(PriceLevelError::SerializationError {
                message: error.to_string(),
            });
        }
        // `serde_json` only emits UTF-8; the conversion reuses the buffer.
        String::from_utf8(writer.into_inner()).map_err(|error| {
            PriceLevelError::SerializationError {
                message: error.to_string(),
            }
        })
    }

    /// Deserializes a package from JSON.
    ///
    /// The package's own collections (the orders vector and the checksum
    /// string) are decoded through fallible reservations (issue #164); a
    /// refusal surfaces through `serde_json`'s error as a
    /// [`PriceLevelError::DeserializationError`] whose message names the
    /// capacity failure. Transient buffers internal to `serde_json` are
    /// outside this crate (see `doc/panic-boundaries.md`).
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::DeserializationError`] if `data` is not a
    /// valid JSON representation of a snapshot package, or if one of the
    /// package's collections cannot be reserved. The returned package is
    /// not yet checksum-validated; call [`Self::validate`] or
    /// [`Self::into_snapshot`] to verify integrity.
    pub fn from_json(data: &str) -> Result<Self, PriceLevelError> {
        serde_json::from_str(data).map_err(|error| PriceLevelError::DeserializationError {
            message: error.to_string(),
        })
    }

    /// Validates the checksum contained in the package against the serialized snapshot data.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::InvalidOperation`] if the package's format
    /// version is not one of the supported versions (v2, v3, v4),
    /// [`PriceLevelError::SerializationError`] if the snapshot payload cannot be re-encoded to recompute the checksum,
    /// [`PriceLevelError::CapacityExceeded`] (resource
    /// [`CapacityResource::SerializationBuffer`]) if the recomputed hex
    /// checksum or the copy of the stored one cannot be reserved (issue #164),
    /// and [`PriceLevelError::ChecksumMismatch`] if the recomputed SHA-256
    /// checksum does not match the stored one (tampered or corrupted snapshot).
    // Snapshot restoration / validation is a cold path: keep it out of line.
    #[inline(never)]
    pub fn validate(&self) -> Result<(), PriceLevelError> {
        if !SUPPORTED_SNAPSHOT_VERSIONS.contains(&self.version) {
            return Err(PriceLevelError::InvalidOperation {
                message: format!(
                    "Unsupported snapshot version: {} (expected one of {:?})",
                    self.version, SUPPORTED_SNAPSHOT_VERSIONS
                ),
            });
        }

        let computed = Self::compute_checksum(&self.snapshot)?;
        if computed != self.checksum {
            // The stored checksum is input-sized: copy it fallibly (#164).
            return Err(PriceLevelError::ChecksumMismatch {
                expected: try_copy_str(&self.checksum, CapacityResource::SerializationBuffer)?,
                actual: computed,
            });
        }

        Ok(())
    }

    /// Consumes the package after validating the checksum and returns the contained snapshot.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::validate`]:
    /// [`PriceLevelError::InvalidOperation`] on an unsupported format version,
    /// [`PriceLevelError::SerializationError`] if the payload cannot be
    /// re-encoded, [`PriceLevelError::CapacityExceeded`] if a checksum buffer
    /// cannot be reserved, and [`PriceLevelError::ChecksumMismatch`] if the
    /// stored checksum does not match the recomputed one.
    pub fn into_snapshot(self) -> Result<PriceLevelSnapshot, PriceLevelError> {
        self.validate()?;
        Ok(self.snapshot)
    }

    #[inline(never)]
    fn compute_checksum(snapshot: &PriceLevelSnapshot) -> Result<String, PriceLevelError> {
        use std::fmt::Write as _;

        // Stream the JSON payload straight into the hasher (issues #149 /
        // #164): no payload buffer is materialized, so there is no growth to
        // fail, and the orders go through `BorrowedOrders` (no reference
        // vector). Byte equivalence with the former buffered encoding is
        // pinned by `tests/snapshot_equivalence.rs`.
        // `serde_json::to_vec` is `to_writer` into a `Vec` with the same
        // compact formatter, so the hashed bytes, and therefore the checksum,
        // are identical to the former buffered encoding.
        let mut hasher = Sha256::new();
        serde_json::to_writer(DigestWriter(&mut hasher), snapshot).map_err(|error| {
            PriceLevelError::SerializationError {
                message: error.to_string(),
            }
        })?;

        // `digest` 0.11 returns the digest as a `hybrid_array::Array`, which —
        // unlike the `generic_array::GenericArray` from 0.10 — does not
        // implement `LowerHex`. Encode the raw SHA-256 bytes to lowercase hex
        // by hand. The bytes are defined by the algorithm and are unchanged, so
        // the produced checksum string is byte-identical to the 0.10 output.
        let checksum_bytes = hasher.finalize();
        // Two hex digits per byte. The digest length is fixed (32), but the
        // capacity product is still checked and the buffer reserved fallibly
        // (issue #164); the writes below then fit the reservation exactly and
        // never grow the string.
        let hex_len = checksum_bytes
            .len()
            .checked_mul(2)
            .ok_or_else(|| capacity_error(CapacityResource::SerializationBuffer, usize::MAX))?;
        let mut checksum = String::new();
        try_reserve_string(
            &mut checksum,
            hex_len,
            CapacityResource::SerializationBuffer,
        )?;
        for byte in checksum_bytes {
            // Writing to a `String` is infallible; `{byte:02x}` is the same
            // lowercase, zero-padded, two-hex-digits-per-byte encoding the
            // previous `format!("{:x}", checksum_bytes)` produced.
            let _ = write!(checksum, "{byte:02x}");
        }
        Ok(checksum)
    }
}

/// Initial JSON output reservation, matching `serde_json::to_string`.
const JSON_INITIAL_CAPACITY: usize = 128;

/// An [`std::io::Write`] adapter feeding every byte into a SHA-256 hasher, so
/// the checksum payload is never buffered (issue #164). Infallible: hashing
/// allocates nothing.
struct DigestWriter<'a>(&'a mut Sha256);

impl std::io::Write for DigestWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Serializes a borrowed orders slice as a JSON-style sequence of plain
/// orders, with no intermediate vector (issue #164; replaces the
/// `Vec<&OrderType<()>>` collected for issue #72). `Serialize for &T`
/// forwards to `T`, so the output is byte-identical to serializing a
/// `Vec<OrderType<()>>`: checksums and round-trips are unchanged.
pub(crate) struct BorrowedOrders<'a>(pub(crate) &'a [Arc<OrderType<()>>]);

impl Serialize for BorrowedOrders<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_seq(self.0.iter().map(Arc::as_ref))
    }
}

/// Upper bound, in bytes, of the up-front reservation a decoded sequence may
/// take from its (input-controlled) length hint (issue #164). Longer
/// sequences still decode: they grow element by element through the same
/// fallible path. Mirrors `serde`'s own "cautious" pre-allocation rule.
const MAX_DECODE_PREALLOC_BYTES: usize = 1 << 20;

/// An element type a decoded order sequence can hold.
pub(crate) trait DecodedOrder: Sized {
    /// Wraps one decoded order.
    fn wrap(order: OrderType<()>) -> Self;
}

impl DecodedOrder for OrderType<()> {
    #[inline]
    fn wrap(order: OrderType<()>) -> Self {
        order
    }
}

impl DecodedOrder for Arc<OrderType<()>> {
    #[inline]
    fn wrap(order: OrderType<()>) -> Self {
        Arc::new(order)
    }
}

/// Decodes a sequence of plain orders into a `Vec<U>` whose every growth goes
/// through `try_reserve` (issue #164). A refusal is reported through the
/// deserializer's error type as the fixed-size
/// [`PriceLevelError::CapacityExceeded`] (resource
/// [`CapacityResource::OrderSnapshot`]).
pub(crate) struct OrdersSeed<U>(std::marker::PhantomData<fn() -> U>);

impl<U> OrdersSeed<U> {
    pub(crate) fn new() -> Self {
        Self(std::marker::PhantomData)
    }
}

impl<'de, U: DecodedOrder> DeserializeSeed<'de> for OrdersSeed<U> {
    type Value = Vec<U>;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_seq(self)
    }
}

impl<'de, U: DecodedOrder> Visitor<'de> for OrdersSeed<U> {
    type Value = Vec<U>;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a sequence of orders")
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut out: Vec<U> = Vec::new();
        let cap = MAX_DECODE_PREALLOC_BYTES
            .checked_div(std::mem::size_of::<U>())
            .unwrap_or(0);
        let hint = seq.size_hint().unwrap_or(0).min(cap);
        try_reserve_vec(&mut out, hint, CapacityResource::OrderSnapshot)
            .map_err(de::Error::custom)?;
        while let Some(order) = seq.next_element::<OrderType<()>>()? {
            try_push_vec(&mut out, U::wrap(order), CapacityResource::OrderSnapshot)
                .map_err(de::Error::custom)?;
        }
        Ok(out)
    }
}

/// `deserialize_with` helper for `PriceLevelData::orders`: the fallible
/// [`OrdersSeed`] decode (issue #164).
pub(crate) fn deserialize_plain_orders<'de, D>(
    deserializer: D,
) -> Result<Vec<OrderType<()>>, D::Error>
where
    D: Deserializer<'de>,
{
    OrdersSeed::<OrderType<()>>::new().deserialize(deserializer)
}

/// `deserialize_with` helper for the package checksum: a borrowed or
/// transient string is copied through `try_reserve_exact` (issue #164); an
/// owned string handed over by the deserializer is taken as is.
fn deserialize_checksum<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    struct ChecksumVisitor;

    impl Visitor<'_> for ChecksumVisitor {
        type Value = String;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a hex checksum string")
        }

        fn visit_str<E>(self, value: &str) -> Result<String, E>
        where
            E: de::Error,
        {
            try_copy_str(value, CapacityResource::SerializationBuffer).map_err(E::custom)
        }

        fn visit_string<E>(self, value: String) -> Result<String, E>
        where
            E: de::Error,
        {
            Ok(value)
        }
    }

    deserializer.deserialize_string(ChecksumVisitor)
}

impl Serialize for PriceLevelSnapshot {
    // Snapshot serialization is a cold path (taken/restored, not per-match):
    // keep it out of line.
    #[inline(never)]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("PriceLevelSnapshot", 6)?;

        state.serialize_field("price", &self.price)?;
        state.serialize_field("visible_quantity", &self.visible_quantity)?;
        state.serialize_field("hidden_quantity", &self.hidden_quantity)?;
        state.serialize_field("order_count", &self.order_count)?;

        // Serialize the borrowed orders rather than deep-copying every
        // `OrderType<()>` by value (issue #72), and without collecting even
        // the pointers into a vector (issue #164): `BorrowedOrders` streams
        // the slice. `Serialize for &T` forwards to `T`'s impl, so the output
        // is byte-identical to the previous `Vec<OrderType<()>>`; the
        // checksum and round-trip are unchanged.
        state.serialize_field("orders", &BorrowedOrders(&self.orders))?;
        state.serialize_field("statistics", &self.statistics)?;

        state.end()
    }
}

impl<'de> Deserialize<'de> for PriceLevelSnapshot {
    // Snapshot restoration is a cold path (taken/restored, not per-match):
    // keep it out of line.
    #[inline(never)]
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        enum Field {
            Price,
            VisibleQuantity,
            HiddenQuantity,
            OrderCount,
            Orders,
            Statistics,
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
                        formatter.write_str("`price`, `visible_quantity`, `hidden_quantity`, `order_count`, `orders`, or `statistics`")
                    }

                    fn visit_str<E>(self, value: &str) -> Result<Field, E>
                    where
                        E: de::Error,
                    {
                        match value {
                            "price" => Ok(Field::Price),
                            "visible_quantity" => Ok(Field::VisibleQuantity),
                            "hidden_quantity" => Ok(Field::HiddenQuantity),
                            "order_count" => Ok(Field::OrderCount),
                            "orders" => Ok(Field::Orders),
                            "statistics" => Ok(Field::Statistics),
                            _ => Err(de::Error::unknown_field(
                                value,
                                &[
                                    "price",
                                    "visible_quantity",
                                    "hidden_quantity",
                                    "order_count",
                                    "orders",
                                    "statistics",
                                ],
                            )),
                        }
                    }
                }

                deserializer.deserialize_identifier(FieldVisitor)
            }
        }

        struct PriceLevelSnapshotVisitor;

        impl<'de> Visitor<'de> for PriceLevelSnapshotVisitor {
            type Value = PriceLevelSnapshot;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("struct PriceLevelSnapshot")
            }

            fn visit_map<V>(self, mut map: V) -> Result<PriceLevelSnapshot, V::Error>
            where
                V: MapAccess<'de>,
            {
                let mut price = None;
                let mut visible_quantity = None;
                let mut hidden_quantity = None;
                let mut order_count = None;
                let mut orders = None;
                let mut statistics = None;

                while let Some(key) = map.next_key()? {
                    match key {
                        Field::Price => {
                            if price.is_some() {
                                return Err(de::Error::duplicate_field("price"));
                            }
                            price = Some(map.next_value()?);
                        }
                        Field::VisibleQuantity => {
                            if visible_quantity.is_some() {
                                return Err(de::Error::duplicate_field("visible_quantity"));
                            }
                            visible_quantity = Some(map.next_value()?);
                        }
                        Field::HiddenQuantity => {
                            if hidden_quantity.is_some() {
                                return Err(de::Error::duplicate_field("hidden_quantity"));
                            }
                            hidden_quantity = Some(map.next_value()?);
                        }
                        Field::OrderCount => {
                            if order_count.is_some() {
                                return Err(de::Error::duplicate_field("order_count"));
                            }
                            order_count = Some(map.next_value()?);
                        }
                        Field::Orders => {
                            if orders.is_some() {
                                return Err(de::Error::duplicate_field("orders"));
                            }
                            // Decoded straight into the `Arc` vector through
                            // fallible growth (issue #164): no intermediate
                            // plain-order vector, no infallible `collect`.
                            orders =
                                Some(map.next_value_seed(OrdersSeed::<Arc<OrderType<()>>>::new())?);
                        }
                        Field::Statistics => {
                            if statistics.is_some() {
                                return Err(de::Error::duplicate_field("statistics"));
                            }
                            statistics = Some(map.next_value()?);
                        }
                    }
                }

                let price = price.ok_or_else(|| de::Error::missing_field("price"))?;
                let visible_quantity =
                    visible_quantity.ok_or_else(|| de::Error::missing_field("visible_quantity"))?;
                let hidden_quantity =
                    hidden_quantity.ok_or_else(|| de::Error::missing_field("hidden_quantity"))?;
                let order_count =
                    order_count.ok_or_else(|| de::Error::missing_field("order_count"))?;
                // `unwrap_or_default` of a missing field: an empty `Vec` and
                // empty statistics, neither of which allocates (issue #164
                // audit).
                let orders = orders.unwrap_or_default();
                // `statistics` is optional on deserialize so a payload that omits
                // it (e.g. a hand-built fixture) restores with empty, unstamped
                // (deterministic, clock-free) statistics
                // rather than failing — i.e. tolerant of a *missing* field, not
                // forward-compatible with future *added* fields (this visitor
                // still rejects unknown fields). A genuine v1 *package* is
                // rejected up-front by `validate()`'s version check regardless.
                let statistics = statistics.unwrap_or_default();

                Ok(PriceLevelSnapshot {
                    price,
                    visible_quantity,
                    hidden_quantity,
                    order_count,
                    orders,
                    statistics,
                })
            }
        }

        const FIELDS: &[&str] = &[
            "price",
            "visible_quantity",
            "hidden_quantity",
            "order_count",
            "orders",
            "statistics",
        ];
        deserializer.deserialize_struct("PriceLevelSnapshot", FIELDS, PriceLevelSnapshotVisitor)
    }
}

impl fmt::Display for PriceLevelSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "PriceLevelSnapshot:price={};visible_quantity={};hidden_quantity={};order_count={}",
            self.price, self.visible_quantity, self.hidden_quantity, self.order_count
        )
    }
}

impl FromStr for PriceLevelSnapshot {
    type Err = PriceLevelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Exactly one `:` separates the `PriceLevelSnapshot` tag from the field list.
        let fields_str = match split_exactly_once(s, b':') {
            Some(("PriceLevelSnapshot", fields_str)) => fields_str,
            _ => return Err(PriceLevelError::InvalidFormat),
        };

        // `key=value` pairs: a pair without exactly one `=` is ignored and a
        // repeated key keeps its last value (see `utils::text::Fields`).
        const FIELD_NAMES: [&str; 4] = [
            "price",
            "visible_quantity",
            "hidden_quantity",
            "order_count",
        ];
        let fields = Fields::parse(fields_str, &FIELD_NAMES);
        let get_field = |name: &str| fields.require(name);

        let parse_u64 = |field: &str, value: &str| -> Result<u64, PriceLevelError> {
            value
                .parse::<u64>()
                .map_err(|_| PriceLevelError::InvalidFieldValue {
                    field: field.to_string(),
                    value: value.to_string(),
                })
        };

        let parse_u128 = |field: &str, value: &str| -> Result<u128, PriceLevelError> {
            value
                .parse::<u128>()
                .map_err(|_| PriceLevelError::InvalidFieldValue {
                    field: field.to_string(),
                    value: value.to_string(),
                })
        };

        let parse_usize = |field: &str, value: &str| -> Result<usize, PriceLevelError> {
            value
                .parse::<usize>()
                .map_err(|_| PriceLevelError::InvalidFieldValue {
                    field: field.to_string(),
                    value: value.to_string(),
                })
        };

        // Parse fields
        let price_str = get_field("price")?;
        let price = parse_u128("price", price_str)?;

        let visible_quantity_str = get_field("visible_quantity")?;
        let visible_quantity = parse_u64("visible_quantity", visible_quantity_str)?;

        let hidden_quantity_str = get_field("hidden_quantity")?;
        let hidden_quantity = parse_u64("hidden_quantity", hidden_quantity_str)?;

        let order_count_str = get_field("order_count")?;
        let order_count = parse_usize("order_count", order_count_str)?;

        // Create a new snapshot - note that orders and statistics cannot be
        // serialized/deserialized in this simple, human-readable format. Use the
        // JSON snapshot package for a lossless round-trip.
        Ok(PriceLevelSnapshot {
            price: Price::new(price),
            visible_quantity: Quantity::new(visible_quantity),
            hidden_quantity: Quantity::new(hidden_quantity),
            order_count,
            orders: Vec::new(),
            statistics: PriceLevelStatistics::new(),
        })
    }
}
