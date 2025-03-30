#![allow(unknown_lints)]
#![allow(clippy::literal_string_with_formatting_args)]

//!
//!  [![Dual License](https://img.shields.io/badge/license-MIT%20and%20Apache%202.0-blue)](./LICENSE)
//!  [![Crates.io](https://img.shields.io/crates/v/pricelevel.svg)](https://crates.io/crates/pricelevel)
//!  [![Downloads](https://img.shields.io/crates/d/pricelevel.svg)](https://crates.io/crates/pricelevel)
//!  [![Stars](https://img.shields.io/github/stars/joaquinbejar/PriceLevel.svg)](https://github.com/joaquinbejar/PriceLevel/stargazers)
//!  [![Issues](https://img.shields.io/github/issues/joaquinbejar/PriceLevel.svg)](https://github.com/joaquinbejar/PriceLevel/issues)
//!  [![PRs](https://img.shields.io/github/issues-pr/joaquinbejar/PriceLevel.svg)](https://github.com/joaquinbejar/PriceLevel/pulls)
//!
//!  [![Build Status](https://img.shields.io/github/workflow/status/joaquinbejar/PriceLevel/CI)](https://github.com/joaquinbejar/PriceLevel/actions)
//!  [![Coverage](https://img.shields.io/codecov/c/github/joaquinbejar/PriceLevel)](https://codecov.io/gh/joaquinbejar/PriceLevel)
//!  [![Dependencies](https://img.shields.io/librariesio/github/joaquinbejar/PriceLevel)](https://libraries.io/github/joaquinbejar/PriceLevel)
//!  [![Documentation](https://img.shields.io/badge/docs-latest-blue.svg)](https://docs.rs/pricelevel)
//!
//!  # PriceLevel
//!
//!  A high-performance, lock-free price level implementation for limit order books in Rust. This library provides the building blocks for creating efficient trading systems with support for multiple order types and concurrent access patterns.
//!
//!  ## Features
//!
//!  - Lock-free architecture for high-throughput trading applications
//!  - Support for diverse order types including standard limit orders, iceberg orders, post-only, fill-or-kill, and more
//!  - Thread-safe operations with atomic counters and lock-free data structures
//!  - Efficient order matching and execution logic
//!  - Designed with domain-driven principles for financial markets
//!  - Comprehensive test suite demonstrating concurrent usage scenarios
//!  - Built with crossbeam's lock-free data structures
//!  - Optimized statistics tracking for each price level
//!  - Memory-efficient implementations suitable for high-frequency trading systems
//!
//!  Perfect for building matching engines, market data systems, algorithmic trading platforms, and financial exchanges where performance and correctness are critical.
//!
//!  ## Supported Order Types
//!
//!  The library provides comprehensive support for various order types used in modern trading systems:
//!
//!  - **Standard Limit Order**: Basic price-quantity orders with specified execution price
//!  - **Iceberg Order**: Orders with visible and hidden quantities that replenish automatically
//!  - **Post-Only Order**: Orders that will not execute immediately against existing orders
//!  - **Trailing Stop Order**: Orders that adjust based on market price movements
//!  - **Pegged Order**: Orders that adjust their price based on a reference price
//!  - **Market-to-Limit Order**: Orders that convert to limit orders after initial execution
//!  - **Reserve Order**: Orders with custom replenishment logic for visible quantities
//!
//!  ## Time-in-Force Options
//!
//!  The library supports the following time-in-force policies:
//!
//!  - **Good Till Canceled (GTC)**: Order remains active until explicitly canceled
//!  - **Immediate Or Cancel (IOC)**: Order must be filled immediately (partially or completely) or canceled
//!  - **Fill Or Kill (FOK)**: Order must be filled completely immediately or canceled entirely
//!  - **Good Till Date (GTD)**: Order remains active until a specified date/time
//!  - **Day Order**: Order valid only for the current trading day
//!
//!  ## Implementation Details
//!
//!  - **Thread Safety**: Uses atomic operations and lock-free data structures to ensure thread safety without mutex locks
//!  - **Order Queue Management**: Specialized order queue implementation based on crossbeam's SegQueue
//!  - **Statistics Tracking**: Each price level tracks execution statistics in real-time
//!  - **Snapshot Capabilities**: Create point-in-time snapshots of price levels for market data distribution
//!  - **Efficient Matching**: Optimized algorithms for matching incoming orders against existing orders
//!  - **Support for Special Order Types**: Custom handling for iceberg orders, reserve orders, and other special types
//!
//!  ## Price Level Features
//!
//!  - **Atomic Counters**: Uses atomic types for thread-safe quantity tracking
//!  - **Efficient Order Storage**: Optimized data structures for order storage and retrieval
//!  - **Visibility Controls**: Separate tracking of visible and hidden quantities
//!  - **Performance Monitoring**: Built-in statistics for monitoring execution performance
//!  - **Order Matching Logic**: Sophisticated algorithms for matching orders at each price level
//!
//! ## Performance Benchmark Results
//!
//! The `pricelevel` library has been thoroughly tested for performance in high-frequency trading scenarios. Below are the results from a recent simulation conducted on an M4 Max processor, demonstrating the library's capability to handle intensive concurrent trading operations.
//!
//! ### Simulation Parameters
//!
//! - **Price Level**: 10000
//! - **Duration**: 5000 ms (5 seconds)
//! - **Threads**: 30 total
//!   - 10 maker threads (adding orders)
//!   - 10 taker threads (executing matches)
//!   - 10 canceller threads (cancelling orders)
//! - **Initial Orders**: 1000 orders seeded before simulation
//!
//! ### Performance Metrics
//!
//! | Metric | Total Operations | Rate (per second) |
//! |--------|-----------------|-------------------|
//! | Orders Added | 329,558 | 65,151.94 |
//! | Matches Executed | 147,398 | 29,139.84 |
//! | Cancellations | 27,119 | 5,361.29 |
//! | **Total Operations** | **504,075** | **99,653.07** |
//!
//! ### Final State After Simulation
//!
//! - **Price**: 10000
//! - **Visible Quantity**: 2,172,773
//! - **Hidden Quantity**: 1,880,104
//! - **Total Quantity**: 4,052,877
//! - **Order Count**: 326,040
//!
//! ### Price Level Statistics
//!
//! - **Orders Added**: 330,558
//! - **Orders Removed**: 159
//! - **Orders Executed**: 156,597
//! - **Quantity Executed**: 441,708
//! - **Value Executed**: 4,417,080,000
//! - **Average Execution Price**: 10,000.00
//! - **Average Waiting Time**: 1,236.15 ms
//! - **Time Since Last Execution**: 58 ms
//!
//! ### Analysis
//!
//! The simulation demonstrates the library's capability to handle nearly **100,000 operations per second** with multiple concurrent threads operating on the same price level. This includes a mix of order additions, executions, and cancellations - providing a realistic simulation of a high-frequency trading environment.
//!
//! The lock-free architecture enables high throughput while maintaining data consistency. The minimal difference between orders added (329,558) and the final order count (326,040) indicates efficient order processing with minimal overhead.
//!
//! These performance characteristics make the `pricelevel` library suitable for production use in high-performance trading systems, matching engines, and other financial applications where microsecond-level performance is critical.
//!
//!  ## Setup Instructions
//!  
//!  1. Clone the repository:
//!  ```shell
//!  git clone https://github.com/joaquinbejar/PriceLevel.git
//!  cd PriceLevel
//!  ```
//!  
//!  2. Build the project:
//!  ```shell
//!  make build
//!  ```
//!  
//!  3. Run tests:
//!  ```shell
//!  make test
//!  ```
//!  
//!  4. Format the code:
//!  ```shell
//!  make fmt
//!  ```
//!  
//!  5. Run linting:
//!  ```shell
//!  make lint
//!  ```
//!  
//!  6. Clean the project:
//!  ```shell
//!  make clean
//!  ```
//!  
//!  7. Run the project:
//!  ```shell
//!  make run
//!  ```
//!  
//!  8. Fix issues:
//!  ```shell
//!  make fix
//!  ```
//!  
//!  9. Run pre-push checks:
//!  ```shell
//!  make pre-push
//!  ```
//!  
//!  10. Generate documentation:
//!  ```shell
//!  make doc
//!  ```
//!  
//!  11. Publish the package:
//!  ```shell
//!  make publish
//!  ```
//!  
//!  12. Generate coverage report:
//!  ```shell
//!  make coverage
//!  ```
//!
//!  ## Library Usage
//!  
//!  To use the library in your project, add the following to your `Cargo.toml`:
//!  
//!  ```toml
//!  [dependencies]
//!  pricelevel = { git = "https://github.com/joaquinbejar/PriceLevel.git" }
//!  ```
//!  
//!  ## Usage Examples
//!  
//!  Here are some examples of how to use the library:
//!  

