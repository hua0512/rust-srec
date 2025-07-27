use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn cloning_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("Cloning");

    let path_buf = PathBuf::from("/a/very/long/path/that/is/used/for/testing/purposes");
    group.bench_function("PathBuf", |b| b.iter(|| black_box(path_buf.clone())));

    let arc_path: Arc<Path> = path_buf.into();
    group.bench_function("Arc<Path>", |b| b.iter(|| black_box(arc_path.clone())));

    group.finish();
}

criterion_group!(benches, cloning_benchmark);
criterion_main!(benches);
