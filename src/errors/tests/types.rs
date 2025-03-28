#[cfg(test)]
mod tests {
    use crate::errors::PriceLevelError;
    use std::error::Error;

    #[test]
    fn test_parse_error_display() {
        let error = PriceLevelError::ParseError {
            message: "Failed to parse".to_string(),
        };
        assert_eq!(error.to_string(), "Failed to parse");
    }

    #[test]
    fn test_invalid_format_display() {
        let error = PriceLevelError::InvalidFormat;
        assert_eq!(error.to_string(), "Invalid format");
    }

    #[test]
    fn test_unknown_order_type_display() {
        let error = PriceLevelError::UnknownOrderType("CustomOrder".to_string());
        assert_eq!(error.to_string(), "Unknown order type: CustomOrder");
    }

    #[test]
    fn test_missing_field_display() {
        let error = PriceLevelError::MissingField("price".to_string());
        assert_eq!(error.to_string(), "Missing field: price");
    }

    #[test]
    fn test_invalid_field_value_display() {
        let error = PriceLevelError::InvalidFieldValue {
            field: "quantity".to_string(),
            value: "abc".to_string(),
        };
        assert_eq!(error.to_string(), "Invalid value for field quantity: abc");
    }

    #[test]
    fn test_invalid_operation_display() {
        let error = PriceLevelError::InvalidOperation {
            message: "Cannot update price to same value".to_string(),
        };
        assert_eq!(
            error.to_string(),
            "Invalid operation: Cannot update price to same value"
        );
    }

    #[test]
    fn test_debug_implementation() {
        // Test that Debug produces the same output as Display for our cases

        let errors = [
            PriceLevelError::ParseError {
                message: "Debug test".to_string(),
            },
            PriceLevelError::InvalidFormat,
            PriceLevelError::UnknownOrderType("TestOrder".to_string()),
            PriceLevelError::MissingField("id".to_string()),
            PriceLevelError::InvalidFieldValue {
                field: "side".to_string(),
                value: "MIDDLE".to_string(),
            },
            PriceLevelError::InvalidOperation {
                message: "Debug operation test".to_string(),
            },
        ];

        for error in &errors {
            assert_eq!(format!("{:?}", error), error.to_string());
        }
    }

    #[test]
    fn test_implements_error_trait() {
        // Test that our error type implements the standard Error trait
        let error = PriceLevelError::InvalidFormat;
        let _: &dyn Error = &error;

        // If this compiles, the test passes since it confirms
        // PriceLevelError implements the Error trait
    }

    #[test]
    fn test_error_source() {
        // Test that source() returns None as we don't have nested errors
        let error = PriceLevelError::InvalidFormat;
        assert!(error.source().is_none());
    }

    #[test]
    fn test_clone_and_compare_errors() {
        // Test error equality (though it's not derived in the original code)
        // We'll test by comparing string representation
        let error1 = PriceLevelError::MissingField("price".to_string());
        let error2 = PriceLevelError::MissingField("price".to_string());
        let error3 = PriceLevelError::MissingField("quantity".to_string());

        assert_eq!(error1.to_string(), error2.to_string());
        assert_ne!(error1.to_string(), error3.to_string());
    }

    #[test]
    fn test_error_formatting_consistency() {
        // Test that formatting is consistent across different error variants
        let parse_error = PriceLevelError::ParseError {
            message: "test message".to_string(),
        };
        assert_eq!(parse_error.to_string(), "test message");

        let field_error = PriceLevelError::MissingField("field".to_string());
        assert_eq!(field_error.to_string(), "Missing field: field");

        // Verify error messages don't have trailing whitespace or unexpected formatting
        for error in [
            PriceLevelError::InvalidFormat.to_string(),
            PriceLevelError::UnknownOrderType("Test".to_string()).to_string(),
            PriceLevelError::InvalidFieldValue {
                field: "f".to_string(),
                value: "v".to_string(),
            }
            .to_string(),
            PriceLevelError::InvalidOperation {
                message: "op".to_string(),
            }
            .to_string(),
        ] {
            assert_eq!(
                error.trim(),
                error,
                "Error message contains unexpected whitespace"
            );
        }
    }
}
