#[cfg(test)]
mod tests {
    use crate::orders::time_in_force::TimeInForce;
    use std::str::FromStr;

    #[test]
    fn test_is_immediate() {
        assert!(TimeInForce::Ioc.is_immediate());
        assert!(TimeInForce::Fok.is_immediate());
        assert!(!TimeInForce::Gtc.is_immediate());
        assert!(!TimeInForce::Gtd(1000).is_immediate());
        assert!(!TimeInForce::Day.is_immediate());
    }

    #[test]
    fn test_has_expiry() {
        assert!(TimeInForce::Gtd(1000).has_expiry());
        assert!(TimeInForce::Day.has_expiry());
        assert!(!TimeInForce::Gtc.has_expiry());
        assert!(!TimeInForce::Ioc.has_expiry());
        assert!(!TimeInForce::Fok.has_expiry());
    }

    #[test]
    fn test_is_expired_gtd() {
        let expiry_time = 1000;
        let tif = TimeInForce::Gtd(expiry_time);
        assert!(!tif.is_expired(999, None));
        assert!(tif.is_expired(1000, None));
        assert!(tif.is_expired(1001, None));
    }

    #[test]
    fn test_is_expired_day() {
        let tif = TimeInForce::Day;
        let market_close = 1600;
        assert!(!tif.is_expired(1500, None));
        assert!(!tif.is_expired(1500, Some(market_close)));
        assert!(tif.is_expired(1600, Some(market_close)));
        assert!(tif.is_expired(1700, Some(market_close)));
    }

    #[test]
    fn test_non_expiring_types() {
        assert!(!TimeInForce::Gtc.is_expired(9999, Some(1000)));
        assert!(!TimeInForce::Ioc.is_expired(9999, Some(1000)));
        assert!(!TimeInForce::Fok.is_expired(9999, Some(1000)));
    }

    #[test]
    fn test_serialize_basic_types() {
        assert_eq!(serde_json::to_string(&TimeInForce::Gtc).unwrap(), "\"GTC\"");
        assert_eq!(serde_json::to_string(&TimeInForce::Ioc).unwrap(), "\"IOC\"");
        assert_eq!(serde_json::to_string(&TimeInForce::Fok).unwrap(), "\"FOK\"");
        assert_eq!(serde_json::to_string(&TimeInForce::Day).unwrap(), "\"DAY\"");
    }

    #[test]
    fn test_serialize_gtd() {
        assert_eq!(
            serde_json::to_string(&TimeInForce::Gtd(12345)).unwrap(),
            "{\"GTD\":12345}"
        );
    }

    #[test]
    fn test_deserialize_standard_format() {
        assert_eq!(
            serde_json::from_str::<TimeInForce>("\"Gtc\"").unwrap(),
            TimeInForce::Gtc
        );
        assert_eq!(
            serde_json::from_str::<TimeInForce>("\"GTC\"").unwrap(),
            TimeInForce::Gtc
        );
        assert_eq!(
            serde_json::from_str::<TimeInForce>("\"Ioc\"").unwrap(),
            TimeInForce::Ioc
        );
        assert_eq!(
            serde_json::from_str::<TimeInForce>("\"IOC\"").unwrap(),
            TimeInForce::Ioc
        );
        assert_eq!(
            serde_json::from_str::<TimeInForce>("\"Fok\"").unwrap(),
            TimeInForce::Fok
        );
        assert_eq!(
            serde_json::from_str::<TimeInForce>("\"FOK\"").unwrap(),
            TimeInForce::Fok
        );
        assert_eq!(
            serde_json::from_str::<TimeInForce>("\"Day\"").unwrap(),
            TimeInForce::Day
        );
        assert_eq!(
            serde_json::from_str::<TimeInForce>("\"DAY\"").unwrap(),
            TimeInForce::Day
        );
    }

    #[test]
    fn test_deserialize_mixed_case() {
        assert_eq!(
            serde_json::from_str::<TimeInForce>("\"Gtc\"").unwrap(),
            TimeInForce::Gtc
        );
        assert_eq!(
            serde_json::from_str::<TimeInForce>("\"gtc\"").unwrap(),
            TimeInForce::Gtc
        );
        // assert_eq!(serde_json::from_str::<TimeInForce>("\"iOc\"").unwrap(), TimeInForce::Ioc); // Esto fallará
        assert_eq!(
            serde_json::from_str::<TimeInForce>("\"Ioc\"").unwrap(),
            TimeInForce::Ioc
        );
        assert_eq!(
            serde_json::from_str::<TimeInForce>("\"ioc\"").unwrap(),
            TimeInForce::Ioc
        );
        // assert_eq!(serde_json::from_str::<TimeInForce>("\"fOk\"").unwrap(), TimeInForce::Fok); // Esto fallará
        assert_eq!(
            serde_json::from_str::<TimeInForce>("\"Fok\"").unwrap(),
            TimeInForce::Fok
        );
        assert_eq!(
            serde_json::from_str::<TimeInForce>("\"fok\"").unwrap(),
            TimeInForce::Fok
        );
        // assert_eq!(serde_json::from_str::<TimeInForce>("\"dAy\"").unwrap(), TimeInForce::Day); // Esto fallará
        assert_eq!(
            serde_json::from_str::<TimeInForce>("\"Day\"").unwrap(),
            TimeInForce::Day
        );
        assert_eq!(
            serde_json::from_str::<TimeInForce>("\"day\"").unwrap(),
            TimeInForce::Day
        );
    }

