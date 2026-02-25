// benches/price_level/mod.rs
pub mod add_orders;
pub mod checked_arithmetic;
pub mod lifecycle;
pub mod match_orders;
pub mod mixed_operations;
pub mod newtypes;
pub mod serialization;
pub mod snapshot_recovery;
pub mod special_orders;
pub mod update_orders;

// Import common benchmarks into the main bench group
pub fn register_benchmarks(c: &mut criterion::Criterion) {
    add_orders::register_benchmarks(c);
    match_orders::register_benchmarks(c);
    update_orders::register_benchmarks(c);
    mixed_operations::register_benchmarks(c);
    snapshot_recovery::register_benchmarks(c);
    checked_arithmetic::register_benchmarks(c);
    serialization::register_benchmarks(c);
    newtypes::register_benchmarks(c);
    special_orders::register_benchmarks(c);
    lifecycle::register_benchmarks(c);
}
