use criterion::criterion_group;

mod contention;
mod register;

pub use contention::register_contention_benchmarks;
pub use register::register_benchmarks;

// Import and re-export our main concurrent benchmarks
criterion_group!(
    concurrent_benches,
    register_benchmarks,
    register_contention_benchmarks
);
