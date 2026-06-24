//! Shared `proptest` strategies for the single price-level invariant harness.
//!
//! Every generated value passes through the validated crate newtypes
//! (`Price`, `Quantity`, `TimestampMs`, `Id`, `Side`, `TimeInForce`) plus the
//! taker discriminators (`TakerKind`) so the harness can never construct an
//! order or a match call the public API would reject. A single
//! [`PriceLevel`](pricelevel::PriceLevel) owns exactly one price, so every
//! generated order rests at [`LEVEL_PRICE`] — cross-price routing is an
//! order-book concern that does not exist at this layer.

use pricelevel::prelude::*;
use proptest::prelude::*;
use uuid::Uuid;

/// The single price every order in a one-level test shares, in price ticks.
pub const LEVEL_PRICE: u128 = 10_000;

/// Fixed namespace for the trade-id generator so a fixed input replays to a
/// fixed, deterministic trade stream (no wall-clock, no RNG in the matcher).
pub const TRADE_ID_NAMESPACE: Uuid = Uuid::nil();

/// A maker (resting) order plus the bookkeeping a property needs to reason
/// about it without re-reading the queue: its total resting quantity
/// (`visible + hidden`). The book's shared side is returned by
/// [`book_strategy`].
#[derive(Clone, Debug)]
pub struct Maker {
    /// The order to rest at the level.
    pub order: OrderType<()>,
    /// `visible + hidden` at construction, in quantity units. The book's shared
    /// side is returned by [`book_strategy`] rather than stored per maker.
    pub total: u64,
}

/// Builds an owner id from a small pool so same-owner orders are frequent.
fn owner(i: u8) -> Hash32 {
    let mut bytes = [0u8; 32];
    bytes[0] = i;
    Hash32::from(bytes)
}

/// Strategy over the resting-maker side.
pub fn side_strategy() -> impl Strategy<Value = Side> {
    prop_oneof![Just(Side::Buy), Just(Side::Sell)]
}

/// Strategy over a resting maker's time-in-force. Only the resting-relevant
/// policies (`Gtc` / `Gtd` / `Day`) are generated: maker expiry is not enforced
/// at this layer, so any of them simply rests, and `Ioc` / `Fok` are taker
/// concerns generated separately.
fn maker_tif_strategy() -> impl Strategy<Value = TimeInForce> {
    prop_oneof![
        Just(TimeInForce::Gtc),
        Just(TimeInForce::Day),
        (1u64..1_000_000u64).prop_map(TimeInForce::Gtd),
    ]
}

/// Strategy over a resting maker order, given a fixed side so a whole book can
/// be built on one side (the taker takes the opposite side).
///
/// Generates `Standard`, `IcebergOrder`, and `ReserveOrder` makers so the
/// conservation / FIFO / replenishment properties see the visible-only and the
/// visible+hidden shapes. Quantities are bounded small so short streams still
/// fill and the checked sums never approach `u64` overflow.
pub fn maker_strategy(side: Side) -> impl Strategy<Value = Maker> {
    let standard = (
        any::<u64>().prop_map(Id::from_u64),
        1u64..=200u64,
        0u8..4u8,
        maker_tif_strategy(),
        any::<u64>(),
    )
        .prop_map(move |(id, qty, owner_ix, tif, ts)| Maker {
            order: OrderType::Standard {
                id,
                price: Price::new(LEVEL_PRICE),
                quantity: Quantity::new(qty),
                side,
                user_id: owner(owner_ix),
                timestamp: TimestampMs::new(ts),
                time_in_force: tif,
                extra_fields: (),
            },
            total: qty,
        });

    let iceberg = (
        any::<u64>().prop_map(Id::from_u64),
        1u64..=100u64,
        1u64..=200u64,
        0u8..4u8,
        maker_tif_strategy(),
        any::<u64>(),
    )
        .prop_map(move |(id, visible, hidden, owner_ix, tif, ts)| Maker {
            order: OrderType::IcebergOrder {
                id,
                price: Price::new(LEVEL_PRICE),
                visible_quantity: Quantity::new(visible),
                hidden_quantity: Quantity::new(hidden),
                side,
                user_id: owner(owner_ix),
                timestamp: TimestampMs::new(ts),
                time_in_force: tif,
                extra_fields: (),
            },
            total: visible + hidden,
        });

    // Reserve makers here are always `auto_replenish: true`. A *non-auto*
    // reserve legitimately DISCARDS its remaining hidden depth when its visible
    // is fully consumed (the engine removes the order and strands the hidden) —
    // that depth is neither traded nor retained, which would break the
    // conservation / FIFO / IOC properties' "depth is conserved" assumption.
    // The non-auto discard is a distinct behavior best covered by an explicit
    // example test, not these generative properties; an auto-replenishing
    // reserve keeps every unit either traded or resting, so it is the honest
    // shape for the conservation harness.
    let reserve = (
        any::<u64>().prop_map(Id::from_u64),
        1u64..=100u64,
        1u64..=200u64,
        0u64..=50u64,
        0u8..4u8,
        maker_tif_strategy(),
        any::<u64>(),
    )
        .prop_map(
            move |(id, visible, hidden, threshold, owner_ix, tif, ts)| Maker {
                order: OrderType::ReserveOrder {
                    id,
                    price: Price::new(LEVEL_PRICE),
                    visible_quantity: Quantity::new(visible),
                    hidden_quantity: Quantity::new(hidden),
                    side,
                    user_id: owner(owner_ix),
                    timestamp: TimestampMs::new(ts),
                    time_in_force: tif,
                    replenish_threshold: Quantity::new(threshold),
                    replenish_amount: None,
                    auto_replenish: true,
                    extra_fields: (),
                },
                total: visible + hidden,
            },
        );

    prop_oneof![3 => standard, 2 => iceberg, 2 => reserve]
}

