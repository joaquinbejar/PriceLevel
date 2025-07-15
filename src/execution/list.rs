use crate::errors::PriceLevelError;
use crate::execution::transaction::Transaction;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// A wrapper for a vector of transactions to implement custom serialization
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransactionList {
    pub transactions: Vec<Transaction>,
}

impl TransactionList {
    /// Create a new empty transaction list
    pub fn new() -> Self {
        Self {
            transactions: Vec::new(),
        }
    }

    /// Create a transaction list from an existing vector
    pub fn from_vec(transactions: Vec<Transaction>) -> Self {
        Self { transactions }
    }

    /// Add a transaction to the list
    pub fn add(&mut self, transaction: Transaction) {
        self.transactions.push(transaction);
    }

    /// Get a reference to the underlying vector
    pub fn as_vec(&self) -> &Vec<Transaction> {
        &self.transactions
    }

    /// Convert into a vector of transactions
    pub fn into_vec(self) -> Vec<Transaction> {
        self.transactions
    }

    pub fn is_empty(&self) -> bool {
        self.transactions.is_empty()
    }

    pub fn len(&self) -> usize {
        self.transactions.len()
    }
}

impl Default for TransactionList {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for TransactionList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Transactions:[")?;

        for (i, transaction) in self.transactions.iter().enumerate() {
            if i > 0 {
                write!(f, ",")?;
            }
            write!(f, "{transaction}")?;
        }

        write!(f, "]")
    }
}

impl FromStr for TransactionList {
    type Err = PriceLevelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if !s.starts_with("Transactions:[") || !s.ends_with("]") {
            return Err(PriceLevelError::InvalidFormat);
        }

        let content_start = s.find('[').ok_or(PriceLevelError::InvalidFormat)?;
        let content_end = s.rfind(']').ok_or(PriceLevelError::InvalidFormat)?;

        if content_start >= content_end {
            return Err(PriceLevelError::InvalidFormat);
        }

        let content = &s[content_start + 1..content_end];

        if content.is_empty() {
            return Ok(TransactionList::new());
        }

        let mut transactions = Vec::new();
        let mut current_transaction = String::new();
        let mut bracket_depth = 0;

        for c in content.chars() {
            match c {
                ',' if bracket_depth == 0 => {
                    if !current_transaction.is_empty() {
                        let transaction = Transaction::from_str(&current_transaction)?;
                        transactions.push(transaction);
                        current_transaction.clear();
                    }
                }
                '[' => {
                    bracket_depth += 1;
                    current_transaction.push(c);
                }
                ']' => {
                    bracket_depth -= 1;
                    current_transaction.push(c);
                }
                _ => current_transaction.push(c),
            }
        }

        if !current_transaction.is_empty() {
            let transaction = Transaction::from_str(&current_transaction)?;
            transactions.push(transaction);
        }

        Ok(TransactionList { transactions })
    }
}

impl From<Vec<Transaction>> for TransactionList {
    fn from(transactions: Vec<Transaction>) -> Self {
        Self::from_vec(transactions)
    }
}

impl From<TransactionList> for Vec<Transaction> {
    fn from(list: TransactionList) -> Self {
        list.into_vec()
    }
}
