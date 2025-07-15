#[cfg(test)]
mod tests {
    use crate::errors::PriceLevelError;
    use crate::orders::PegReferenceType;
    use std::str::FromStr;

    #[test]
    fn test_peg_reference_type_from_str_best_bid() {
        assert_eq!(
            PegReferenceType::from_str("BestBid").unwrap(),
            PegReferenceType::BestBid
        );
        assert_eq!(
            PegReferenceType::from_str("BESTBID").unwrap(),
            PegReferenceType::BestBid
        );
        assert_eq!(
            PegReferenceType::from_str("bestbid").unwrap(),
            PegReferenceType::BestBid
        );
    }

    #[test]
    fn test_peg_reference_type_from_str_best_ask() {
        assert_eq!(
            PegReferenceType::from_str("BestAsk").unwrap(),
            PegReferenceType::BestAsk
        );
        assert_eq!(
            PegReferenceType::from_str("BESTASK").unwrap(),
            PegReferenceType::BestAsk
        );
        assert_eq!(
            PegReferenceType::from_str("bestask").unwrap(),
            PegReferenceType::BestAsk
        );
    }

    #[test]
    fn test_peg_reference_type_from_str_mid_price() {
        assert_eq!(
            PegReferenceType::from_str("MidPrice").unwrap(),
            PegReferenceType::MidPrice
        );
        assert_eq!(
            PegReferenceType::from_str("MIDPRICE").unwrap(),
            PegReferenceType::MidPrice
        );
        assert_eq!(
            PegReferenceType::from_str("midprice").unwrap(),
            PegReferenceType::MidPrice
        );
    }

    #[test]
    fn test_peg_reference_type_from_str_last_trade() {
        assert_eq!(
            PegReferenceType::from_str("LastTrade").unwrap(),
            PegReferenceType::LastTrade
        );
        assert_eq!(
            PegReferenceType::from_str("LASTTRADE").unwrap(),
            PegReferenceType::LastTrade
        );
        assert_eq!(
            PegReferenceType::from_str("lasttrade").unwrap(),
            PegReferenceType::LastTrade
        );
    }

    #[test]
    fn test_peg_reference_type_from_str_error() {
        let error = PegReferenceType::from_str("InvalidType").unwrap_err();
        if let PriceLevelError::ParseError {
            message: actual_message,
        } = error
        {
            assert_eq!(actual_message, "InvalidType");
        } else {
            panic!("Expected PriceLevelError::ParseError, got {error:?}");
        }

        let error = PegReferenceType::from_str("").unwrap_err();
        if let PriceLevelError::ParseError {
            message: actual_message,
        } = error
        {
            assert_eq!(actual_message, "");
        } else {
            panic!("Expected PriceLevelError::ParseError, got {error:?}");
        }

        let error = PegReferenceType::from_str("Best").unwrap_err();
        if let PriceLevelError::ParseError {
            message: actual_message,
        } = error
        {
            assert_eq!(actual_message, "Best");
        } else {
            panic!("Expected PriceLevelError::ParseError, got {error:?}");
        }
    }

    #[test]
    fn test_peg_reference_type_display() {
        assert_eq!(PegReferenceType::BestBid.to_string(), "BestBid");
        assert_eq!(PegReferenceType::BestAsk.to_string(), "BestAsk");
        assert_eq!(PegReferenceType::MidPrice.to_string(), "MidPrice");
        assert_eq!(PegReferenceType::LastTrade.to_string(), "LastTrade");
    }

    #[test]
    fn test_peg_reference_type_error_display() {
        let error = PriceLevelError::ParseError {
            message: "InvalidType".to_string(),
        };
        assert_eq!(error.to_string(), "InvalidType");
    }

    #[test]
    fn test_peg_reference_type_serde() {
        // Test serialization
        let reference_type = PegReferenceType::BestBid;
        let serialized = serde_json::to_string(&reference_type).unwrap();
        assert_eq!(serialized, "\"BestBid\"");

        // Test deserialization
        let deserialized: PegReferenceType = serde_json::from_str("\"BestAsk\"").unwrap();
        assert_eq!(deserialized, PegReferenceType::BestAsk);

        let deserialized: PegReferenceType = serde_json::from_str("\"MidPrice\"").unwrap();
        assert_eq!(deserialized, PegReferenceType::MidPrice);

        let deserialized: PegReferenceType = serde_json::from_str("\"LastTrade\"").unwrap();
        assert_eq!(deserialized, PegReferenceType::LastTrade);
    }

    #[test]
    fn test_peg_reference_type_round_trip() {
        // Test from_str -> to_string round trip
        for reference_type in [
            PegReferenceType::BestBid,
            PegReferenceType::BestAsk,
            PegReferenceType::MidPrice,
            PegReferenceType::LastTrade,
        ] {
            let string_representation = reference_type.to_string();
            let parsed_back = PegReferenceType::from_str(&string_representation).unwrap();
            assert_eq!(reference_type, parsed_back);
        }
    }

    #[test]
    fn test_peg_reference_type_error_implements_std_error() {
        // Verify that PegReferenceTypeParseError implements std::error::Error
        let error = PriceLevelError::ParseError {
            message: "test".to_string(),
        };

        // This will fail to compile if PegReferenceTypeParseError doesn't implement std::error::Error
        let _: Box<dyn std::error::Error> = Box::new(error);
    }
}
