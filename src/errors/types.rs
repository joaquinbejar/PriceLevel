use std::fmt::{Debug, Display, Formatter, Result};

/// Represents errors that can occur when processing price levels in trading operations.
///
/// This enum encapsulates various error conditions that might arise during order book
/// management, price validation, and other trading-related operations.
///
/// # Examples
///
/// ```
/// use pricelevel::PriceLevelError;
///
/// // Creating a parse error
/// let error = PriceLevelError::ParseError {
///     message: "Failed to parse price: invalid decimal format".to_string()
/// };
///
/// // Creating a missing field error
/// let missing_field_error = PriceLevelError::MissingField("price".to_string());
/// ```
pub enum PriceLevelError {
    /// Error that occurs when parsing fails with a specific message.
    ///
    /// This variant is used when string conversion or data parsing operations fail.
    ParseError {
        /// Descriptive message explaining the parsing failure
        message: String,
    },

    /// Error indicating that the input is in an invalid format.
    ///
    /// This is a general error for when the input data doesn't conform to expected patterns
    /// but doesn't fit into more specific error categories.
    InvalidFormat,

    /// Error indicating an unrecognized order type was provided.
    ///
    /// Used when the system encounters an order type string that isn't in the supported set.
    /// The string parameter contains the unrecognized order type.
    UnknownOrderType(String),

    /// Error indicating a required field is missing.
    ///
    /// Used when a mandatory field is absent in the input data.
    /// The string parameter specifies which field is missing.
    MissingField(String),

    /// Error indicating a field has an invalid value.
    ///
    /// This error occurs when a field's value is present but doesn't meet validation criteria.
    InvalidFieldValue {
        /// The name of the field with the invalid value
        field: String,
        /// The invalid value as a string representation
        value: String,
    },

    /// Error indicating an operation cannot be performed for the specified reason.
    ///
    /// Used when an action is prevented due to business rules or system constraints.
    InvalidOperation {
        /// Explanation of why the operation is invalid
        message: String,
    },
}
impl Display for PriceLevelError {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        match self {
            PriceLevelError::ParseError { message } => write!(f, "{message}"),
            PriceLevelError::InvalidFormat => write!(f, "Invalid format"),
            PriceLevelError::UnknownOrderType(order_type) => {
                write!(f, "Unknown order type: {order_type}")
            }
            PriceLevelError::MissingField(field) => write!(f, "Missing field: {field}"),
            PriceLevelError::InvalidFieldValue { field, value } => {
                write!(f, "Invalid value for field {field}: {value}")
            }
            PriceLevelError::InvalidOperation { message } => {
                write!(f, "Invalid operation: {message}")
            }
        }
    }
}

impl Debug for PriceLevelError {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        match self {
            PriceLevelError::ParseError { message } => write!(f, "{message}"),
            PriceLevelError::InvalidFormat => write!(f, "Invalid format"),
            PriceLevelError::UnknownOrderType(order_type) => {
                write!(f, "Unknown order type: {order_type}")
            }
            PriceLevelError::MissingField(field) => write!(f, "Missing field: {field}"),
            PriceLevelError::InvalidFieldValue { field, value } => {
                write!(f, "Invalid value for field {field}: {value}")
            }
            PriceLevelError::InvalidOperation { message } => {
                write!(f, "Invalid operation: {message}")
            }
        }
    }
}

impl std::error::Error for PriceLevelError {}
