use criterion::{criterion_group, criterion_main};

mod price_level;
mod simple;

mod concurrent;

use concurrent::register_benchmarks as register_concurrent_benchmarks;
use concurrent::register_contention_benchmarks as register_concurrent_contention_benchmarks;
use price_level::register_benchmarks as register_price_level_benchmarks;
use simple::first::benchmark_data;

// Define the benchmark groups. `register_concurrent_contention_benchmarks`
// was previously only wired into a dead nested `criterion_group!` inside
// `concurrent/mod.rs` that this real entry point never invoked, so the
// "PriceLevel - Contention Patterns" group never ran (issue #141).
criterion_group!(
    benches,
    benchmark_data,
    register_price_level_benchmarks,
    register_concurrent_benchmarks,
    register_concurrent_contention_benchmarks,
);

criterion_main!(benches);
