use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

const NUM_MESSAGES: usize = 1_000_000;

fn mpsc_benchmark(c: &mut Criterion) {
    c.bench_function("mpsc", |b| {
        b.iter(|| {
            let (tx, rx) = mpsc::channel();
            let sender = thread::spawn(move || {
                for i in 0..NUM_MESSAGES {
                    tx.send(i).unwrap();
                }
            });
            let receiver = thread::spawn(move || {
                for _ in 0..NUM_MESSAGES {
                    black_box(rx.recv().unwrap());
                }
            });
            sender.join().unwrap();
            receiver.join().unwrap();
        })
    });
}

fn flume_benchmark(c: &mut Criterion) {
    c.bench_function("flume", |b| {
        b.iter(|| {
            let (tx, rx) = flume::unbounded();
            let sender = thread::spawn(move || {
                for i in 0..NUM_MESSAGES {
                    tx.send(i).unwrap();
                }
            });
            let receiver = thread::spawn(move || {
                for _ in 0..NUM_MESSAGES {
                    black_box(rx.recv().unwrap());
                }
            });
            sender.join().unwrap();
            receiver.join().unwrap();
        })
    });
}

fn crossbeam_benchmark(c: &mut Criterion) {
    c.bench_function("crossbeam", |b| {
        b.iter(|| {
            let (tx, rx) = crossbeam_channel::unbounded();
            let sender = thread::spawn(move || {
                for i in 0..NUM_MESSAGES {
                    tx.send(i).unwrap();
                }
            });
            let receiver = thread::spawn(move || {
                for _ in 0..NUM_MESSAGES {
                    black_box(rx.recv().unwrap());
                }
            });
            sender.join().unwrap();
            receiver.join().unwrap();
        })
    });
}

fn mpsc_bounded_benchmark(c: &mut Criterion) {
    c.bench_function("mpsc_bounded", |b| {
        b.iter(|| {
            let (tx, rx) = mpsc::sync_channel(32);
            let sender = thread::spawn(move || {
                for i in 0..NUM_MESSAGES {
                    tx.send(i).unwrap();
                }
            });
            let receiver = thread::spawn(move || {
                for _ in 0..NUM_MESSAGES {
                    black_box(rx.recv().unwrap());
                }
            });
            sender.join().unwrap();
            receiver.join().unwrap();
        })
    });
}

fn flume_bounded_benchmark(c: &mut Criterion) {
    c.bench_function("flume_bounded", |b| {
        b.iter(|| {
            let (tx, rx) = flume::bounded(32);
            let sender = thread::spawn(move || {
                for i in 0..NUM_MESSAGES {
                    tx.send(i).unwrap();
                }
            });
            let receiver = thread::spawn(move || {
                for _ in 0..NUM_MESSAGES {
                    black_box(rx.recv().unwrap());
                }
            });
            sender.join().unwrap();
            receiver.join().unwrap();
        })
    });
}

fn crossbeam_bounded_benchmark(c: &mut Criterion) {
    c.bench_function("crossbeam_bounded", |b| {
        b.iter(|| {
            let (tx, rx) = crossbeam_channel::bounded(32);
            let sender = thread::spawn(move || {
                for i in 0..NUM_MESSAGES {
                    tx.send(i).unwrap();
                }
            });
            let receiver = thread::spawn(move || {
                for _ in 0..NUM_MESSAGES {
                    black_box(rx.recv().unwrap());
                }
            });
            sender.join().unwrap();
            receiver.join().unwrap();
        })
    });
}

criterion_group!(
    name = benches;
    config = Criterion::default().measurement_time(Duration::from_secs(15));
    targets = mpsc_benchmark,
    flume_benchmark,
    crossbeam_benchmark,
    flume_bounded_benchmark,
    mpsc_bounded_benchmark,
    crossbeam_bounded_benchmark
);
criterion_main!(benches);
