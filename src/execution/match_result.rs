use crate::errors::PriceLevelError;
use crate::execution::list::TransactionList;
use crate::execution::transaction::Transaction;
use crate::orders::OrderId;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Represents the result of a matching operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchResult {
    /// The ID of the incoming order that initiated the match
    pub order_id: OrderId,

    /// List of transactions that resulted from the match
    pub transactions: TransactionList,

    /// Remaining quantity of the incoming order after matching
    pub remaining_quantity: u64,

    /// Whether the order was completely filled
    pub is_complete: bool,

    /// Any orders that were completely filled and removed from the book
    pub filled_order_ids: Vec<OrderId>,
}

impl MatchResult {
    /// Create a new empty match result
    pub fn new(order_id: OrderId, initial_quantity: u64) -> Self {
        Self {
            order_id,
            transactions: TransactionList::new(),
            remaining_quantity: initial_quantity,
            is_complete: false,
            filled_order_ids: Vec::new(),
        }
    }

    /// Add a transaction to this match result
    pub fn add_transaction(&mut self, transaction: Transaction) {
        self.remaining_quantity = self.remaining_quantity.saturating_sub(transaction.quantity);
        self.is_complete = self.remaining_quantity == 0;
        self.transactions.add(transaction);
    }

    /// Add a filled order ID to track orders removed from the book
    pub fn add_filled_order_id(&mut self, order_id: OrderId) {
        self.filled_order_ids.push(order_id);
    }

    /// Get the total executed quantity
    pub fn executed_quantity(&self) -> u64 {
        self.transactions.as_vec().iter().map(|t| t.quantity).sum()
    }

    /// Get the total value executed
    pub fn executed_value(&self) -> u64 {
        self.transactions
            .as_vec()
            .iter()
            .map(|t| t.price * t.quantity)
            .sum()
    }

    /// Calculate the average execution price
    pub fn average_price(&self) -> Option<f64> {
        let executed_qty = self.executed_quantity();
        if executed_qty == 0 {
            None
        } else {
            Some(self.executed_value() as f64 / executed_qty as f64)
        }
    }
}

impl fmt::Display for MatchResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "MatchResult:order_id={};remaining_quantity={};is_complete={}",
            self.order_id, self.remaining_quantity, self.is_complete
        )?;
        write!(f, ";transactions={}", self.transactions)?;
        write!(f, ";filled_order_ids=[")?;
        for (i, order_id) in self.filled_order_ids.iter().enumerate() {
            if i > 0 {
                write!(f, ",")?;
            }
            write!(f, "{}", order_id)?;
        }
        write!(f, "]")
    }
}

impl FromStr for MatchResult {
    type Err = PriceLevelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        fn find_next_field(s: &str, start_pos: usize) -> Result<(&str, usize), PriceLevelError> {
            let mut pos = start_pos;

            while pos < s.len() {
                if s[pos..].starts_with(';') {
                    let value = &s[start_pos..pos];
                    return Ok((value, pos + 1));
                }
                pos += 1;
            }

            if pos == s.len() {
                let value = &s[start_pos..pos];
                return Ok((value, pos));
            }

            Err(PriceLevelError::InvalidFormat)
        }
        if !s.starts_with("MatchResult:") {
            return Err(PriceLevelError::InvalidFormat);
        }

        let mut order_id_str = None;
        let mut remaining_quantity_str = None;
        let mut is_complete_str = None;
        let mut transactions_str = None;
        let mut filled_order_ids_str = None;

        let mut pos = "MatchResult:".len();

