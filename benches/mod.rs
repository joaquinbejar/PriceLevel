// This crate root is entirely bench code, not production (issue #173's
// Production Panic Policy gate, `[lints.clippy]` in `Cargo.toml`, is
// package-wide and would otherwise apply here too). Bench fixtures freely
// `.unwrap()` / `.expect()` setup, index fixed-size scratch buffers and do
// raw arithmetic on sample sizes; none of that reaches `src/`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::string_slice,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]

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
