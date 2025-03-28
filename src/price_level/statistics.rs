use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Tracks performance statistics for a price level
#[derive(Debug)]
pub struct PriceLevelStatistics {
    /// Number of orders added
    orders_added: AtomicUsize,

    /// Number of orders removed
    orders_removed: AtomicUsize,

    /// Number of orders executed
    orders_executed: AtomicUsize,

    /// Total quantity executed
    quantity_executed: AtomicU64,

    /// Total value executed
    value_executed: AtomicU64,

    /// Last execution timestamp
    last_execution_time: AtomicU64,

    /// First order arrival timestamp
    first_arrival_time: AtomicU64,

    /// Sum of waiting times for orders
    sum_waiting_time: AtomicU64,
}

impl PriceLevelStatistics {
    /// Create new empty statistics
    pub fn new() -> Self {
        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        Self {
            orders_added: AtomicUsize::new(0),
            orders_removed: AtomicUsize::new(0),
            orders_executed: AtomicUsize::new(0),
            quantity_executed: AtomicU64::new(0),
            value_executed: AtomicU64::new(0),
            last_execution_time: AtomicU64::new(0),
            first_arrival_time: AtomicU64::new(current_time),
            sum_waiting_time: AtomicU64::new(0),
        }
    }

    /// Record a new order being added
    pub fn record_order_added(&self) {
        self.orders_added.fetch_add(1, Ordering::Relaxed);
    }

    /// Record an order being removed without execution
    pub fn record_order_removed(&self) {
        self.orders_removed.fetch_add(1, Ordering::Relaxed);
    }

    /// Record an order execution
    pub fn record_execution(&self, quantity: u64, price: u64, order_timestamp: u64) {
        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        self.orders_executed.fetch_add(1, Ordering::Relaxed);
        self.quantity_executed
            .fetch_add(quantity, Ordering::Relaxed);
        self.value_executed
            .fetch_add(quantity * price, Ordering::Relaxed);
        self.last_execution_time
            .store(current_time, Ordering::Relaxed);

        // Calculate waiting time for this order
        if order_timestamp > 0 {
            let waiting_time = current_time.saturating_sub(order_timestamp);
            self.sum_waiting_time
                .fetch_add(waiting_time, Ordering::Relaxed);
        }
    }

    /// Get total number of orders added
    pub fn orders_added(&self) -> usize {
        self.orders_added.load(Ordering::Relaxed)
    }

    /// Get total number of orders removed
    pub fn orders_removed(&self) -> usize {
        self.orders_removed.load(Ordering::Relaxed)
    }

    /// Get total number of orders executed
    pub fn orders_executed(&self) -> usize {
        self.orders_executed.load(Ordering::Relaxed)
    }

    /// Get total quantity executed
    pub fn quantity_executed(&self) -> u64 {
        self.quantity_executed.load(Ordering::Relaxed)
    }

    /// Get total value executed
    pub fn value_executed(&self) -> u64 {
        self.value_executed.load(Ordering::Relaxed)
    }

    /// Get average execution price
    pub fn average_execution_price(&self) -> Option<f64> {
        let qty = self.quantity_executed.load(Ordering::Relaxed);
        let value = self.value_executed.load(Ordering::Relaxed);

        if qty == 0 {
            None
        } else {
            Some(value as f64 / qty as f64)
        }
    }

    /// Get average waiting time for executed orders (in milliseconds)
    pub fn average_waiting_time(&self) -> Option<f64> {
        let count = self.orders_executed.load(Ordering::Relaxed);
        let sum = self.sum_waiting_time.load(Ordering::Relaxed);

        if count == 0 {
            None
        } else {
            Some(sum as f64 / count as f64)
        }
    }

    /// Get time since last execution (in milliseconds)
    pub fn time_since_last_execution(&self) -> Option<u64> {
        let last = self.last_execution_time.load(Ordering::Relaxed);
        if last == 0 {
            None
        } else {
            let current_time = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("Time went backwards")
                .as_millis() as u64;

            Some(current_time.saturating_sub(last))
        }
    }

    /// Reset all statistics
    pub fn reset(&self) {
        let current_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards")
            .as_millis() as u64;

        self.orders_added.store(0, Ordering::Relaxed);
        self.orders_removed.store(0, Ordering::Relaxed);
        self.orders_executed.store(0, Ordering::Relaxed);
        self.quantity_executed.store(0, Ordering::Relaxed);
        self.value_executed.store(0, Ordering::Relaxed);
        self.last_execution_time.store(0, Ordering::Relaxed);
        self.first_arrival_time
            .store(current_time, Ordering::Relaxed);
        self.sum_waiting_time.store(0, Ordering::Relaxed);
    }
}

impl Default for PriceLevelStatistics {
    fn default() -> Self {
        Self::new()
    }
}
