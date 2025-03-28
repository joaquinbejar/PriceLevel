#[cfg(test)]
mod tests_order_status {
    use crate::orders::status::OrderStatus;
    use std::str::FromStr;

    #[test]
    fn test_is_active() {
        assert!(OrderStatus::Active.is_active());
        assert!(OrderStatus::PartiallyFilled.is_active());

        assert!(!OrderStatus::New.is_active());
        assert!(!OrderStatus::Filled.is_active());
        assert!(!OrderStatus::Canceled.is_active());
        assert!(!OrderStatus::Rejected.is_active());
        assert!(!OrderStatus::Expired.is_active());
    }

    #[test]
    fn test_is_terminated() {
        assert!(OrderStatus::Filled.is_terminated());
        assert!(OrderStatus::Canceled.is_terminated());
        assert!(OrderStatus::Rejected.is_terminated());
        assert!(OrderStatus::Expired.is_terminated());

        assert!(!OrderStatus::New.is_terminated());
        assert!(!OrderStatus::Active.is_terminated());
        assert!(!OrderStatus::PartiallyFilled.is_terminated());
    }

    #[test]
    fn test_active_and_terminated_are_mutually_exclusive() {
        // Test that no status can be both active and terminated
        for status in [
            OrderStatus::New,
            OrderStatus::Active,
            OrderStatus::PartiallyFilled,
            OrderStatus::Filled,
            OrderStatus::Canceled,
            OrderStatus::Rejected,
            OrderStatus::Expired,
        ] {
            assert!(
                !(status.is_active() && status.is_terminated()),
                "{:?} should not be both active and terminated",
                status
            );
        }
    }

    #[test]
    fn test_from_str_valid() {
        // Test all valid status values with exact case
        assert_eq!(OrderStatus::from_str("NEW").unwrap(), OrderStatus::New);
        assert_eq!(
            OrderStatus::from_str("ACTIVE").unwrap(),
            OrderStatus::Active
        );
        assert_eq!(
            OrderStatus::from_str("PARTIALLYFILLED").unwrap(),
            OrderStatus::PartiallyFilled
        );
        assert_eq!(
            OrderStatus::from_str("FILLED").unwrap(),
            OrderStatus::Filled
        );
        assert_eq!(
            OrderStatus::from_str("CANCELED").unwrap(),
            OrderStatus::Canceled
        );
        assert_eq!(
            OrderStatus::from_str("REJECTED").unwrap(),
            OrderStatus::Rejected
        );
        assert_eq!(
            OrderStatus::from_str("EXPIRED").unwrap(),
            OrderStatus::Expired
        );

        // Test with different cases
        assert_eq!(OrderStatus::from_str("new").unwrap(), OrderStatus::New);
        assert_eq!(
            OrderStatus::from_str("Active").unwrap(),
            OrderStatus::Active
        );
        assert_eq!(
            OrderStatus::from_str("partiallyFilled").unwrap(),
            OrderStatus::PartiallyFilled
        );
    }

    #[test]
    fn test_from_str_invalid() {
        // Test with invalid values
        assert!(OrderStatus::from_str("").is_err());
        assert!(OrderStatus::from_str("UNKNOWN").is_err());
        assert!(OrderStatus::from_str("PARTIALLY_FILLED").is_err());
        assert!(OrderStatus::from_str("CANCEL").is_err());

        // Verify error message
        let error = OrderStatus::from_str("INVALID").unwrap_err();
        if let crate::errors::PriceLevelError::ParseError { message } = error {
            assert!(message.contains("Invalid OrderStatus: INVALID"));
        } else {
            panic!("Expected ParseError, got {:?}", error);
        }
    }

    #[test]
    fn test_display() {
        // Test that display outputs the expected string
        assert_eq!(OrderStatus::New.to_string(), "NEW");
        assert_eq!(OrderStatus::Active.to_string(), "ACTIVE");
        assert_eq!(OrderStatus::PartiallyFilled.to_string(), "PARTIALLYFILLED");
        assert_eq!(OrderStatus::Filled.to_string(), "FILLED");
        assert_eq!(OrderStatus::Canceled.to_string(), "CANCELED");
        assert_eq!(OrderStatus::Rejected.to_string(), "REJECTED");
        assert_eq!(OrderStatus::Expired.to_string(), "EXPIRED");
    }

    #[test]
    fn test_serialization() {
        // Test serialization
        assert_eq!(serde_json::to_string(&OrderStatus::New).unwrap(), "\"New\"");
        assert_eq!(
            serde_json::to_string(&OrderStatus::Active).unwrap(),
            "\"Active\""
        );
        assert_eq!(
            serde_json::to_string(&OrderStatus::PartiallyFilled).unwrap(),
            "\"PartiallyFilled\""
        );
        assert_eq!(
            serde_json::to_string(&OrderStatus::Filled).unwrap(),
            "\"Filled\""
        );
        assert_eq!(
            serde_json::to_string(&OrderStatus::Canceled).unwrap(),
            "\"Canceled\""
        );
        assert_eq!(
            serde_json::to_string(&OrderStatus::Rejected).unwrap(),
            "\"Rejected\""
        );
        assert_eq!(
            serde_json::to_string(&OrderStatus::Expired).unwrap(),
            "\"Expired\""
        );
    }

    #[test]
    fn test_deserialization() {
        // Test deserialization
        assert_eq!(
            serde_json::from_str::<OrderStatus>("\"New\"").unwrap(),
            OrderStatus::New
        );
        assert_eq!(
            serde_json::from_str::<OrderStatus>("\"Active\"").unwrap(),
            OrderStatus::Active
        );
        assert_eq!(
            serde_json::from_str::<OrderStatus>("\"PartiallyFilled\"").unwrap(),
            OrderStatus::PartiallyFilled
        );
        assert_eq!(
            serde_json::from_str::<OrderStatus>("\"Filled\"").unwrap(),
            OrderStatus::Filled
        );
        assert_eq!(
            serde_json::from_str::<OrderStatus>("\"Canceled\"").unwrap(),
            OrderStatus::Canceled
        );
        assert_eq!(
            serde_json::from_str::<OrderStatus>("\"Rejected\"").unwrap(),
            OrderStatus::Rejected
        );
        assert_eq!(
            serde_json::from_str::<OrderStatus>("\"Expired\"").unwrap(),
            OrderStatus::Expired
        );
    }

    #[test]
    fn test_roundtrip_parsing() {
        // Test round trip from enum to string and back
        for status in [
            OrderStatus::New,
            OrderStatus::Active,
            OrderStatus::PartiallyFilled,
            OrderStatus::Filled,
            OrderStatus::Canceled,
            OrderStatus::Rejected,
            OrderStatus::Expired,
        ] {
            let string_representation = status.to_string();
            let parsed_back = OrderStatus::from_str(&string_representation).unwrap();
            assert_eq!(status, parsed_back);
        }
    }
}
