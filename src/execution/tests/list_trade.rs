#[cfg(test)]
mod tests {
    use crate::execution::list::TradeList;
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

    fn sample_trade() -> Trade {
        Trade {
            trade_id: Id::from_uuid(parse_uuid("6ba7b810-9dad-11d1-80b4-00c04fd430c8")),
            taker_order_id: Id::from_u64(1),
            maker_order_id: Id::from_u64(2),
            price: 10_000,
            quantity: 5,
            taker_side: Side::Buy,
            timestamp: 1_616_823_000_000,
        }
    }

    #[test]
    fn trade_list_display_and_parse_roundtrip() {
        let mut list = TradeList::new();
        list.add(sample_trade());

        let rendered = list.to_string();
        assert!(rendered.starts_with("Trades:["));
        assert!(rendered.contains("Trade:trade_id="));

        let parsed = match TradeList::from_str(&rendered) {
            Ok(value) => value,
            Err(error) => panic!("failed to parse trade list: {error:?}"),
        };

        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed.as_vec()[0].trade_id, list.as_vec()[0].trade_id);
    }

    #[test]
    fn trade_list_from_str_rejects_old_prefix() {
        let result = TradeList::from_str("Transactions:[]");
        assert!(result.is_err());
    }
}