/// Strategy over a book of makers (size taken from the `len` range, so it may be
/// empty when the range includes 0), all on the same generated side and with
/// distinct sequential ids so a `Cancel` / FIFO test can address them.
///
/// Ids are assigned `1..=len` after generation (overwriting the strategy's
/// random id) to guarantee uniqueness within the book without rejection-sampling
/// a `HashSet`.
pub fn book_strategy(
    len: std::ops::RangeInclusive<usize>,
) -> impl Strategy<Value = (Side, Vec<Maker>)> {
    side_strategy().prop_flat_map(move |side| {
        proptest::collection::vec(maker_strategy(side), len.clone()).prop_map(move |mut makers| {
            for (i, maker) in makers.iter_mut().enumerate() {
                let id = Id::from_u64(i as u64 + 1);
                // Monotonic timestamp in insertion order so price-time priority
                // is unambiguous (snapshot (timestamp, seq) order == sweep seq
                // order). Earlier-inserted maker => earlier timestamp.
                let ts = TimestampMs::new(1_700_000_000_000 + i as u64);
                maker.order = with_id_and_ts(&maker.order, id, ts);
            }
            (side, makers)
        })
    })
}

/// Strategy over the taker time-in-force for the generic match properties.
pub fn taker_tif_strategy() -> impl Strategy<Value = TimeInForce> {
    prop_oneof![
        Just(TimeInForce::Gtc),
        Just(TimeInForce::Ioc),
        Just(TimeInForce::Day),
    ]
}

/// Rebuilds an order with a replaced id AND a replaced timestamp, preserving
/// every other field. Used to stamp unique sequential ids and
/// insertion-order-monotonic timestamps onto a generated book.
///
/// The monotonic timestamp is essential: price-time priority is only
/// well-defined when timestamps are monotonic with insertion order, and the
/// snapshot view is `(timestamp, sequence)`-ordered while the live sweep pops by
/// sequence. Stamping monotonic timestamps makes the two orderings coincide, so
/// "the front" is unambiguous for the FIFO properties. (The per-maker random
/// timestamp from `maker_strategy` is intentionally overwritten here.)
fn with_id_and_ts(order: &OrderType<()>, new_id: Id, new_ts: TimestampMs) -> OrderType<()> {
    match *order {
        OrderType::Standard {
            price,
            quantity,
            side,
            user_id,
            time_in_force,
            ..
        } => OrderType::Standard {
            id: new_id,
            price,
            quantity,
            side,
            user_id,
            timestamp: new_ts,
            time_in_force,
            extra_fields: (),
        },
        OrderType::IcebergOrder {
            price,
            visible_quantity,
            hidden_quantity,
            side,
            user_id,
            time_in_force,
            ..
        } => OrderType::IcebergOrder {
            id: new_id,
            price,
            visible_quantity,
            hidden_quantity,
            side,
            user_id,
            timestamp: new_ts,
            time_in_force,
            extra_fields: (),
        },
        OrderType::ReserveOrder {
            price,
            visible_quantity,
            hidden_quantity,
            side,
            user_id,
            time_in_force,
            replenish_threshold,
            replenish_amount,
            auto_replenish,
            ..
        } => OrderType::ReserveOrder {
            id: new_id,
            price,
            visible_quantity,
            hidden_quantity,
            side,
            user_id,
            timestamp: new_ts,
            time_in_force,
            replenish_threshold,
            replenish_amount,
            auto_replenish,
            extra_fields: (),
        },
        // The maker strategy only emits the three resting shapes above; any
        // other variant is left untouched. Unreachable for generated books.
        other => other,
    }
}

/// Sum of `total` across a book, in quantity units. The book strategy bounds
/// each maker's total to <= 300 over a small `len`, so this never overflows the
/// `u64` accumulator for any generated book.
#[must_use]
pub fn book_total(makers: &[Maker]) -> u64 {
    makers.iter().map(|m| m.total).sum()
}
