use crate::errors::PriceLevelError;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Reference price type for pegged orders
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PegReferenceType {
    /// Pegged to best bid price
    BestBid,
    /// Pegged to best ask price
    BestAsk,
    /// Pegged to mid price between bid and ask
    MidPrice,
    /// Pegged to last trade price
    LastTrade,
}

impl FromStr for PegReferenceType {
    type Err = PriceLevelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "BestBid" | "BESTBID" | "bestbid" => Ok(PegReferenceType::BestBid),
            "BestAsk" | "BESTASK" | "bestask" => Ok(PegReferenceType::BestAsk),
            "MidPrice" | "MIDPRICE" | "midprice" => Ok(PegReferenceType::MidPrice),
            "LastTrade" | "LASTTRADE" | "lasttrade" => Ok(PegReferenceType::LastTrade),
            _ => Err(PriceLevelError::ParseError {
                message: s.to_string(),
            }),
        }
    }
}

impl fmt::Display for PegReferenceType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PegReferenceType::BestBid => write!(f, "BestBid"),
            PegReferenceType::BestAsk => write!(f, "BestAsk"),
            PegReferenceType::MidPrice => write!(f, "MidPrice"),
            PegReferenceType::LastTrade => write!(f, "LastTrade"),
        }
    }
}
