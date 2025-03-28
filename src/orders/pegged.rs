/// Reference price type for pegged orders
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
