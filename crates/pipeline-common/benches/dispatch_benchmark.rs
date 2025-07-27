use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};

// Function using static dispatch with generics
fn static_dispatch<F: Fn()>(f: F) {
    f();
}

// Function using dynamic dispatch with trait objects
fn dynamic_dispatch(f: &dyn Fn()) {
    f();
}

fn dispatch_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("Dispatch");

    group.bench_function("static_dispatch", |b| {
        let closure = || black_box(());
        b.iter(|| static_dispatch(closure))
    });

    group.bench_function("dynamic_dispatch", |b| {
        let closure = || black_box(());
        let boxed_closure = &closure as &dyn Fn();
        b.iter(|| dynamic_dispatch(boxed_closure))
    });

    group.finish();
}

criterion_group!(benches, dispatch_benchmark);
criterion_main!(benches);
