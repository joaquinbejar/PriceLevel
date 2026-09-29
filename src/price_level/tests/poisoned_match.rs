//! Issue #217: `PriceLevel::match_order` on a poisoned level reports the
//! refusal through the `MatchResult` error slot. Both poisoned exits (the fast
//! path and the fill-or-kill check right after acquiring the exclusive guard)
//! return no trades, the full remaining quantity and the same
//! `InvalidOperation` the level's mutators return. A positive-quantity
//! fill-or-kill taker is killed, any other positive-quantity taker is not
//! filled, and a zero-quantity taker keeps its vacuously complete `Filled`
//! result. Every test also checks that the refusal left the level untouched.

#[cfg(test)]
mod tests {
    use crate::UuidGenerator;
    use crate::errors::PriceLevelError;
    use crate::execution::{MatchOutcome, MatchResult, TakerKind};
    use crate::orders::{Hash32, Id, OrderType, OrderUpdate, Side, TimeInForce};
    use crate::price_level::level::{PriceLevel, set_fok_locked_hook, set_pre_fok_lock_hook};
    use crate::utils::{Price, Quantity, TimestampMs};
    use std::sync::Arc;
    use uuid::Uuid;

    const PRICE: u128 = 10_000;
    const TAKER: u64 = 999;

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

    fn generator() -> UuidGenerator {
        UuidGenerator::new(
            Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").expect("namespace"),
        )
    }

    /// A level with crossable visible and hidden depth, so a healthy level
    /// would trade against every taker used below.
    fn seeded_level() -> PriceLevel {
        let level = PriceLevel::new(PRICE);
        level.add_order(standard(1, 10)).expect("add standard");
        level.add_order(iceberg(2, 5, 20)).expect("add iceberg");
        level.add_order(standard(3, 7)).expect("add standard");
        level
    }

    /// Poison the fill-or-kill guard, then trip the sticky flag through a
    /// refused admission, returning the error that admission reported.
    fn poison(level: &PriceLevel) -> PriceLevelError {
        level.test_poison_guard();
        let err = level
            .add_order(standard(50, 1))
            .expect_err("admission on a poisoned guard must fail");
        assert!(level.is_poisoned(), "the refused admission trips the flag");
        err
    }

    /// Everything the refusal must leave untouched.
    #[derive(Debug, PartialEq)]
    struct LevelState {
        orders: Vec<OrderType<()>>,
        visible: u64,
        hidden: u64,
        order_count: usize,
        epochs: (u64, u64),
    }

    fn state(level: &PriceLevel) -> LevelState {
        LevelState {
            orders: level
                .snapshot_by_insertion_seq()
                .expect("materialize")
                .iter()
                .map(|order| *order.as_ref())
                .collect(),
            visible: level.visible_quantity(),
            hidden: level.hidden_quantity(),
            order_count: level.order_count(),
            epochs: level.test_epochs(),
        }
    }

    fn run(level: &PriceLevel, quantity: u64, tif: TimeInForce, kind: TakerKind) -> MatchResult {
        level.match_order(
            quantity,
            Id::from_u64(TAKER),
            tif,
            kind,
            TimestampMs::new(1_700_000_000_000),
            &generator(),
        )
    }

    /// The fields every refusal shares, plus a serde JSON round-trip through
    /// the decode-time validator.
    fn assert_refused(result: &MatchResult, quantity: u64, expected: &PriceLevelError) {
        assert!(result.is_failed(), "a refusal must carry an error");
        assert_eq!(result.error(), Some(expected));
        assert!(result.trades().is_empty(), "a refusal emits no trades");
        assert!(result.filled_order_ids().is_empty());
        assert_eq!(result.remaining_quantity().as_u64(), quantity);
        assert_eq!(result.is_complete(), quantity == 0);

        let json = serde_json::to_string(result).expect("serialize");
        let decoded: MatchResult = serde_json::from_str(&json).expect("validated decode");
        assert_eq!(decoded.outcome(), result.outcome());
        assert_eq!(decoded.error(), result.error());
        assert_eq!(decoded.remaining_quantity(), result.remaining_quantity());
        assert_eq!(decoded.is_complete(), result.is_complete());
    }

    #[test]
    fn test_is_poisoned_public_accessor() {
        let level = seeded_level();
        assert!(!level.is_poisoned());
        // A panicked holder is reported at once through the lock poison,
        // before any later acquisition trips the sticky flag.
        level.test_poison_guard();
        assert!(level.is_poisoned());
        assert!(!level.test_is_poisoned(), "no acquisition yet: flag clear");
        let _ = poison(&level);
        assert!(level.is_poisoned());
        assert!(level.test_is_poisoned());
        // Sticky: a refused match does not clear it.
        let _ = run(&level, 5, TimeInForce::Gtc, TakerKind::Standard);
        assert!(level.is_poisoned());
    }

