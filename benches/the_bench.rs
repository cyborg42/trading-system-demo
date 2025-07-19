use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};

fn addup(n: u32) -> u32 {
    let mut sum = 0;
    for i in 0..n {
        sum += i;
    }
    sum
}

pub fn criterion_benchmark(c: &mut Criterion) {
    c.bench_function("addup 20", |b| b.iter(|| addup(black_box(20))));
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
