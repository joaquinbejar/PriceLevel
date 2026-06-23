#[cfg(test)]
mod tests {
    use crate::execution::match_result::MatchResult;
    use crate::execution::trade::Trade;
    use crate::orders::{Id, Side};
    use crate::utils::{Price, Quantity, TimestampMs};
    use std::str::FromStr;
    use uuid::Uuid;

    fn parse_uuid(input: &str) -> Uuid {
        match Uuid::parse_str(input) {
            Ok(value) => value,
            Err(error) => panic!("failed to parse uuid: {error}"),
        }
    }

    fn sample_trade(quantity: u64) -> Trade {
        Trade::with_timestamp(
            Id::from_uuid(parse_uuid("6ba7b810-9dad-11d1-80b4-00c04fd430c8")),
            Id::from_u64(10),
            Id::from_u64(20),
            Price::new(1_000),
            Quantity::new(quantity),
            Side::Buy,
            TimestampMs::new(1_616_823_000_000),
        )
    }

    #[test]
    fn add_trade_updates_remaining_and_trades() {
        let mut result = MatchResult::new(Id::from_u64(10), 100);
        assert!(result.add_trade(sample_trade(25)).is_ok());

        assert_eq!(result.remaining_quantity(), 75);
        assert_eq!(result.trades().len(), 1);
        assert!(!result.is_complete());
    }

    #[test]
    fn display_and_parse_use_trades_field() {
        let mut result = MatchResult::new(Id::from_u64(10), 100);
        assert!(result.add_trade(sample_trade(40)).is_ok());

        let rendered = result.to_string();
        assert!(rendered.contains(";trades=Trades:[Trade:"));

        let parsed = match MatchResult::from_str(&rendered) {
            Ok(value) => value,
            Err(error) => panic!("failed to parse match result: {error:?}"),
        };

        assert_eq!(parsed.trades().len(), 1);
        assert_eq!(parsed.remaining_quantity(), 60);
    }

    #[test]
    fn from_str_rejects_old_transactions_field() {
        let old_payload = "MatchResult:order_id=1;remaining_quantity=1;is_complete=false;transactions=Transactions:[];filled_order_ids=[]";
        let parsed = MatchResult::from_str(old_payload);
        assert!(parsed.is_err());
    }

    #[test]
    fn add_trade_rejects_underflow() {
        let mut result = MatchResult::new(Id::from_u64(10), 10);
        let error = result.add_trade(sample_trade(11));
        assert!(error.is_err());
        assert_eq!(result.remaining_quantity(), 10);
        assert_eq!(result.trades().len(), 0);
    }

    #[test]
    fn executed_value_rejects_overflow() {
        let mut result = MatchResult::new(Id::from_u64(10), 4);

        let trade = Trade::with_timestamp(
            Id::from_uuid(parse_uuid("6ba7b810-9dad-11d1-80b4-00c04fd430c8")),
            Id::from_u64(10),
            Id::from_u64(20),
            Price::new(u128::MAX),
            Quantity::new(2),
            Side::Buy,
            TimestampMs::new(1_616_823_000_000),
        );

        assert!(result.add_trade(trade).is_ok());
        assert!(result.executed_value().is_err());
    }

    // ----- average_price edge cases -----
    //
    // `average_price()` returns `Result<Option<f64>, PriceLevelError>` and
    // computes `executed_value as f64 / executed_quantity as f64`, guarding
    // the zero-quantity case so it never divides by zero. These tests pin the
    // zero-quantity, precision-loss, and never-NaN/Inf behavior. The
    // happy-path average (`test_average_price`) is verified via real
    // `match_order` output in the price-level tests.

    /// No trades have been added, so executed quantity is zero and there is no
    /// average price to report: `average_price()` must return `Ok(None)`
    /// (never an error, never a division by zero, never NaN).
    #[test]
    fn test_average_price_zero_executed_quantity_returns_none() {
        let result = MatchResult::new(Id::from_u64(10), 100);

        match result.average_price() {
            Ok(None) => {}
            other => panic!("expected Ok(None) for zero executed quantity, got {other:?}"),
        }
    }

