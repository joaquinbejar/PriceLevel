//! `MatchResult` analytics (#151): the fused single-traversal
//! `average_price` must agree exactly, value and error, with the
//! quantity-then-value sequence it replaces, on API-built and decoded results.

#[cfg(test)]
mod tests {
    use crate::errors::PriceLevelError;
    use crate::execution::match_result::MatchResult;
    use crate::execution::trade::Trade;
    use crate::orders::{Id, Side};
    use crate::utils::{Price, Quantity, TimestampMs};
    use proptest::prelude::*;
    use std::str::FromStr;

    const TAKER: u64 = 10;

    fn trade(maker: u64, price: u128, quantity: u64) -> Trade {
        Trade::with_timestamp(
            Id::from_u64(1_000 + maker),
            Id::from_u64(TAKER),
            Id::from_u64(maker),
            Price::new(price),
            Quantity::new(quantity),
            Side::Buy,
            TimestampMs::new(1_616_823_000_000),
        )
    }

    /// Builds a result through the public API holding `fills` in order, with
    /// `extra` units left over (`extra == 0` yields a complete result).
    fn build(fills: &[(u128, u64)], extra: u64) -> MatchResult {
        let total = fills
            .iter()
            .try_fold(extra, |acc, (_, q)| acc.checked_add(*q))
            .unwrap_or_else(|| panic!("fixture quantities overflow u64"));
        let mut result = MatchResult::new(Id::from_u64(TAKER), Quantity::new(total));
        for (i, (price, quantity)) in fills.iter().enumerate() {
            if let Err(error) = result.add_trade(trade(i as u64 + 20, *price, *quantity)) {
                panic!("add_trade must accept a representable quantity: {error}");
            }
        }
        result
    }

    /// Reference: the pre-#151 `average_price`, an executed-quantity scan
    /// followed by an executed-value scan, written independently of the
    /// crate's helpers.
    fn reference_average(result: &MatchResult) -> Result<Option<f64>, PriceLevelError> {
        let mut qty = 0u64;
        for t in result.trades().as_vec() {
            qty = qty.checked_add(t.quantity().as_u64()).ok_or_else(|| {
                PriceLevelError::InvalidOperation {
                    message: "executed quantity overflow".to_string(),
                }
            })?;
        }
        if qty == 0 {
            return Ok(None);
        }
        let mut value = 0u128;
        for t in result.trades().as_vec() {
            let step = t
                .price()
                .as_u128()
                .checked_mul(u128::from(t.quantity().as_u64()))
                .ok_or_else(|| PriceLevelError::InvalidOperation {
                    message: "executed value multiplication overflow".to_string(),
                })?;
            value = value
                .checked_add(step)
                .ok_or_else(|| PriceLevelError::InvalidOperation {
                    message: "executed value accumulation overflow".to_string(),
                })?;
        }
        Ok(Some(value as f64 / qty as f64))
    }

    /// Exact (bitwise for `f64`) agreement of two average results.
    fn same_average(
        a: &Result<Option<f64>, PriceLevelError>,
        b: &Result<Option<f64>, PriceLevelError>,
    ) -> bool {
        match (a, b) {
            (Ok(Some(x)), Ok(Some(y))) => x.to_bits() == y.to_bits(),
            (Ok(None), Ok(None)) => true,
            (Err(x), Err(y)) => x == y,
            _ => false,
        }
    }

    fn err_message(result: Result<impl std::fmt::Debug, PriceLevelError>) -> String {
        match result {
            Err(PriceLevelError::InvalidOperation { message }) => message,
            other => panic!("expected InvalidOperation, got {other:?}"),
        }
    }

    /// Every decode / construction boundary of a result: JSON, bincode, text
    /// (`Display` -> `FromStr`) and `try_clone`.
    fn decoded_copies(result: &MatchResult) -> Vec<(&'static str, MatchResult)> {
        let json = serde_json::to_string(result).unwrap_or_else(|e| panic!("json encode: {e}"));
        let from_json: MatchResult =
            serde_json::from_str(&json).unwrap_or_else(|e| panic!("json decode: {e}"));
        let bytes = bincode::serde::encode_to_vec(result, bincode::config::standard())
            .unwrap_or_else(|e| panic!("bincode encode: {e}"));
        let (from_bincode, _): (MatchResult, usize) =
            bincode::serde::decode_from_slice(&bytes, bincode::config::standard())
                .unwrap_or_else(|e| panic!("bincode decode: {e}"));
        let from_text = MatchResult::from_str(&result.to_string())
            .unwrap_or_else(|e| panic!("text decode: {e}"));
        let cloned = result
            .try_clone()
            .unwrap_or_else(|e| panic!("try_clone: {e}"));
        vec![
            ("json", from_json),
            ("bincode", from_bincode),
            ("text", from_text),
            ("try_clone", cloned),
        ]
    }

    #[test]
    fn empty_result_has_no_average() {
        let result = build(&[], 5);
        assert_eq!(result.executed_quantity().map(|q| q.as_u64()), Ok(0));
        assert_eq!(result.executed_value(), Ok(0));
        assert_eq!(result.average_price(), Ok(None));
    }

    #[test]
    fn zero_quantity_trades_have_no_average_even_at_max_price() {
        let result = build(&[(u128::MAX, 0), (u128::MAX, 0)], 5);
        assert_eq!(result.executed_quantity().map(|q| q.as_u64()), Ok(0));
        assert_eq!(result.executed_value(), Ok(0));
        assert_eq!(result.average_price(), Ok(None));
    }

