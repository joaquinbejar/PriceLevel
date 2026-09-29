//! Issue #170 / #164 contract: fallible result allocation and growth, and the
//! `MatchResult` error slot (serde / bincode / legacy decoding).

#[cfg(test)]
// Scoped to this co-located test module only (issue #173): raw arithmetic is
// permitted inside `mod tests` per the Testing section of
// `rules/global_rules.md`. Production code outside this module keeps the
// full deny list.
#[allow(clippy::arithmetic_side_effects)]
mod tests {
    use crate::errors::{CapacityResource, PriceLevelError};
    use crate::execution::list::TradeList;
    use crate::execution::match_result::{MatchOutcome, MatchResult};
    use crate::execution::trade::Trade;
    use crate::execution::{match_result_seam, trade_list_seam};
    use crate::orders::{Id, Side};
    use crate::utils::{Price, Quantity, TimestampMs};

    fn trade(maker: u64, quantity: u64) -> Trade {
        Trade::with_timestamp(
            Id::from_u64(1_000 + maker),
            Id::from_u64(10),
            Id::from_u64(maker),
            Price::new(1_000),
            Quantity::new(quantity),
            Side::Buy,
            TimestampMs::new(1_616_823_000_000),
        )
    }

    /// First element count whose byte size exceeds `isize::MAX`.
    fn byte_overflow_len<T>() -> usize {
        (isize::MAX as usize) / std::mem::size_of::<T>() + 1
    }

    fn assert_capacity_error(err: &PriceLevelError, resource: CapacityResource, additional: usize) {
        match err {
            PriceLevelError::CapacityExceeded {
                resource: got,
                additional: got_additional,
            } => {
                assert_eq!(*got, resource);
                assert_eq!(*got_additional, additional);
            }
            other => panic!("expected CapacityExceeded, got {other:?}"),
        }
    }

    #[test]
    fn trade_list_try_with_capacity_zero_normal_and_overflow() {
        let empty = TradeList::try_with_capacity(0).expect("zero capacity");
        assert_eq!(empty.capacity(), 0, "zero capacity must not allocate");
        assert!(empty.is_empty());

        let normal = TradeList::try_with_capacity(16).expect("normal capacity");
        assert!(normal.capacity() >= 16);
        assert!(normal.is_empty());

        for n in [usize::MAX, byte_overflow_len::<Trade>()] {
            match TradeList::try_with_capacity(n) {
                Err(err) => assert_capacity_error(&err, CapacityResource::Trades, n),
                Ok(_) => panic!("capacity {n} must be rejected"),
            }
        }
    }

    #[test]
    fn match_result_try_with_capacity_zero_normal_and_overflow() {
        let zero = MatchResult::try_with_capacity(Id::from_u64(10), Quantity::new(5), 0)
            .expect("zero capacity");
        assert_eq!(zero.trades().capacity(), 0);
        assert_eq!(zero.remaining_quantity().as_u64(), 5);
        assert_eq!(zero.outcome(), MatchOutcome::NotFilled);
        assert!(zero.error().is_none());

        let normal = MatchResult::try_with_capacity(Id::from_u64(10), Quantity::new(5), 8)
            .expect("normal capacity");
        assert!(normal.trades().capacity() >= 8);

        for n in [usize::MAX, byte_overflow_len::<Trade>()] {
            match MatchResult::try_with_capacity(Id::from_u64(10), Quantity::new(1), n) {
                Err(err) => assert_capacity_error(&err, CapacityResource::Trades, n),
                Ok(_) => panic!("capacity {n} must be rejected"),
            }
        }
    }