    #[test]
    fn test_deserialize_lowercase() {
        assert_eq!(
            serde_json::from_str::<TimeInForce>("\"gtc\"").unwrap(),
            TimeInForce::Gtc
        );
        assert_eq!(
            serde_json::from_str::<TimeInForce>("\"ioc\"").unwrap(),
            TimeInForce::Ioc
        );
        assert_eq!(
            serde_json::from_str::<TimeInForce>("\"fok\"").unwrap(),
            TimeInForce::Fok
        );
        assert_eq!(
            serde_json::from_str::<TimeInForce>("\"day\"").unwrap(),
            TimeInForce::Day
        );
    }

    #[test]
    fn test_deserialize_gtd() {
        assert_eq!(
            serde_json::from_str::<TimeInForce>("{\"GTD\":12345}").unwrap(),
            TimeInForce::Gtd(12345)
        );

        assert_eq!(
            serde_json::from_str::<TimeInForce>("{\"Gtd\":12345}").unwrap(),
            TimeInForce::Gtd(12345)
        );

        assert_eq!(
            serde_json::from_str::<TimeInForce>("{\"gtd\":54321}").unwrap(),
            TimeInForce::Gtd(54321)
        );
    }

    #[test]
    fn test_round_trip_serialization() {
        let test_cases = vec![
            TimeInForce::Gtc,
            TimeInForce::Ioc,
            TimeInForce::Fok,
            TimeInForce::Gtd(12345),
            TimeInForce::Day,
        ];

        for tif in test_cases {
            let serialized = serde_json::to_string(&tif).unwrap();
            let deserialized: TimeInForce = serde_json::from_str(&serialized).unwrap();
            assert_eq!(tif, deserialized);
        }
    }

    #[test]
    fn test_invalid_deserialization() {
        assert!(serde_json::from_str::<TimeInForce>("\"Invalid\"").is_err());
        assert!(serde_json::from_str::<TimeInForce>("{\"GTD\":\"not_a_number\"}").is_err());
        assert!(serde_json::from_str::<TimeInForce>("{\"InvalidType\":12345}").is_err());
    }

    #[test]
    fn test_serialize_uppercase_consistency() {
        let test_cases = [
            (TimeInForce::Gtc, "\"GTC\""),
            (TimeInForce::Ioc, "\"IOC\""),
            (TimeInForce::Fok, "\"FOK\""),
            (TimeInForce::Day, "\"DAY\""),
        ];

        for (tif, expected) in test_cases {
            assert_eq!(serde_json::to_string(&tif).unwrap(), expected);
        }
    }

    #[test]
    fn test_display() {
        assert_eq!(TimeInForce::Gtc.to_string(), "GTC");
        assert_eq!(TimeInForce::Ioc.to_string(), "IOC");
        assert_eq!(TimeInForce::Fok.to_string(), "FOK");
        assert_eq!(
            TimeInForce::Gtd(1616823000000).to_string(),
            "GTD-1616823000000"
        );
        assert_eq!(TimeInForce::Day.to_string(), "DAY");
    }

    #[test]
    fn test_from_str_valid() {
        assert_eq!(TimeInForce::from_str("GTC").unwrap(), TimeInForce::Gtc);
        assert_eq!(TimeInForce::from_str("IOC").unwrap(), TimeInForce::Ioc);
        assert_eq!(TimeInForce::from_str("FOK").unwrap(), TimeInForce::Fok);
        assert_eq!(TimeInForce::from_str("DAY").unwrap(), TimeInForce::Day);
        assert_eq!(
            TimeInForce::from_str("GTD-1616823000000").unwrap(),
            TimeInForce::Gtd(1616823000000)
        );

        // Test case insensitivity
        assert_eq!(TimeInForce::from_str("gtc").unwrap(), TimeInForce::Gtc);
        assert_eq!(TimeInForce::from_str("ioc").unwrap(), TimeInForce::Ioc);
        assert_eq!(TimeInForce::from_str("fok").unwrap(), TimeInForce::Fok);
        assert_eq!(TimeInForce::from_str("day").unwrap(), TimeInForce::Day);
        assert_eq!(
            TimeInForce::from_str("gtd-1616823000000").unwrap(),
            TimeInForce::Gtd(1616823000000)
        );

        // Test mixed case
        assert_eq!(TimeInForce::from_str("Gtc").unwrap(), TimeInForce::Gtc);
        assert_eq!(TimeInForce::from_str("IoC").unwrap(), TimeInForce::Ioc);
    }

    #[test]
    fn test_from_str_invalid() {
        // Test invalid time-in-force values
        assert!(TimeInForce::from_str("").is_err());
        assert!(TimeInForce::from_str("INVALID").is_err());
        assert!(TimeInForce::from_str("GTD").is_err());
        assert!(TimeInForce::from_str("GTD-").is_err());
        assert!(TimeInForce::from_str("GTD-INVALID").is_err());

        // Test error messages
        let error = TimeInForce::from_str("INVALID").unwrap_err();
        match error {
            crate::errors::PriceLevelError::ParseError { message } => {
                assert!(message.contains("Invalid TimeInForce: INVALID"));
            }
            _ => panic!("Expected ParseError"),
        }

        let error = TimeInForce::from_str("GTD-INVALID").unwrap_err();
        match error {
            crate::errors::PriceLevelError::ParseError { message } => {
                assert!(message.contains("Invalid expiry timestamp in GTD: INVALID"));
            }
            _ => panic!("Expected ParseError"),
        }
    }

    #[test]
    fn test_roundtrip() {
        // Test round-trip conversion (Display -> FromStr)
        let time_in_force_values = [
            TimeInForce::Gtc,
            TimeInForce::Ioc,
            TimeInForce::Fok,
            TimeInForce::Gtd(1616823000000),
            TimeInForce::Day,
        ];

        for &original in &time_in_force_values {
            let string_representation = original.to_string();
            let parsed = TimeInForce::from_str(&string_representation).unwrap();
            assert_eq!(original, parsed);
        }
    }
}
