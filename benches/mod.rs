use criterion::{criterion_group, criterion_main};

mod simple;

use simple::first::benchmark_data;

criterion_group!(benches, benchmark_data,);
criterion_main!(benches);
