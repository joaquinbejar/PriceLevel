//! Issue #169 / #164 contract: `PriceLevel::match_order` when a resting
//! maker's `OrderType::match_against` returns a typed arithmetic error.
//!
//! The only reachable failure is a reserve whose own visible + hidden
//! overflows `u64` (the partial-fill replenish add). `add_order` rejects such
//! an order, so these tests rest it through the `test_rest_unadmitted` seam.
//! The real sweep must stop at that maker with the committed prefix and the
//! error; the fill-or-kill dry run must detect the same stop and kill the
//! taker with the level unchanged.

#[cfg(test)]
mod tests {
    use crate::UuidGenerator;
    use crate::errors::PriceLevelError;
    use crate::execution::{MatchOutcome, TakerKind};
    use crate::orders::{Hash32, Id, OrderType, Side, TimeInForce};
    use crate::price_level::level::PriceLevel;
    use crate::utils::{Price, Quantity, TimestampMs};
    use std::num::NonZeroU64;
    use std::sync::Arc;
    use uuid::Uuid;

    const PRICE: u128 = 10_000;
    const TAKER: u64 = 9_999;

    fn standard(id: u64, quantity: u64) -> OrderType<()> {
        OrderType::Standard {
            id: Id::from_u64(id),
            price: Price::new(PRICE),
            quantity: Quantity::new(quantity),
            side: Side::Sell,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1_616_823_000_000 + id),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        }
    }

    /// The issue #169 reserve: visible = hidden = threshold = `u64::MAX`,
    /// `auto_replenish`. Any partial fill overflows the replenish add.
    fn overflowing_reserve(id: u64) -> OrderType<()> {
        OrderType::ReserveOrder {
            id: Id::from_u64(id),
            price: Price::new(PRICE),
            visible_quantity: Quantity::new(u64::MAX),
            hidden_quantity: Quantity::new(u64::MAX),
            side: Side::Sell,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1_616_823_000_000 + id),
            time_in_force: TimeInForce::Gtc,
            replenish_threshold: Quantity::new(u64::MAX),
            replenish_amount: NonZeroU64::new(u64::MAX),
            auto_replenish: true,
            extra_fields: (),
        }
    }

    fn generator() -> UuidGenerator {
        UuidGenerator::new(Uuid::from_u128(169))
    }

    /// Level with: admitted standard 1 (qty 5), unadmitted overflowing reserve
    /// 2, admitted standard 3 (qty 5), in that FIFO order.
    fn level_with_failing_middle_maker() -> PriceLevel {
        let level = PriceLevel::new(PRICE);
        level.add_order(standard(1, 5)).expect("admit maker 1");
        level
            .test_rest_unadmitted(overflowing_reserve(2))
            .expect("rest failing maker 2");
        level.add_order(standard(3, 5)).expect("admit maker 3");
        level
    }

    fn queue(level: &PriceLevel) -> Vec<Arc<OrderType<()>>> {
        level.snapshot_orders()
    }

    fn assert_invalid_operation(err: Option<&PriceLevelError>) {
        match err {
            Some(PriceLevelError::InvalidOperation { message }) => {
                assert!(message.contains("overflow"), "message: {message}");
            }
            other => panic!("expected InvalidOperation, got {other:?}"),
        }
    }

    #[test]
    fn test_sweep_stops_at_failing_maker_with_prefix_and_error() {
        for tif in [TimeInForce::Gtc, TimeInForce::Ioc] {
            let level = level_with_failing_middle_maker();
            let visible_before = level.visible_quantity();
            let hidden_before = level.hidden_quantity();
            let count_before = level.order_count();

            let result = level.match_order(
                10,
                Id::from_u64(TAKER),
                tif,
                TakerKind::Standard,
                TimestampMs::new(1_716_000_000_000),
                &generator(),
            );

            // Committed prefix: maker 1 only.
            assert_invalid_operation(result.error());
            assert!(result.is_failed());
            assert_eq!(result.trades().len(), 1, "{tif:?}");
            let trade = &result.trades().as_vec()[0];
            assert_eq!(trade.maker_order_id(), Id::from_u64(1));
            assert_eq!(trade.quantity(), Quantity::new(5));
            assert_eq!(result.remaining_quantity(), Quantity::new(5));
            assert!(!result.is_complete());
            assert_eq!(
                result.executed_quantity().expect("sum"),
                Quantity::new(5),
                "executed == sum of trades"
            );
            assert_eq!(result.filled_order_ids(), &[Id::from_u64(1)]);

            // Counters moved by exactly the committed fill.
            assert_eq!(level.visible_quantity(), visible_before - 5);
            assert_eq!(level.hidden_quantity(), hidden_before);
            assert_eq!(level.order_count(), count_before - 1);

            // Failing maker and the maker behind it are untouched, in order.
            let rest = queue(&level);
            assert_eq!(rest.len(), 2);
            assert_eq!(*rest[0], overflowing_reserve(2));
            assert_eq!(*rest[1], standard(3, 5));
        }
    }

    #[test]
    fn test_sweep_failing_front_maker_reports_error_without_trades() {
        let level = PriceLevel::new(PRICE);
        level
            .test_rest_unadmitted(overflowing_reserve(2))
            .expect("rest failing maker");
        level.add_order(standard(3, 5)).expect("admit maker 3");
        let visible_before = level.visible_quantity();

        let result = level.match_order(
            4,
            Id::from_u64(TAKER),
            TimeInForce::Gtc,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &generator(),
        );

        assert_invalid_operation(result.error());
        assert!(result.trades().is_empty());
        assert_eq!(result.remaining_quantity(), Quantity::new(4));
        assert_eq!(level.visible_quantity(), visible_before);
        let rest = queue(&level);
        assert_eq!(*rest[0], overflowing_reserve(2));
        assert_eq!(*rest[1], standard(3, 5));
    }

    #[test]
    fn test_fok_with_failing_maker_is_killed_with_error_level_unchanged() {
        let level = level_with_failing_middle_maker();
        let visible_before = level.visible_quantity();
        let hidden_before = level.hidden_quantity();
        let count_before = level.order_count();
        let queue_before: Vec<OrderType<()>> = queue(&level).iter().map(|o| **o).collect();

        // Dry-run parity: the prediction is exactly the real sweep's prefix.
        assert_eq!(level.matchable_quantity(10, Id::from_u64(TAKER)), 5);

        let result = level.match_order(
            10,
            Id::from_u64(TAKER),
            TimeInForce::Fok,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &generator(),
        );

        assert!(result.was_killed());
        assert_eq!(result.outcome(), MatchOutcome::Killed);
        assert_invalid_operation(result.error());
        assert!(result.trades().is_empty());
        assert_eq!(result.remaining_quantity(), Quantity::new(10));

        assert_eq!(level.visible_quantity(), visible_before);
        assert_eq!(level.hidden_quantity(), hidden_before);
        assert_eq!(level.order_count(), count_before);
        let queue_after: Vec<OrderType<()>> = queue(&level).iter().map(|o| **o).collect();
        assert_eq!(queue_after, queue_before);
    }

    #[test]
    fn test_fok_filled_before_reaching_failing_maker_is_unaffected() {
        // The dry run stops at `remaining == 0` before visiting the failing
        // maker, so a fill-or-kill the prefix fully covers still fills.
        let level = level_with_failing_middle_maker();
        let result = level.match_order(
            5,
            Id::from_u64(TAKER),
            TimeInForce::Fok,
            TakerKind::Standard,
            TimestampMs::new(1_716_000_000_000),
            &generator(),
        );
        assert!(result.error().is_none());
        assert!(result.is_complete());
        assert_eq!(result.outcome(), MatchOutcome::Filled);
        assert_eq!(result.trades().len(), 1);
        let rest = queue(&level);
        assert_eq!(*rest[0], overflowing_reserve(2));
    }
}
