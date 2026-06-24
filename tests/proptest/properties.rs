//! The nine named price-level invariants, expressed as `proptest` properties
//! driving a single [`PriceLevel`](pricelevel::PriceLevel) through its public
//! API (issue #80).
//!
//! Each property builds a randomly generated, valid book of resting makers, runs
//! one or more `match_order` sweeps against it, and asserts the named invariant
//! on the resulting level state and the emitted [`MatchResult`] / [`Trade`]
//! values. The matcher reads no wall clock and no RNG, so a fixed input always
//! replays to a fixed trade stream — `proptest` shrinks deterministically.

use std::collections::HashSet;

use pricelevel::prelude::*;
use proptest::prelude::*;

use crate::strategies::{
    LEVEL_PRICE, Maker, TRADE_ID_NAMESPACE, book_strategy, book_total, taker_tif_strategy,
};

/// Shared config: bound the case count so the harness stays out of the unit
/// `cargo test` hot loop's time budget while still exercising a wide surface.
/// `max_shrink_iters` is raised because matching counterexamples often need
/// aggressive shrinking to reach a minimal failing book.
fn config(cases: u32) -> ProptestConfig {
    ProptestConfig {
        cases,
        max_shrink_iters: 50_000,
        ..ProptestConfig::default()
    }
}

/// Fresh, deterministic trade-id generator (fixed namespace).
fn trade_ids() -> UuidGenerator {
    UuidGenerator::new(TRADE_ID_NAMESPACE)
}

/// Rests every maker in `makers` on a fresh level and returns the level.
fn build_level(makers: &[Maker]) -> PriceLevel {
    let level = PriceLevel::new(LEVEL_PRICE);
    for maker in makers {
        // `OrderType<()>` is `Copy`, so this rests a copy of the maker order.
        let _ = level.add_order(maker.order);
    }
    level
}

/// Sum of trade quantities in a result, checked for overflow.
fn traded_quantity(result: &MatchResult) -> Result<u64, TestCaseError> {
    let mut total: u64 = 0;
    for trade in result.trades().as_vec() {
        total = total
            .checked_add(trade.quantity().as_u64())
            .ok_or_else(|| TestCaseError::fail("traded quantity overflow"))?;
    }
    Ok(total)
}

