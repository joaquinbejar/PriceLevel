#[cfg(test)]
mod tests {
    use crate::execution::match_result::MatchResult;
    use crate::execution::trade::Trade;
    use crate::orders::{Id, Side};
    use std::str::FromStr;
    use uuid::Uuid;

    fn parse_uuid(input: &str) -> Uuid {
        match Uuid::parse_str(input) {
            Ok(value) => value,
            Err(error) => panic!("failed to parse uuid: {error}"),
        }
    }

    fn sample_trade(quantity: u64) -> Trade {
        Trade {
            trade_id: Id::from_uuid(parse_uuid("6ba7b810-9dad-11d1-80b4-00c04fd430c8")),
            taker_order_id: Id::from_u64(10),
            maker_order_id: Id::from_u64(20),
            price: 1_000,
            quantity,
            taker_side: Side::Buy,
            timestamp: 1_616_823_000_000,
        }
    }

    #[test]
    fn add_trade_updates_remaining_and_trades() {
        let mut result = MatchResult::new(Id::from_u64(10), 100);
        assert!(result.add_trade(sample_trade(25)).is_ok());

        assert_eq!(result.remaining_quantity, 75);
        assert_eq!(result.trades.len(), 1);
        assert!(!result.is_complete);
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

        assert_eq!(parsed.trades.len(), 1);
        assert_eq!(parsed.remaining_quantity, 60);
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
        assert_eq!(result.remaining_quantity, 10);
        assert_eq!(result.trades.len(), 0);
    }

    #[test]
    fn executed_value_rejects_overflow() {
        let mut result = MatchResult::new(Id::from_u64(10), 4);

        let trade = Trade {
            trade_id: Id::from_uuid(parse_uuid("6ba7b810-9dad-11d1-80b4-00c04fd430c8")),
            taker_order_id: Id::from_u64(10),
            maker_order_id: Id::from_u64(20),
            price: u128::MAX,
            quantity: 2,
            taker_side: Side::Buy,
            timestamp: 1_616_823_000_000,
        };

        assert!(result.add_trade(trade).is_ok());
        assert!(result.executed_value().is_err());
    }
}