    #[test]
    fn test_poisoned_non_fok_positive_taker_not_filled_with_error() {
        let cases = [
            (TimeInForce::Gtc, TakerKind::Standard),
            (TimeInForce::Ioc, TakerKind::Standard),
            (TimeInForce::Day, TakerKind::Standard),
            (TimeInForce::Gtc, TakerKind::PostOnly),
            (TimeInForce::Ioc, TakerKind::MarketToLimit),
        ];
        for (tif, kind) in cases {
            let level = seeded_level();
            let expected = poison(&level);
            assert!(matches!(expected, PriceLevelError::InvalidOperation { .. }));
            let before = state(&level);

            let result = run(&level, 5, tif, kind);

            assert_refused(&result, 5, &expected);
            assert_eq!(
                result.outcome(),
                MatchOutcome::NotFilled,
                "{tif:?} {kind:?}"
            );
            assert!(!result.was_killed());
            assert_eq!(state(&level), before, "{tif:?} {kind:?}");
        }
    }

    #[test]
    fn test_poisoned_fok_positive_taker_killed_on_fast_path() {
        let level = seeded_level();
        let expected = poison(&level);
        let before = state(&level);

        // Fully fillable on a healthy level (42 resting), so only the poison
        // explains the kill.
        let result = run(&level, 20, TimeInForce::Fok, TakerKind::Standard);

        assert_refused(&result, 20, &expected);
        assert!(result.was_killed());
        assert_eq!(result.outcome(), MatchOutcome::Killed);
        assert_eq!(state(&level), before);
    }

    #[test]
    fn test_unrecovered_lock_poison_refused_on_fast_path() {
        // The guard's lock is poisoned but no acquisition has recovered it,
        // so the sticky flag is still clear. The fast path reads the lock
        // poison, trips the flag and refuses, for fill-or-kill and other
        // takers alike (a non-fill-or-kill match takes no guard, so nothing
        // else would catch it).
        for (tif, outcome) in [
            (TimeInForce::Fok, MatchOutcome::Killed),
            (TimeInForce::Gtc, MatchOutcome::NotFilled),
            (TimeInForce::Ioc, MatchOutcome::NotFilled),
        ] {
            let level = seeded_level();
            level.test_poison_guard();
            assert!(!level.test_is_poisoned());
            let before = state(&level);

            let result = run(&level, 20, tif, TakerKind::Standard);

            assert!(
                level.test_is_poisoned(),
                "{tif:?}: the fast path trips the flag"
            );
            let expected = level
                .update_order(OrderUpdate::Cancel {
                    order_id: Id::from_u64(1),
                })
                .expect_err("mutators fail fast on a poisoned level");
            assert_refused(&result, 20, &expected);
            assert_eq!(result.outcome(), outcome, "{tif:?}");
            assert_eq!(state(&level), before, "{tif:?}");
        }
    }

    #[test]
    fn test_non_fok_match_refused_after_fok_sweep_unwinds() {
        // A genuine fill-or-kill unwind: a hook panics while the taker holds
        // the exclusive guard, poisoning its lock. The next match is not
        // fill-or-kill, so it takes no guard; it must still refuse rather
        // than sweep the level.
        let level = seeded_level();
        {
            let _hook = set_fok_locked_hook(Box::new(|| panic!("intentional FOK unwind for test")));
            let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run(&level, 20, TimeInForce::Fok, TakerKind::Standard)
            }));
            assert!(unwound.is_err(), "the fill-or-kill match must unwind");
        }
        assert!(level.is_poisoned(), "the lock poison is visible at once");
        assert!(
            !level.test_is_poisoned(),
            "no acquisition has tripped the flag yet"
        );
        let before = state(&level);

        let result = run(&level, 5, TimeInForce::Gtc, TakerKind::Standard);

        assert!(level.test_is_poisoned());
        let expected = level
            .add_order(standard(50, 1))
            .expect_err("mutators fail fast on a poisoned level");
        assert_refused(&result, 5, &expected);
        assert_eq!(result.outcome(), MatchOutcome::NotFilled);
        assert_eq!(state(&level), before);
    }

    #[test]
    fn test_poisoned_fok_positive_taker_killed_when_poisoned_before_lock() {
        // The guard is healthy at the fast-path check and is poisoned in the
        // window between that check and the exclusive-guard acquisition, so
        // the post-guard exit (the acquisition recovers the poison) refuses.
        let level = Arc::new(seeded_level());
        let before = state(&level);
        let hook_level = Arc::clone(&level);
        let _hook = set_pre_fok_lock_hook(Box::new(move || hook_level.test_poison_guard()));

        let result = run(&level, 20, TimeInForce::Fok, TakerKind::Standard);

        assert!(
            level.test_is_poisoned(),
            "the guard acquisition trips the flag"
        );
        let expected = level
            .add_order(standard(50, 1))
            .expect_err("mutators fail fast on a poisoned level");
        assert_refused(&result, 20, &expected);
        assert!(result.was_killed());
        assert_eq!(result.outcome(), MatchOutcome::Killed);
        assert_eq!(state(&level), before);
    }

    #[test]
    fn test_poisoned_zero_quantity_taker_vacuously_filled_with_error() {
        for tif in [TimeInForce::Fok, TimeInForce::Gtc, TimeInForce::Ioc] {
            let level = seeded_level();
            let expected = poison(&level);
            let before = state(&level);

            let result = run(&level, 0, tif, TakerKind::Standard);

            assert_refused(&result, 0, &expected);
            assert!(result.is_complete(), "{tif:?}");
            assert!(!result.was_killed(), "{tif:?}");
            assert_eq!(result.outcome(), MatchOutcome::Filled, "{tif:?}");
            assert_eq!(state(&level), before, "{tif:?}");
        }
    }
}