proptest! {
    #![proptest_config(config(256))]

    // ---------------------------------------------------------------------
    // 1. Quantity conservation.
    //
    // After a single match sweep, the resting depth left in the level plus the
    // quantity traded away equals the initial resting total. Equivalently the
    // depth dropped by exactly the traded quantity. The advisory counters are
    // exact under a single matcher, so `total_quantity()` equals the sum over
    // the queue contents.
    // ---------------------------------------------------------------------
    #[test]
    fn prop_quantity_conserved(
        (_side, makers) in book_strategy(1..=12),
        taker_qty in 1u64..=2_000u64,
    ) {
        let level = build_level(&makers);
        let generator = trade_ids();

        let initial_total = book_total(&makers);
        // Advisory counter equals the queue contents before any match (single
        // matcher, no concurrency).
        let counter_total = level
            .total_quantity()
            .map_err(|e| TestCaseError::fail(format!("total_quantity: {e}")))?;
        prop_assert_eq!(counter_total, initial_total);

        let result = level.match_order(
            taker_qty,
            Id::from_u64(1_000_000),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_700_000_000_000),
            &generator,
        );

        let traded = traded_quantity(&result)?;
        let remaining_depth = level
            .total_quantity()
            .map_err(|e| TestCaseError::fail(format!("total_quantity: {e}")))?;

        // Conservation: nothing is created or destroyed by a match.
        prop_assert_eq!(
            remaining_depth + traded,
            initial_total,
            "resting depth + traded must equal the initial resting total"
        );

        // Counter equals the queue contents after the match: sum visible+hidden
        // over the live orders and compare to the atomic total.
        let mut queue_visible: u64 = 0;
        let mut queue_hidden: u64 = 0;
        for order in level.iter_orders() {
            queue_visible += order.visible_quantity().as_u64();
            queue_hidden += order.hidden_quantity().as_u64();
        }
        prop_assert_eq!(level.visible_quantity(), queue_visible);
        prop_assert_eq!(level.hidden_quantity(), queue_hidden);
        prop_assert_eq!(remaining_depth, queue_visible + queue_hidden);
    }

    // ---------------------------------------------------------------------
    // 2. No zero-quantity orders rest or trade.
    //
    // Every emitted trade has positive quantity, and no order left resting has
    // a zero `visible + hidden` total. `order_count()` equals the number of
    // live orders in the queue.
    // ---------------------------------------------------------------------
    #[test]
    fn prop_no_zero_quantity(
        (_side, makers) in book_strategy(1..=12),
        taker_qty in 1u64..=2_000u64,
    ) {
        let level = build_level(&makers);
        let generator = trade_ids();

        let result = level.match_order(
            taker_qty,
            Id::from_u64(1_000_000),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_700_000_000_000),
            &generator,
        );

        for trade in result.trades().as_vec() {
            prop_assert!(
                trade.quantity().as_u64() > 0,
                "a trade must never carry zero quantity"
            );
        }

        let mut live = 0usize;
        for order in level.iter_orders() {
            let total = order.visible_quantity().as_u64() + order.hidden_quantity().as_u64();
            prop_assert!(total > 0, "a resting order must never have zero total quantity");
            live += 1;
        }
        prop_assert_eq!(level.order_count(), live);
    }

    // ---------------------------------------------------------------------
    // 3. FIFO / price-time priority.
    //
    // Makers are consumed oldest-first. With distinct sequential ids 1..=n
    // assigned in insertion order, the *first time* each maker is touched by the
    // sweep must follow insertion order: the first-touch sequence is the
    // strictly increasing prefix 1, 2, ..., k with no older maker skipped before
    // a younger one is touched.
    //
    // A replenishing iceberg / reserve correctly loses time priority and is
    // re-queued at the TAIL, so it may legitimately be touched again *after* a
    // younger maker (e.g. touch order 1, 2, 1). That is price-time priority, not
    // a violation — hence the invariant is on FIRST touches, not on every touch.
    // ---------------------------------------------------------------------
    #[test]
    fn prop_fifo_price_time_priority(
        (_side, makers) in book_strategy(2..=12),
        taker_qty in 1u64..=2_000u64,
    ) {
        // Insertion order is the order the makers were added. Sequential ids
        // 1..=n were stamped in that order by the book strategy.
        let level = build_level(&makers);
        let generator = trade_ids();

        let result = level.match_order(
            taker_qty,
            Id::from_u64(1_000_000),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_700_000_000_000),
            &generator,
        );

        // First-touch order: the maker ids in the order each is first seen.
        let mut seen: HashSet<u64> = HashSet::new();
        let mut first_touch: Vec<u64> = Vec::new();
        for trade in result.trades().as_vec() {
            let id = trade
                .maker_order_id()
                .as_u64()
                .or_else(|| seq_from_uuid_id(trade.maker_order_id()))
                .ok_or_else(|| TestCaseError::fail("maker id must be recoverable"))?;
            if seen.insert(id) {
                first_touch.push(id);
            }
        }

        // First touches must be exactly 1, 2, ..., k — oldest-first, no maker
        // skipped before an older one is touched.
        for (expected, actual) in (1u64..).zip(first_touch.iter()) {
            prop_assert_eq!(
                expected,
                *actual,
                "makers must be first-touched oldest-first with no skipped maker"
            );
        }

        // A purely partially-filled front maker keeps the front. Assert this only
        // for a STANDARD front maker (makers[0], id 1): a partially-consumed
        // Standard order never replenishes, so it stays at the front. An
        // iceberg/reserve whose visible tranche is fully drawn correctly
        // replenishes from hidden and is re-queued at the TAIL (loses time
        // priority) — that is not a violation, so it is excluded here. The
        // guard conditions: exactly one maker touched, the taker fully filled
        // (`is_complete`), and no maker fully consumed (`filled_order_ids` empty)
        // — i.e. the taker took only part of the front Standard maker.
        if first_touch.len() == 1
            && matches!(makers.first().map(|m| &m.order), Some(OrderType::Standard { .. }))
            && result.is_complete()
            && result.filled_order_ids().is_empty()
            && let Some(front) = level.snapshot_orders().first()
            && let Some(front_id) = front.id().as_u64().or_else(|| seq_from_uuid_id(front.id()))
        {
            prop_assert_eq!(
                front_id,
                first_touch[0],
                "a partially filled front maker must keep the front"
            );
        }
    }

    // ---------------------------------------------------------------------
    // 4. MatchResult field agreement.
    //
    // `is_complete ⇔ remaining_quantity == 0`; `executed_quantity()` equals the
    // sum of trade quantities; `executed_value()` equals price * traded (all
    // trades at the level price); `filled_order_ids` is unique.
    // ---------------------------------------------------------------------
    #[test]
    fn prop_match_result_consistency(
        (_side, makers) in book_strategy(1..=12),
        taker_qty in 1u64..=2_000u64,
        taker_tif in taker_tif_strategy(),
    ) {
        let level = build_level(&makers);
        let generator = trade_ids();

        let result = level.match_order(
            taker_qty,
            Id::from_u64(1_000_000),
            taker_tif,
            TakerKind::Standard,
            TimestampMs::new(1_700_000_000_000),
            &generator,
        );

        prop_assert_eq!(
            result.is_complete(),
            result.remaining_quantity().as_u64() == 0,
            "is_complete must agree with remaining_quantity == 0"
        );

        let traded = traded_quantity(&result)?;
        let executed = result
            .executed_quantity()
            .map_err(|e| TestCaseError::fail(format!("executed_quantity: {e}")))?
            .as_u64();
        prop_assert_eq!(executed, traded, "executed_quantity must equal the trade sum");

        // taker_qty == traded + remaining for the sweeping outcomes (Gtc / Ioc
        // / Day at TakerKind::Standard never kill or reject).
        prop_assert_eq!(taker_qty, traded + result.remaining_quantity().as_u64());

        // executed_value == level price * traded (all fills at the level price).
        let executed_value = result
            .executed_value()
            .map_err(|e| TestCaseError::fail(format!("executed_value: {e}")))?;
        prop_assert_eq!(executed_value, LEVEL_PRICE * u128::from(traded));

        // filled_order_ids is unique.
        let filled = result.filled_order_ids();
        let unique: HashSet<&Id> = filled.iter().collect();
        prop_assert_eq!(unique.len(), filled.len(), "filled_order_ids must be unique");
    }

    // ---------------------------------------------------------------------
    // 5. Trade field agreement.
    //
    // For every trade: maker != taker; price == level price; quantity > 0;
    // taker_side == maker_side.opposite() (the whole book is on a known side).
    // ---------------------------------------------------------------------
    #[test]
    fn prop_trade_fields_agree(
        (side, makers) in book_strategy(1..=12),
        taker_qty in 1u64..=2_000u64,
    ) {
        let level = build_level(&makers);
        let generator = trade_ids();
        let taker_id = Id::from_u64(1_000_000);

        let result = level.match_order(
            taker_qty,
            taker_id,
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_700_000_000_000),
            &generator,
        );

        for trade in result.trades().as_vec() {
            prop_assert_ne!(
                trade.maker_order_id(),
                trade.taker_order_id(),
                "a trade must have distinct maker and taker"
            );
            prop_assert_eq!(trade.taker_order_id(), taker_id);
            prop_assert_eq!(trade.price().as_u128(), LEVEL_PRICE);
            prop_assert!(trade.quantity().as_u64() > 0);
            // Whole book is on `side`; the taker takes the opposite side.
            prop_assert_eq!(trade.taker_side(), side.opposite());
        }
    }

    // ---------------------------------------------------------------------
    // 6. IOC fills available and discards the remainder (never rests).
    //
    // An IOC taker larger than the resting depth fills exactly the matchable
    // depth and reports a positive remainder; no taker order is left resting
    // (the level's order count never exceeds the original maker count, and the
    // taker id never appears among resting orders).
    // ---------------------------------------------------------------------
    #[test]
    fn prop_ioc_partial_discards_remainder(
        (_side, makers) in book_strategy(1..=10),
        extra in 1u64..=500u64,
    ) {
        let level = build_level(&makers);
        let generator = trade_ids();
        let taker_id = Id::from_u64(1_000_000);

        let initial_total = book_total(&makers);
        // Oversized taker: strictly larger than the resting depth so a remainder
        // is guaranteed regardless of replenishment ordering.
        let taker_qty = initial_total + extra;

        let order_count_before = level.order_count();
        let result = level.match_order(
            taker_qty,
            taker_id,
            TimeInForce::Ioc,
            TakerKind::Standard,
            TimestampMs::new(1_700_000_000_000),
            &generator,
        );

        let traded = traded_quantity(&result)?;
        // The IOC taker took exactly all the resting depth.
        prop_assert_eq!(traded, initial_total, "IOC must fill all available depth");
        prop_assert_eq!(
            result.remaining_quantity().as_u64(),
            taker_qty - traded,
            "IOC remainder must be reported"
        );
        prop_assert!(result.remaining_quantity().as_u64() > 0);
        prop_assert!(!result.is_complete());

        // The taker never rests at this layer: order count cannot grow, and the
        // taker id is absent from the resting queue.
        prop_assert!(level.order_count() <= order_count_before);
        for order in level.iter_orders() {
            prop_assert_ne!(order.id(), taker_id, "the taker must never rest");
        }
    }

    // ---------------------------------------------------------------------
    // 7. FOK is all-or-nothing.
    //
    // Run a FOK taker and compare the level's JSON snapshot before and after:
    // either the taker fully filled (is_complete, zero remainder) OR it was
    // killed (MatchOutcome::Killed, zero trades, full remainder, queue
    // byte-identical to the pre-match snapshot).
    // ---------------------------------------------------------------------
    #[test]
    fn prop_fok_all_or_nothing(
        (_side, makers) in book_strategy(1..=10),
        taker_qty in 1u64..=2_000u64,
    ) {
        let level = build_level(&makers);
        let generator = trade_ids();

        let before = level
            .snapshot_to_json()
            .map_err(|e| TestCaseError::fail(format!("snapshot before: {e}")))?;

        let result = level.match_order(
            taker_qty,
            Id::from_u64(1_000_000),
            TimeInForce::Fok,
            TakerKind::Standard,
            TimestampMs::new(1_700_000_000_000),
            &generator,
        );

        if result.is_complete() {
            // Fully filled: remainder is zero and at least one trade occurred.
            prop_assert_eq!(result.remaining_quantity().as_u64(), 0);
            prop_assert!(!result.trades().is_empty());
            prop_assert_eq!(result.outcome(), MatchOutcome::Filled);
        } else {
            // Killed: zero trades, full remainder, queue untouched.
            prop_assert_eq!(result.outcome(), MatchOutcome::Killed);
            prop_assert!(result.was_killed());
            prop_assert!(result.trades().is_empty());
            prop_assert_eq!(result.remaining_quantity().as_u64(), taker_qty);
            let after = level
                .snapshot_to_json()
                .map_err(|e| TestCaseError::fail(format!("snapshot after: {e}")))?;
            prop_assert_eq!(before, after, "a killed FOK must leave the queue untouched");
        }
    }

    // ---------------------------------------------------------------------
    // 8. Iceberg / reserve replenishment.
    //
    // Rest a single iceberg (or auto-replenishing reserve) with hidden depth,
    // fire a taker that exactly drains the current visible tranche, and assert
    // the visible was refreshed from hidden (visible stays positive while
    // hidden remains, totals conserved: total dropped by exactly the consumed
    // visible).
    // ---------------------------------------------------------------------
    #[test]
    fn prop_iceberg_reserve_replenish(
        visible in 1u64..=80u64,
        hidden in 1u64..=300u64,
        use_reserve in any::<bool>(),
    ) {
        let level = PriceLevel::new(LEVEL_PRICE);
        let order = if use_reserve {
            OrderType::ReserveOrder {
                id: Id::from_u64(1),
                price: Price::new(LEVEL_PRICE),
                visible_quantity: Quantity::new(visible),
                hidden_quantity: Quantity::new(hidden),
                side: Side::Sell,
                user_id: Hash32::zero(),
                timestamp: TimestampMs::new(1_000),
                time_in_force: TimeInForce::Gtc,
                // Threshold >= visible so draining the visible always trips the
                // replenish, and auto so it happens inside the sweep.
                replenish_threshold: Quantity::new(visible),
                replenish_amount: None,
                auto_replenish: true,
                extra_fields: (),
            }
        } else {
            OrderType::IcebergOrder {
                id: Id::from_u64(1),
                price: Price::new(LEVEL_PRICE),
                visible_quantity: Quantity::new(visible),
                hidden_quantity: Quantity::new(hidden),
                side: Side::Sell,
                user_id: Hash32::zero(),
                timestamp: TimestampMs::new(1_000),
                time_in_force: TimeInForce::Gtc,
                extra_fields: (),
            }
        };
        let _ = level.add_order(order);
        let generator = trade_ids();

        let total_before = level
            .total_quantity()
            .map_err(|e| TestCaseError::fail(format!("total before: {e}")))?;
        prop_assert_eq!(total_before, visible + hidden);

        // Drain exactly the visible tranche so the next tranche is pulled.
        let result = level.match_order(
            visible,
            Id::from_u64(1_000_000),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_700_000_000_000),
            &generator,
        );

        let traded = traded_quantity(&result)?;
        prop_assert_eq!(traded, visible, "draining the visible tranche fills exactly the visible");

        // Totals conserved: dropped by exactly the consumed visible.
        let total_after = level
            .total_quantity()
            .map_err(|e| TestCaseError::fail(format!("total after: {e}")))?;
        prop_assert_eq!(total_after, (visible + hidden) - visible);
        prop_assert_eq!(total_after, hidden);

        // The order is still resting (hidden > 0 backs a fresh tranche) and its
        // visible was replenished from hidden.
        prop_assert_eq!(level.order_count(), 1);
        let mut saw_order = false;
        for resting in level.iter_orders() {
            saw_order = true;
            prop_assert!(
                resting.visible_quantity().as_u64() > 0,
                "replenished order must expose a fresh visible tranche"
            );
            prop_assert_eq!(
                resting.visible_quantity().as_u64() + resting.hidden_quantity().as_u64(),
                hidden,
                "replenished order total must equal the remaining hidden depth"
            );
        }
        prop_assert!(saw_order, "the replenishing order must still rest");
    }

    // ---------------------------------------------------------------------
    // 9. Snapshot round-trip and tampered checksum.
    //
    // After an arbitrary book + match, the JSON snapshot restores to an
    // equivalent level (price, visible / hidden, order ids in order, stats), and
    // a tampered JSON payload is rejected with ChecksumMismatch.
    // ---------------------------------------------------------------------
    #[test]
    fn prop_snapshot_roundtrip(
        (_side, makers) in book_strategy(0..=12),
        taker_qty in 0u64..=2_000u64,
    ) {
        let level = build_level(&makers);
        let generator = trade_ids();

        if taker_qty > 0 {
            let _ = level.match_order(
                taker_qty,
                Id::from_u64(1_000_000),
                TimeInForce::Gtc,
                TakerKind::Standard,
                TimestampMs::new(1_700_000_000_000),
                &generator,
            );
        }

        let json = level
            .snapshot_to_json()
            .map_err(|e| TestCaseError::fail(format!("snapshot_to_json: {e}")))?;
        let restored = PriceLevel::from_snapshot_json(&json)
            .map_err(|e| TestCaseError::fail(format!("from_snapshot_json: {e}")))?;

        prop_assert_eq!(restored.price(), level.price());
        prop_assert_eq!(restored.visible_quantity(), level.visible_quantity());
        prop_assert_eq!(restored.hidden_quantity(), level.hidden_quantity());
        prop_assert_eq!(restored.order_count(), level.order_count());

        // Order order preserved: snapshot_orders is the deterministic view.
        let original_ids: Vec<Id> = level.snapshot_orders().iter().map(|o| o.id()).collect();
        let restored_ids: Vec<Id> = restored.snapshot_orders().iter().map(|o| o.id()).collect();
        prop_assert_eq!(&original_ids, &restored_ids, "order order must round-trip");

        // Statistics round-trip (executions recorded survive the snapshot).
        prop_assert_eq!(
            restored.stats().orders_executed(),
            level.stats().orders_executed(),
            "executed-order stat must round-trip"
        );

        // Tampered checksum is rejected. Flip a digit in the checksum field of
        // the JSON payload and assert ChecksumMismatch.
        let tampered = tamper_checksum(&json)
            .ok_or_else(|| TestCaseError::fail("could not locate checksum to tamper"))?;
        match PriceLevel::from_snapshot_json(&tampered) {
            Err(PriceLevelError::ChecksumMismatch { .. }) => {}
            other => {
                return Err(TestCaseError::fail(format!(
                    "tampered snapshot must yield ChecksumMismatch, got {other:?}"
                )));
            }
        }
    }
}