    #[test]
    fn exact_value_limit_is_representable() {
        // Single product at the limit, and an accumulation landing exactly on it.
        for fills in [
            vec![(u128::MAX, 1)],
            vec![(u128::MAX - 1, 1), (1, 1)],
            vec![(u128::MAX / 3, 3)],
        ] {
            let result = build(&fills, 0);
            let expected_qty: u64 = fills.iter().map(|(_, q)| q).sum();
            let value = result.executed_value();
            assert!(value.is_ok(), "{fills:?}: {value:?}");
            assert_eq!(
                result.average_price(),
                Ok(Some(value.unwrap_or_default() as f64 / expected_qty as f64))
            );
            assert!(same_average(
                &result.average_price(),
                &reference_average(&result)
            ));
        }
    }

    #[test]
    fn exact_quantity_limit_is_representable() {
        let result = build(&[(1, u64::MAX - 1), (1, 1)], 0);
        assert_eq!(result.executed_quantity().map(|q| q.as_u64()), Ok(u64::MAX));
        assert_eq!(result.executed_value(), Ok(u128::from(u64::MAX)));
        assert_eq!(result.average_price(), Ok(Some(1.0)));
    }

    /// `add_trade` accepts the representable quantity; only the value-based
    /// analytics fail, and the quantity-only query keeps working.
    #[test]
    fn multiplication_overflow_keeps_quantity_usable() {
        let result = build(&[(1_000, 3), (u128::MAX, 2)], 1);
        assert_eq!(result.trades().len(), 2);
        assert_eq!(result.executed_quantity().map(|q| q.as_u64()), Ok(5));
        assert_eq!(
            err_message(result.executed_value()),
            "executed value multiplication overflow"
        );
        assert_eq!(
            err_message(result.average_price()),
            "executed value multiplication overflow"
        );
        for (path, copy) in decoded_copies(&result) {
            assert_eq!(
                copy.executed_quantity(),
                result.executed_quantity(),
                "{path}"
            );
            assert_eq!(copy.executed_value(), result.executed_value(), "{path}");
            assert_eq!(copy.average_price(), result.average_price(), "{path}");
        }
    }

    #[test]
    fn accumulation_overflow_keeps_quantity_usable() {
        let result = build(&[(u128::MAX, 1), (1, 1)], 0);
        assert_eq!(result.executed_quantity().map(|q| q.as_u64()), Ok(2));
        assert_eq!(
            err_message(result.executed_value()),
            "executed value accumulation overflow"
        );
        assert_eq!(
            err_message(result.average_price()),
            "executed value accumulation overflow"
        );
        for (path, copy) in decoded_copies(&result) {
            assert_eq!(
                copy.executed_quantity(),
                result.executed_quantity(),
                "{path}"
            );
            assert_eq!(copy.average_price(), result.average_price(), "{path}");
        }
    }

    /// The first overflow in trade order wins, whichever kind it is.
    #[test]
    fn first_value_overflow_in_trade_order_is_reported() {
        let acc_first = build(&[(u128::MAX, 1), (1, 1), (u128::MAX, 2)], 0);
        assert_eq!(
            err_message(acc_first.average_price()),
            "executed value accumulation overflow"
        );
        let mul_first = build(&[(u128::MAX, 2), (u128::MAX, 1), (1, 1)], 0);
        assert_eq!(
            err_message(mul_first.average_price()),
            "executed value multiplication overflow"
        );
    }

    /// Decoders still refuse a result whose quantities cannot be summed, so
    /// no decoded value can reach the analytics with an unrepresentable
    /// executed quantity.
    #[test]
    fn invalid_decoded_quantity_sum_is_rejected() {
        let valid = build(&[(1_000, 40), (1_000, 20)], 40);
        let mut value = serde_json::to_value(&valid).unwrap_or_else(|e| panic!("{e}"));
        value["trades"]["trades"][0]["quantity"] = serde_json::json!(u64::MAX);
        value["trades"]["trades"][1]["quantity"] = serde_json::json!(u64::MAX);
        assert!(serde_json::from_value::<MatchResult>(value).is_err());
    }

    fn price_strategy() -> impl Strategy<Value = u128> {
        prop_oneof![
            4 => 0u128..=1_000_000,
            2 => (u128::MAX / 4)..=u128::MAX,
            1 => any::<u128>(),
        ]
    }

    fn quantity_strategy() -> impl Strategy<Value = u64> {
        prop_oneof![
            1 => Just(0u64),
            4 => 1u64..=1_000,
            2 => 0u64..=(u64::MAX / 64),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

        /// The fused traversal equals the separate scans (value and error)
        /// on arbitrary trade lists, including every overflow mix, and every
        /// decode boundary reproduces the same analytics.
        #[test]
        fn prop_fused_average_matches_separate_scans(
            fills in proptest::collection::vec((price_strategy(), quantity_strategy()), 0..=64),
            extra in 0u64..=1,
        ) {
            let result = build(&fills, extra);
            let fused = result.average_price();
            let reference = reference_average(&result);
            prop_assert!(same_average(&fused, &reference), "fused {fused:?} vs reference {reference:?}");

            // Quantity-only success never depends on value representability.
            let expected_qty: u64 = fills.iter().map(|(_, q)| *q).sum();
            prop_assert_eq!(result.executed_quantity().map(|q| q.as_u64()), Ok(expected_qty));

            for (path, copy) in decoded_copies(&result) {
                prop_assert!(same_average(&copy.average_price(), &fused), "{}", path);
                prop_assert_eq!(copy.executed_quantity(), result.executed_quantity(), "{}", path);
                prop_assert_eq!(copy.executed_value(), result.executed_value(), "{}", path);
            }
        }
    }
}
