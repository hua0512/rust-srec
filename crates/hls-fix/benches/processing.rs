use std::{hint::black_box, sync::Arc, time::Duration};

use bytes::Bytes;
use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use hls::{HlsData, StreamProfileOptions, TsSegmentData};
use hls_fix::operators::{DefragmentOperator, SegmentSplitOperator};
use m3u8_rs::MediaSegment;
use pipeline_common::{Processor, StreamerContext};
use tokio_util::sync::CancellationToken;

fn processing(c: &mut Criterion) {
    // Supply a real recording to measure representative segment sizes. The
    // checked-in fixture keeps the benchmark reproducible without external tools.
    let ts_bytes = std::env::var_os("HLS_BENCH_TS").map_or_else(
        || Bytes::from_static(include_bytes!("../../hls/tests/fixtures/avc-640x352.ts")),
        |path| std::fs::read(path).expect("read HLS_BENCH_TS").into(),
    );
    let context = Arc::new(StreamerContext::new(CancellationToken::new()));
    let options = StreamProfileOptions {
        include_resolution: true,
    };
    let mut group = c.benchmark_group("ts_processing");
    group.throughput(Throughput::Bytes(ts_bytes.len() as u64));
    for (label, include_resolution) in [("analysis_base", false), ("analysis_resolution", true)] {
        group.bench_function(label, |b| {
            b.iter(|| {
                // Construct a fresh segment to measure parsing, not an initialized cache.
                let segment = TsSegmentData::new(MediaSegment::empty(), ts_bytes.clone());
                black_box(
                    segment
                        .analysis(StreamProfileOptions { include_resolution })
                        .unwrap(),
                );
            })
        });
    }
    let mut splitter = SegmentSplitOperator::new(context.clone());
    group.bench_function("split_cold", |b| {
        b.iter(|| {
            splitter
                .process(
                    &context,
                    HlsData::ts(MediaSegment::empty(), ts_bytes.clone()),
                    &mut |item| {
                        black_box(item);
                        Ok(())
                    },
                )
                .unwrap();
        })
    });
    let cached = HlsData::ts(MediaSegment::empty(), ts_bytes);
    assert!(
        cached
            .get_stream_profile_with_options(options)
            .unwrap()
            .has_video
    );
    let mut splitter = SegmentSplitOperator::new(context.clone());
    // This case reuses parsed metadata and does not read the segment bytes.
    group.throughput(Throughput::Elements(1));
    group.bench_function("split_cached", |b| {
        b.iter(|| {
            splitter
                .process(&context, cached.clone(), &mut |item| {
                    black_box(item);
                    Ok(())
                })
                .unwrap();
        })
    });
    group.finish();

    let init = HlsData::mp4_init(
        MediaSegment::empty(),
        Bytes::from_static(include_bytes!("../tests/fixtures/init.mp4")),
    );
    let media = HlsData::mp4_segment(
        MediaSegment::empty(),
        Bytes::from_static(include_bytes!("../tests/fixtures/media00.m4s")),
    );
    let mut defragment = DefragmentOperator::new(context.clone());
    c.bench_function("fmp4_gather_five_items", |b| {
        b.iter(|| {
            for item in [&init, &media, &media, &media, &media] {
                defragment
                    .process(&context, item.clone(), &mut |item| {
                        black_box(item);
                        Ok(())
                    })
                    .unwrap();
            }
        })
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(30)
        .warm_up_time(Duration::from_secs(1)).measurement_time(Duration::from_secs(2));
    targets = processing
}
criterion_main!(benches);