/// Recovers the sequential id encoded by [`Id::from_u64`], which stores the
/// value in the first 8 bytes of a UUID. The maker / front ids stamped by the
/// book strategy use `Id::from_u64`, so this reverses that encoding for FIFO
/// comparison.
fn seq_from_uuid_id(id: Id) -> Option<u64> {
    let bytes = id.as_bytes();
    // `Id::from_u64` packs the value big-endian into bytes[0..8] and zero-fills
    // the rest. Reject anything that doesn't fit that shape.
    if bytes[8..].iter().any(|&b| b != 0) {
        return None;
    }
    let mut value = [0u8; 8];
    value.copy_from_slice(&bytes[0..8]);
    Some(u64::from_be_bytes(value))
}

/// Flips one hex digit inside the `"checksum":"..."` field of a snapshot package
/// JSON so the recomputed SHA-256 no longer matches. Returns `None` if the field
/// is absent (it never is for a real snapshot package).
fn tamper_checksum(json: &str) -> Option<String> {
    let key = "\"checksum\":\"";
    let start = json.find(key)? + key.len();
    let end = start + json[start..].find('"')?;
    let original = &json[start..end];
    // Flip the first hex digit deterministically: 'a' if it was '0', else '0'.
    let mut chars: Vec<char> = original.chars().collect();
    let first = *chars.first()?;
    chars[0] = if first == '0' { 'a' } else { '0' };
    let tampered_checksum: String = chars.into_iter().collect();
    // Guard: tampering must actually change the checksum.
    if tampered_checksum == original {
        return None;
    }
    let mut out = String::with_capacity(json.len());
    out.push_str(&json[..start]);
    out.push_str(&tampered_checksum);
    out.push_str(&json[end..]);
    Some(out)
}