    #[test]
    fn try_reserve_overflow_leaves_list_and_result_unchanged() {
        let mut list = TradeList::new();
        list.add(trade(1, 5)).expect("add");
        let err = list.try_reserve(usize::MAX).expect_err("must overflow");
        assert_capacity_error(&err, CapacityResource::Trades, usize::MAX);
        assert_eq!(list.len(), 1);

        let mut result = MatchResult::new(Id::from_u64(10), Quantity::new(20));
        result.add_trade(trade(1, 5)).expect("add_trade");
        let err = result.try_reserve(usize::MAX).expect_err("must overflow");
        assert_capacity_error(&err, CapacityResource::Trades, usize::MAX);
        assert_eq!(result.trades().len(), 1);
        assert_eq!(result.remaining_quantity().as_u64(), 15);
        // A reservation failure returned to the caller is NOT recorded as a
        // match failure: only the engine sets the slot.
        assert!(result.error().is_none());
    }

    /// Issue #219: each split reservation sizes only its own vector.
    #[test]
    fn split_reservations_size_one_vector_each() {
        let mut result = MatchResult::new(Id::from_u64(10), Quantity::new(20));
        result.try_reserve_trades(4).expect("reserve trades");
        assert!(result.trades().capacity() >= 4);
        assert_eq!(result.test_filled_order_ids_capacity(), 0);

        let mut result = MatchResult::new(Id::from_u64(10), Quantity::new(20));
        result
            .try_reserve_filled_order_ids(3)
            .expect("reserve filled ids");
        assert!(result.test_filled_order_ids_capacity() >= 3);
        assert_eq!(result.trades().capacity(), 0);

        // Zero never allocates.
        let mut result = MatchResult::new(Id::from_u64(10), Quantity::new(20));
        result.try_reserve_trades(0).expect("zero trades");
        result.try_reserve_filled_order_ids(0).expect("zero ids");
        assert_eq!(result.trades().capacity(), 0);
        assert_eq!(result.test_filled_order_ids_capacity(), 0);

        // The combined convenience still sizes both from one count.
        let mut result = MatchResult::new(Id::from_u64(10), Quantity::new(20));
        result.try_reserve(2).expect("reserve both");
        assert!(result.trades().capacity() >= 2);
        assert!(result.test_filled_order_ids_capacity() >= 2);
    }

    /// Issue #219: a refused split reservation reports its own resource and
    /// changes no observable field; spare capacity makes it allocation-free.
    #[test]
    fn split_reservation_failures_change_nothing() {
        let mut result = MatchResult::new(Id::from_u64(10), Quantity::new(20));
        result.add_trade(trade(1, 5)).expect("add_trade");
        result
            .add_filled_order_id(Id::from_u64(1))
            .expect("filled id");

        let err = result
            .try_reserve_trades(usize::MAX)
            .expect_err("must overflow");
        assert_capacity_error(&err, CapacityResource::Trades, usize::MAX);
        let err = result
            .try_reserve_filled_order_ids(usize::MAX)
            .expect_err("must overflow");
        assert_capacity_error(&err, CapacityResource::FilledOrderIds, usize::MAX);
        let n = byte_overflow_len::<Id>();
        let err = result
            .try_reserve_filled_order_ids(n)
            .expect_err("byte size overflows");
        assert_capacity_error(&err, CapacityResource::FilledOrderIds, n);

        let _limit = trade_list_seam::limit_trades(1);
        let err = result.try_reserve_trades(1).expect_err("limited");
        assert_capacity_error(&err, CapacityResource::Trades, 1);

        assert_eq!(result.trades().len(), 1);
        assert_eq!(result.filled_order_ids(), &[Id::from_u64(1)]);
        assert_eq!(result.remaining_quantity().as_u64(), 15);
        assert_eq!(result.outcome(), MatchOutcome::PartiallyFilled);
        assert!(result.error().is_none());

        // Spare capacity: the reservation reuses the buffer.
        let mut result = MatchResult::new(Id::from_u64(10), Quantity::new(20));
        result.try_reserve_filled_order_ids(4).expect("reserve");
        let before = result.filled_order_ids().as_ptr();
        result.try_reserve_filled_order_ids(4).expect("fits");
        assert_eq!(result.filled_order_ids().as_ptr(), before);
    }