    /// A single trade whose `price` and `quantity` are exactly representable in
    /// `f64` yields an exact average. Pins that the common case is precise and
    /// never NaN/Inf.
    #[test]
    fn test_average_price_exact_small_values_is_precise() {
        let mut result = MatchResult::new(Id::from_u64(10), 100);

        // price 1000, quantity 30 -> value 30_000, avg 1000.0 exactly.
        assert!(result.add_trade(sample_trade(30)).is_ok());

        match result.average_price() {
            Ok(Some(avg)) => {
                assert!(avg.is_finite(), "average price must be finite");
                assert!((avg - 1000.0_f64).abs() < f64::EPSILON);
            }
            other => panic!("expected Ok(Some(1000.0)), got {other:?}"),
        }
    }

    /// Large value/quantity case where `f64`'s 53-bit integer mantissa cannot
    /// represent the exact integer average.
    ///
    /// A single trade with `price = 2^53 + 1` and `quantity = 1` has an exact
    /// integer average of `9_007_199_254_740_993`, but `average_price()`
    /// returns `9_007_199_254_740_992.0` because `2^53 + 1` is not
    /// representable as an `f64` (it rounds to `2^53`). This is a documented,
    /// intrinsic limitation of using `f64` for the analytics average — the
    /// matching path itself keeps exact integer `Price`/`Quantity` math and
    /// only `average_price` drops to `f64`.
    ///
    /// We therefore do NOT assert exact equality with the integer average.
    /// Instead we assert the result is finite and within a small absolute
    /// tolerance (well under 1 ULP-worth of drift at this magnitude, here a
    /// fixed difference of exactly 1).
    #[test]
    fn test_average_price_large_values_loses_f64_precision_but_stays_finite() {
        const PRICE: u128 = (1_u128 << 53) + 1; // 9_007_199_254_740_993
        const EXACT_INT_AVG: u128 = PRICE; // quantity == 1, so average == price

        let mut result = MatchResult::new(Id::from_u64(10), 1);
        let trade = Trade::with_timestamp(
            Id::from_uuid(parse_uuid("6ba7b810-9dad-11d1-80b4-00c04fd430c8")),
            Id::from_u64(10),
            Id::from_u64(20),
            Price::new(PRICE),
            Quantity::new(1),
            Side::Buy,
            TimestampMs::new(1_616_823_000_000),
        );
        assert!(result.add_trade(trade).is_ok());

        match result.average_price() {
            Ok(Some(avg)) => {
                // Never NaN/Inf for a valid, in-range input.
                assert!(avg.is_finite(), "average price must be finite");
                assert!(!avg.is_nan(), "average price must not be NaN");
                assert!(!avg.is_infinite(), "average price must not be Inf");

                // Precision genuinely drifts: the f64 average differs from the
                // exact integer average by exactly 1 here. Assert it is close,
                // not equal.
                let exact = EXACT_INT_AVG as f64;
                let abs_err = (avg - exact).abs();
                assert!(
                    abs_err <= 2.0,
                    "f64 average should be within tolerance of the exact \
                     integer average; abs_err = {abs_err}"
                );

                // Document the observed drift: the exact integer average is
                // 2^53 + 1, but f64 cannot represent it and returns 2^53.
                assert_eq!(
                    avg, 9_007_199_254_740_992.0_f64,
                    "observed f64 average rounds 2^53 + 1 down to 2^53"
                );
            }
            other => panic!("expected Ok(Some(_)), got {other:?}"),
        }
    }

    /// For valid inputs producing trades, `average_price()` must never yield a
    /// NaN or infinite value. Sweeps a handful of (price, quantity) inputs and
    /// asserts finiteness on each.
    #[test]
    fn test_average_price_valid_inputs_never_nan_or_inf() {
        let cases: [(u128, u64); 4] = [
            (1, 1),
            (1_000, 7),
            (u64::MAX as u128, 3),
            ((1_u128 << 60) + 5, 11),
        ];

        for (idx, (price, quantity)) in cases.into_iter().enumerate() {
            let mut result = MatchResult::new(Id::from_u64(10), quantity);
            let trade = Trade::with_timestamp(
                Id::from_uuid(parse_uuid("6ba7b810-9dad-11d1-80b4-00c04fd430c8")),
                Id::from_u64(10),
                Id::from_u64(20),
                Price::new(price),
                Quantity::new(quantity),
                Side::Buy,
                TimestampMs::new(1_616_823_000_000),
            );
            assert!(result.add_trade(trade).is_ok());

            match result.average_price() {
                Ok(Some(avg)) => {
                    assert!(
                        avg.is_finite(),
                        "case {idx}: average must be finite (price={price}, qty={quantity})"
                    );
                }
                other => panic!("case {idx}: expected Ok(Some(_)), got {other:?}"),
            }
        }
    }
}
