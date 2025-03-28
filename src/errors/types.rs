use std::fmt::{Debug, Display, Formatter, Result};

pub enum PriceLevelError {
    ParseError { message: String },
    InvalidFormat,
    UnknownOrderType(String),
    MissingField(String),
    InvalidFieldValue { field: String, value: String },
}

impl Display for PriceLevelError {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        match self {
            PriceLevelError::ParseError { message } => write!(f, "{}", message),
            PriceLevelError::InvalidFormat => write!(f, "Invalid format"),
            PriceLevelError::UnknownOrderType(order_type) => {
                write!(f, "Unknown order type: {}", order_type)
            }
            PriceLevelError::MissingField(field) => write!(f, "Missing field: {}", field),
            PriceLevelError::InvalidFieldValue { field, value } => {
                write!(f, "Invalid value for field {}: {}", field, value)
            }
        }
    }
}

impl Debug for PriceLevelError {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        match self {
            PriceLevelError::ParseError { message } => write!(f, "{}", message),
            PriceLevelError::InvalidFormat => write!(f, "Invalid format"),
            PriceLevelError::UnknownOrderType(order_type) => {
                write!(f, "Unknown order type: {}", order_type)
            }
            PriceLevelError::MissingField(field) => write!(f, "Missing field: {}", field),
            PriceLevelError::InvalidFieldValue { field, value } => {
                write!(f, "Invalid value for field {}: {}", field, value)
            }
        }
    }
}

impl std::error::Error for PriceLevelError {}