//!  
//!  ## Testing
//!  
//!  To run unit tests:
//!  ```shell
//!  make test
//!  ```
//!  
//!  To run tests with coverage:
//!  ```shell
//!  make coverage
//!  ```
//!  
//!  ## Contribution and Contact
//!  
//!  We welcome contributions to this project! If you would like to contribute, please follow these steps:
//!  
//!  1. Fork the repository.
//!  2. Create a new branch for your feature or bug fix.
//!  3. Make your changes and ensure that the project still builds and all tests pass.
//!  4. Commit your changes and push your branch to your forked repository.
//!  5. Submit a pull request to the main repository.
//!  
//!  If you have any questions, issues, or would like to provide feedback, please feel free to contact the project maintainer:
//!  
//!  **Joaquín Béjar García**
//!  - Email: jb@taunais.com
//!  - GitHub: [joaquinbejar](https://github.com/joaquinbejar)
//!  
//!  We appreciate your interest and look forward to your contributions!
//!  
//!
mod orders;
mod price_level;
mod utils;

mod errors;
mod execution;

pub use errors::PriceLevelError;
pub use execution::{MatchResult, Transaction};
pub use orders::DEFAULT_RESERVE_REPLENISH_AMOUNT;
pub use orders::PegReferenceType;
pub use orders::{OrderId, OrderType, OrderUpdate, Side, TimeInForce};
pub use price_level::{OrderQueue, PriceLevel, PriceLevelData, PriceLevelSnapshot};
pub use utils::{UuidGenerator, setup_logger};
