// benches/price_level/mod.rs
pub mod add_orders;
pub mod match_orders;
pub mod mixed_operations;
pub mod update_orders;

// Import common benchmarks into the main bench group
pub fn register_benchmarks(c: &mut criterion::Criterion) {
    add_orders::register_benchmarks(c);
    match_orders::register_benchmarks(c);
    update_orders::register_benchmarks(c);
    mixed_operations::register_benchmarks(c);
}
