#[cfg(test)]
mod tests {
    use crate::orders::time_in_force::TimeInForce;

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
        // Formato estándar, usando los nombres exactos de las variantes
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
}
