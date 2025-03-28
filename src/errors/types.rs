use std::fmt;
use std::fmt::{Debug, Formatter};

pub enum PriceLevelError {
    ParseError{ 
        message: String
    },
}

impl fmt::Display for PriceLevelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PriceLevelError::ParseError { message } => write!(f, "{}", message),
            _ => {
                write!(f, "An unknown error occurred")
            }
        }
    }
}


impl Debug for PriceLevelError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            PriceLevelError::ParseError{ message } => write!(f, "{}", message),
            _ => {
                write!(f, "An unknown error occurred")
            }
        }
    }
}

impl std::error::Error for PriceLevelError {}