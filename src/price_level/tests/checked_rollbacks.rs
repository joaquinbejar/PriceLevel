//! Pre-release hardening: every level quantity-counter decrement is a
//! checked `fetch_update(checked_sub)`, never a wrapping `fetch_sub`. A
//! counter that already holds less than the orders it covers (a broken
//! invariant, forced here through a test seam) is left unchanged, the level
//! is poisoned and a typed error is reported.

#[cfg(test)]
mod tests {
    use crate::UuidGenerator;
    use crate::errors::PriceLevelError;
    use crate::execution::TakerKind;
    use crate::orders::{Hash32, Id, OrderType, OrderUpdate, Side, TimeInForce};
    use crate::price_level::PriceLevel;
    use crate::price_level::level::{SNAPSHOT_ATTEMPT_SLOTS, SNAPSHOT_MAX_ATTEMPTS};
    use crate::utils::{Price, Quantity, TimestampMs};
    use uuid::Uuid;

    const PRICE: u128 = 10_000;

    fn standard(id: u64, quantity: u64) -> OrderType<()> {
        OrderType::Standard {
            id: Id::from_u64(id),
            price: Price::new(PRICE),
            quantity: Quantity::new(quantity),
            side: Side::Sell,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1_616_823_000_000),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        }
    }

    fn iceberg(id: u64, visible: u64, hidden: u64) -> OrderType<()> {
        OrderType::IcebergOrder {
            id: Id::from_u64(id),
            price: Price::new(PRICE),
            visible_quantity: Quantity::new(visible),
            hidden_quantity: Quantity::new(hidden),
            side: Side::Sell,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1_616_823_000_000),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        }
    }

    fn is_counter_failure(err: &PriceLevelError) -> bool {
        matches!(
            err,
            PriceLevelError::InvalidOperation { message }
                if message.contains("counter refused a decrement")
        )
    }

    #[test]
    fn cancel_with_short_visible_counter_poisons_without_wrapping() {
        let level = PriceLevel::new(PRICE);
        level.add_order(standard(1, 10)).expect("admit");
        level.test_store_quantity_counters(3, 0);
        let (_, mutation_before) = level.test_epochs();

        let err = level
            .update_order(OrderUpdate::Cancel {
                order_id: Id::from_u64(1),
            })
            .expect_err("counter refusal is reported");
        assert!(is_counter_failure(&err), "{err:?}");
        assert!(level.test_is_poisoned());
        assert!(
            level.test_epochs().1 > mutation_before,
            "the committed removal still moves the mutation epoch"
        );
        assert_eq!(level.visible_quantity(), 3, "refused, not wrapped");
        assert_eq!(level.order_count(), 0, "the removal itself committed");
        assert_eq!(level.test_topology_count(), 0);
        assert!(
            level.add_order(standard(2, 1)).is_err(),
            "poisoned level refuses work"
        );
    }

    #[test]
    fn cancel_with_short_hidden_counter_poisons_without_wrapping() {
        let level = PriceLevel::new(PRICE);
        level.add_order(iceberg(1, 5, 20)).expect("admit");
        level.test_store_quantity_counters(5, 7);

        let err = level
            .update_order(OrderUpdate::Cancel {
                order_id: Id::from_u64(1),
            })
            .expect_err("counter refusal is reported");
        assert!(is_counter_failure(&err), "{err:?}");
        assert!(level.test_is_poisoned());
        assert_eq!(level.visible_quantity(), 0, "visible moved normally");
        assert_eq!(level.hidden_quantity(), 7, "refused, not wrapped");
    }

    #[test]
    fn sweep_with_short_visible_counter_stops_and_poisons() {
        let level = PriceLevel::new(PRICE);
        level.add_order(standard(1, 10)).expect("admit");
        level.add_order(standard(2, 10)).expect("admit");
        level.test_store_quantity_counters(3, 0);
        let ids = UuidGenerator::new(Uuid::new_v4());

        let result = level.match_order(
            20,
            Id::from_u64(100),
            TimeInForce::Ioc,
            TakerKind::Standard,
            TimestampMs::new(1),
            &ids,
        );
        // The first maker's fill committed; the sweep then stopped.
        assert_eq!(result.trades().len(), 1);
        assert!(result.error().is_some_and(is_counter_failure), "{result:?}");
        assert!(level.test_is_poisoned());
        assert_eq!(level.visible_quantity(), 3, "refused, not wrapped");
        assert_eq!(level.order_count(), 1, "second maker untouched");
    }

    #[test]
    fn sweep_replenish_with_short_hidden_counter_stops_and_poisons() {
        let level = PriceLevel::new(PRICE);
        level.add_order(iceberg(1, 5, 20)).expect("admit");
        level.test_store_quantity_counters(5, 2);
        let ids = UuidGenerator::new(Uuid::new_v4());

        let result = level.match_order(
            5,
            Id::from_u64(100),
            TimeInForce::Ioc,
            TakerKind::Standard,
            TimestampMs::new(1),
            &ids,
        );
        assert_eq!(result.trades().len(), 1);
        assert!(result.error().is_some_and(is_counter_failure), "{result:?}");
        assert!(level.test_is_poisoned());
        assert_eq!(level.hidden_quantity(), 2, "refused, not wrapped");
    }

    #[test]
    fn snapshot_rejection_slots_cover_every_attempt() {
        assert_eq!(
            u32::try_from(SNAPSHOT_ATTEMPT_SLOTS).expect("small"),
            SNAPSHOT_MAX_ATTEMPTS
        );
    }
}
