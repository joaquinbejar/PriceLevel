//! Issue #214: `checked_counter_sub` skips the read-modify-write for a zero
//! delta. The skip is keyed on the operation's own delta, never on the
//! counter's value, so it is indistinguishable from the checked RMW it
//! replaces: a zero decrement always succeeds and never moves the counter,
//! and a nonzero decrement is still refused (counter unchanged, never
//! wrapped) when the counter holds less.

#[cfg(test)]
mod tests {
    use crate::orders::{Hash32, Id, OrderType, OrderUpdate, Side, TimeInForce};
    use crate::price_level::PriceLevel;
    use crate::price_level::level::checked_counter_sub;
    use crate::utils::{Price, Quantity, TimestampMs};
    use std::sync::atomic::{AtomicU64, Ordering};

    const PRICE: u128 = 10_000;

    #[test]
    fn zero_delta_succeeds_and_leaves_every_counter_value_unchanged() {
        for value in [0, 1, 7, u64::MAX] {
            let counter = AtomicU64::new(value);
            assert!(checked_counter_sub(&counter, 0));
            assert_eq!(counter.load(Ordering::Relaxed), value);
        }
    }

    #[test]
    fn nonzero_delta_is_still_checked() {
        let counter = AtomicU64::new(4);
        assert!(!checked_counter_sub(&counter, 5), "refused");
        assert_eq!(counter.load(Ordering::Relaxed), 4, "unchanged, not wrapped");
        assert!(checked_counter_sub(&counter, 4));
        assert_eq!(counter.load(Ordering::Relaxed), 0);
        assert!(!checked_counter_sub(&counter, 1), "refused at zero");
        assert_eq!(counter.load(Ordering::Relaxed), 0, "unchanged, not wrapped");
    }

    /// A standard order's zero hidden component takes the skip on cancel;
    /// the visible decrement and the level accounting are unaffected.
    #[test]
    fn standard_cancel_with_zero_hidden_component_keeps_counters_exact() {
        let level = PriceLevel::new(PRICE);
        for id in 1..=3u64 {
            level
                .add_order(OrderType::Standard {
                    id: Id::from_u64(id),
                    price: Price::new(PRICE),
                    quantity: Quantity::new(10),
                    side: Side::Buy,
                    user_id: Hash32::zero(),
                    timestamp: TimestampMs::new(1_616_823_000_000),
                    time_in_force: TimeInForce::Gtc,
                    extra_fields: (),
                })
                .expect("admit");
        }
        let removed = level
            .update_order(OrderUpdate::Cancel {
                order_id: Id::from_u64(2),
            })
            .expect("cancel");
        assert!(removed.is_some());
        assert_eq!(level.visible_quantity(), 20);
        assert_eq!(level.hidden_quantity(), 0);
        assert_eq!(level.order_count(), 2);
    }
}