        while pos < s.len() {
            let field_end = match s[pos..].find('=') {
                Some(idx) => pos + idx,
                None => return Err(PriceLevelError::InvalidFormat),
            };

            let field_name = &s[pos..field_end];
            pos = field_end + 1;
            match field_name {
                "order_id" => {
                    let (value, next_pos) = find_next_field(s, pos)?;
                    order_id_str = Some(value);
                    pos = next_pos;
                }
                "remaining_quantity" => {
                    let (value, next_pos) = find_next_field(s, pos)?;
                    remaining_quantity_str = Some(value);
                    pos = next_pos;
                }
                "is_complete" => {
                    let (value, next_pos) = find_next_field(s, pos)?;
                    is_complete_str = Some(value);
                    pos = next_pos;
                }
                "transactions" => {
                    if !s[pos..].starts_with("Transactions:[") {
                        return Err(PriceLevelError::InvalidFormat);
                    }

                    let mut bracket_depth = 1;
                    let mut i = pos + "Transactions:[".len();

                    while i < s.len() && bracket_depth > 0 {
                        if s[i..].starts_with(']') {
                            bracket_depth -= 1;
                            if bracket_depth == 0 {
                                break;
                            }
                            i += 1;
                        } else if s[i..].starts_with('[') {
                            bracket_depth += 1;
                            i += 1;
                        } else {
                            i += 1;
                        }
                    }

                    if bracket_depth > 0 {
                        return Err(PriceLevelError::InvalidFormat);
                    }

                    transactions_str = Some(&s[pos..=i]);
                    pos = i + 1;
                    if pos < s.len() && s[pos..].starts_with(';') {
                        pos += 1;
                    } else if pos < s.len() {
                        return Err(PriceLevelError::InvalidFormat);
                    }
                }
                "filled_order_ids" => {
                    if !s[pos..].starts_with('[') {
                        return Err(PriceLevelError::InvalidFormat);
                    }

                    let mut bracket_depth = 1;
                    let mut i = pos + 1;

                    while i < s.len() && bracket_depth > 0 {
                        if s[i..].starts_with(']') {
                            bracket_depth -= 1;
                            if bracket_depth == 0 {
                                break;
                            }
                            i += 1;
                        } else if s[i..].starts_with('[') {
                            bracket_depth += 1;
                            i += 1;
                        } else {
                            i += 1;
                        }
                    }

                    if bracket_depth > 0 {
                        return Err(PriceLevelError::InvalidFormat);
                    }

                    filled_order_ids_str = Some(&s[pos..=i]);

                    pos = i + 1;
                    if pos < s.len() && s[pos..].starts_with(';') {
                        pos += 1;
                    }
                }
                _ => return Err(PriceLevelError::InvalidFormat),
            }
        }

        let order_id_str =
            order_id_str.ok_or_else(|| PriceLevelError::MissingField("order_id".to_string()))?;
        let remaining_quantity_str = remaining_quantity_str
            .ok_or_else(|| PriceLevelError::MissingField("remaining_quantity".to_string()))?;
        let is_complete_str = is_complete_str
            .ok_or_else(|| PriceLevelError::MissingField("is_complete".to_string()))?;
        let transactions_str = transactions_str
            .ok_or_else(|| PriceLevelError::MissingField("transactions".to_string()))?;
        let filled_order_ids_str = filled_order_ids_str
            .ok_or_else(|| PriceLevelError::MissingField("filled_order_ids".to_string()))?;

        let order_id =
            OrderId::from_str(order_id_str).map_err(|_| PriceLevelError::InvalidFieldValue {
                field: "order_id".to_string(),
                value: order_id_str.to_string(),
            })?;

        let remaining_quantity = remaining_quantity_str.parse::<u64>().map_err(|_| {
            PriceLevelError::InvalidFieldValue {
                field: "remaining_quantity".to_string(),
                value: remaining_quantity_str.to_string(),
            }
        })?;

        let is_complete =
            is_complete_str
                .parse::<bool>()
                .map_err(|_| PriceLevelError::InvalidFieldValue {
                    field: "is_complete".to_string(),
                    value: is_complete_str.to_string(),
                })?;

        let transactions = TransactionList::from_str(transactions_str)?;

        let filled_order_ids = if filled_order_ids_str == "[]" {
            Vec::new()
        } else {
            let content = &filled_order_ids_str[1..filled_order_ids_str.len() - 1];

            if content.is_empty() {
                Vec::new()
            } else {
                content
                    .split(',')
                    .map(|id_str| {
                        OrderId::from_str(id_str).map_err(|_| PriceLevelError::InvalidFieldValue {
                            field: "filled_order_ids".to_string(),
                            value: id_str.to_string(),
                        })
                    })
                    .collect::<Result<Vec<OrderId>, PriceLevelError>>()?
            }
        };

        Ok(MatchResult {
            order_id,
            transactions,
            remaining_quantity,
            is_complete,
            filled_order_ids,
        })
    }
}