    /// A failed growth in `add_trade` must leave every observable field as it
    /// was: the reservation happens before remaining / completion / outcome
    /// are committed.
    #[test]
    fn add_trade_growth_failure_changes_nothing() {
        let mut result = MatchResult::new(Id::from_u64(10), Quantity::new(10));
        result.add_trade(trade(1, 4)).expect("first trade");
        result
            .add_filled_order_id(Id::from_u64(1))
            .expect("filled id");

        let _limit = trade_list_seam::limit_trades(1);
        // Would complete the taker if it were committed.
        let err = result.add_trade(trade(2, 6)).expect_err("limited");
        assert_capacity_error(&err, CapacityResource::Trades, 1);

        assert_eq!(result.trades().len(), 1);
        assert_eq!(result.remaining_quantity().as_u64(), 6);
        assert!(!result.is_complete());
        assert_eq!(result.outcome(), MatchOutcome::PartiallyFilled);
        assert_eq!(result.filled_order_ids(), &[Id::from_u64(1)]);
        assert!(result.error().is_none());

        let mut list = TradeList::new();
        list.add(trade(1, 1)).expect("within limit");
        let err = list.add(trade(2, 1)).expect_err("over limit");
        assert_capacity_error(&err, CapacityResource::Trades, 1);
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn add_trade_injected_failure_changes_nothing() {
        let mut result = MatchResult::new(Id::from_u64(10), Quantity::new(10));
        let _fail = match_result_seam::fail_add_trade_after(1);
        result.add_trade(trade(1, 4)).expect("first trade");
        assert!(result.add_trade(trade(2, 6)).is_err());
        assert_eq!(result.trades().len(), 1);
        assert_eq!(result.remaining_quantity().as_u64(), 6);
        assert_eq!(result.outcome(), MatchOutcome::PartiallyFilled);
    }

    #[test]
    fn set_error_keeps_the_first_failure() {
        let mut result = MatchResult::new(Id::from_u64(10), Quantity::new(10));
        assert!(!result.is_failed());
        result.set_error(PriceLevelError::capacity_exceeded(
            CapacityResource::Trades,
            1,
        ));
        result.set_error(PriceLevelError::InvalidFormat);
        assert!(result.is_failed());
        assert_capacity_error(
            result.error().expect("error set"),
            CapacityResource::Trades,
            1,
        );
    }

    #[test]
    fn capacity_error_display_is_fixed_and_descriptive() {
        let err = PriceLevelError::capacity_exceeded(CapacityResource::FilledOrderIds, 3);
        assert_eq!(
            err.to_string(),
            "Capacity exceeded: could not reserve 3 more filled order ids entries"
        );
        assert_eq!(format!("{err:?}"), err.to_string());
        assert_eq!(CapacityResource::Trades.to_string(), "trades");
    }

    /// Issue #168: generator exhaustion reuses the fixed-size capacity error.
    #[test]
    fn id_sequence_capacity_error_displays_and_round_trips() {
        let err = PriceLevelError::capacity_exceeded(CapacityResource::IdSequence, 1);
        assert_eq!(
            err.to_string(),
            "Capacity exceeded: could not reserve 1 more id sequence entries"
        );
        let json = serde_json::to_string(&CapacityResource::IdSequence).expect("json");
        assert_eq!(json, "\"id_sequence\"");
        let back: PriceLevelError =
            serde_json::from_str(&serde_json::to_string(&err).expect("json")).expect("decode");
        assert_eq!(back, err);
        assert_eq!(err.try_clone().expect("try_clone"), err);
    }

    #[test]
    fn try_clone_matches_clone() {
        let mut result = MatchResult::new(Id::from_u64(10), Quantity::new(10));
        result.add_trade(trade(1, 4)).expect("trade");
        result.add_filled_order_id(Id::from_u64(1)).expect("id");
        result.set_error(PriceLevelError::InvalidOperation {
            message: "boom".to_string(),
        });
        let copy = result.try_clone().expect("try_clone");
        assert_eq!(copy.trades(), result.trades());
        assert_eq!(copy.filled_order_ids(), result.filled_order_ids());
        assert_eq!(copy.remaining_quantity(), result.remaining_quantity());
        assert_eq!(copy.outcome(), result.outcome());
        assert_eq!(copy.error(), result.error());

        let list = result.trades().try_clone().expect("list try_clone");
        assert_eq!(&list, result.trades());
    }

    fn failed_result(error: PriceLevelError) -> MatchResult {
        let mut result = MatchResult::new(Id::from_u64(10), Quantity::new(100));
        result.add_trade(trade(1, 25)).expect("trade");
        result.add_filled_order_id(Id::from_u64(1)).expect("id");
        result.set_error(error);
        result
    }

    fn assert_same(a: &MatchResult, b: &MatchResult) {
        assert_eq!(a.order_id(), b.order_id());
        assert_eq!(a.trades(), b.trades());
        assert_eq!(a.filled_order_ids(), b.filled_order_ids());
        assert_eq!(a.remaining_quantity(), b.remaining_quantity());
        assert_eq!(a.is_complete(), b.is_complete());
        assert_eq!(a.outcome(), b.outcome());
        assert_eq!(a.error(), b.error());
    }

    fn samples() -> Vec<MatchResult> {
        vec![
            MatchResult::new(Id::from_u64(10), Quantity::new(100)),
            failed_result(PriceLevelError::capacity_exceeded(
                CapacityResource::Trades,
                1,
            )),
            failed_result(PriceLevelError::InvalidOperation {
                message: "stopped".to_string(),
            }),
            {
                let mut killed = MatchResult::new(Id::from_u64(10), Quantity::new(100));
                killed.mark_killed(100);
                killed.set_error(PriceLevelError::capacity_exceeded(
                    CapacityResource::FilledOrderIds,
                    4,
                ));
                killed
            },
        ]
    }

    #[test]
    fn json_round_trip_with_and_without_error() {
        for original in samples() {
            let json = serde_json::to_string(&original).expect("encode");
            let decoded: MatchResult = serde_json::from_str(&json).expect("decode");
            assert_same(&original, &decoded);
        }
    }

    #[test]
    fn bincode_round_trip_with_and_without_error() {
        for original in samples() {
            let bytes = bincode::serde::encode_to_vec(&original, bincode::config::standard())
                .expect("encode");
            let (decoded, read): (MatchResult, usize) =
                bincode::serde::decode_from_slice(&bytes, bincode::config::standard())
                    .expect("decode");
            assert_eq!(read, bytes.len(), "whole payload consumed");
            assert_same(&original, &decoded);
        }
    }

    /// A JSON payload written before the error slot existed (no `error` key)
    /// decodes as "no error".
    #[test]
    fn legacy_json_without_error_decodes_as_no_error() {
        let mut partial = MatchResult::new(Id::from_u64(10), Quantity::new(100));
        partial.add_trade(trade(1, 25)).expect("trade");
        let current = serde_json::to_string(&partial).expect("encode");
        assert!(current.contains(",\"error\":null"), "{current}");

        // Strip the key to reproduce a pre-#164 payload.
        let legacy = current.replace(",\"error\":null", "");
        assert!(!legacy.contains("error"), "{legacy}");
        let decoded: MatchResult = serde_json::from_str(&legacy).expect("legacy decode");
        assert!(decoded.error().is_none());
        assert_same(&partial, &decoded);
    }

    /// The text format does not carry the error slot (lossy, like `outcome`):
    /// it decodes as "no error" while every other field survives.
    #[test]
    fn text_round_trip_drops_error_only() {
        let original = failed_result(PriceLevelError::InvalidFormat);
        let decoded: MatchResult = original.to_string().parse().expect("text decode");
        assert!(decoded.error().is_none());
        assert_eq!(decoded.trades(), original.trades());
        assert_eq!(decoded.remaining_quantity(), original.remaining_quantity());
    }
}
