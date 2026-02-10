use std::hint::black_box;

use av1::ObuType;
use av1::obu_stream::write_obu;
use bytes::Bytes;
use criterion::{Criterion, criterion_group, criterion_main};
use mp4::fragment::{
    Av1ValidationOptions, extract_av1_track_ids_from_init, validate_av1_media_segment_against_init,
    validate_av1_media_segment_with_track_ids_and_options,
};

fn make_box(fourcc: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let size = (8 + body.len()) as u32;
    let mut out = Vec::with_capacity(size as usize);
    out.extend_from_slice(&size.to_be_bytes());
    out.extend_from_slice(fourcc);
    out.extend_from_slice(body);
    out
}

fn make_full_box(fourcc: &[u8; 4], version: u8, flags: u32, payload: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(4 + payload.len());
    body.push(version);
    body.push(((flags >> 16) & 0xFF) as u8);
    body.push(((flags >> 8) & 0xFF) as u8);
    body.push((flags & 0xFF) as u8);
    body.extend_from_slice(payload);
    make_box(fourcc, &body)
}

fn make_init_with_video_sample_entry(track_id: u32, sample_entry: [u8; 4]) -> Bytes {
    let mut tkhd_payload = Vec::new();
    tkhd_payload.extend_from_slice(&0u32.to_be_bytes());
    tkhd_payload.extend_from_slice(&0u32.to_be_bytes());
    tkhd_payload.extend_from_slice(&track_id.to_be_bytes());
    tkhd_payload.extend_from_slice(&0u32.to_be_bytes());
    let tkhd = make_full_box(b"tkhd", 0, 0, &tkhd_payload);

    let sample_entry_box = make_box(&sample_entry, &[]);
    let mut stsd_payload = Vec::new();
    stsd_payload.extend_from_slice(&1u32.to_be_bytes());
    stsd_payload.extend_from_slice(&sample_entry_box);
    let stsd = make_full_box(b"stsd", 0, 0, &stsd_payload);

    let stbl = make_box(b"stbl", &stsd);
    let minf = make_box(b"minf", &stbl);
    let mdia = make_box(b"mdia", &minf);

    let mut trak_body = Vec::new();
    trak_body.extend_from_slice(&tkhd);
    trak_body.extend_from_slice(&mdia);
    let trak = make_box(b"trak", &trak_body);

    let moov = make_box(b"moov", &trak);
    Bytes::from(moov)
}

fn make_media_segment_for_track_samples(track_id: u32, samples: &[Vec<u8>]) -> Bytes {
    let mut tfhd_payload = Vec::new();
    tfhd_payload.extend_from_slice(&track_id.to_be_bytes());
    let tfhd = make_full_box(b"tfhd", 0, 0, &tfhd_payload);

    let trun_flags = 0x000001 | 0x000200;
    let mut trun_payload = Vec::new();
    trun_payload.extend_from_slice(&(samples.len() as u32).to_be_bytes());
    trun_payload.extend_from_slice(&0i32.to_be_bytes());
    for sample in samples {
        trun_payload.extend_from_slice(&(sample.len() as u32).to_be_bytes());
    }
    let mut trun = make_full_box(b"trun", 0, trun_flags, &trun_payload);

    let mut traf_body = Vec::new();
    traf_body.extend_from_slice(&tfhd);
    traf_body.extend_from_slice(&trun);
    let traf = make_box(b"traf", &traf_body);

    let moof = make_box(b"moof", &traf);
    let mut mdat_payload = Vec::new();
    for sample in samples {
        mdat_payload.extend_from_slice(sample);
    }
    let mdat = make_box(b"mdat", &mdat_payload);

    let data_offset = (moof.len() + 8) as i32;
    let trun_data_offset_pos = 8 + 4 + 4;
    trun[trun_data_offset_pos..trun_data_offset_pos + 4]
        .copy_from_slice(&data_offset.to_be_bytes());

    let mut traf_body = Vec::new();
    traf_body.extend_from_slice(&tfhd);
    traf_body.extend_from_slice(&trun);
    let traf = make_box(b"traf", &traf_body);
    let moof = make_box(b"moof", &traf);

    let mut out = Vec::new();
    out.extend_from_slice(&moof);
    out.extend_from_slice(&mdat);
    Bytes::from(out)
}

fn build_av1_frame_sample(payload_len: usize, seed: u8) -> Vec<u8> {
    let mut sample = Vec::new();
    let payload = vec![seed; payload_len];
    write_obu(&mut sample, ObuType::Frame, None, &payload).unwrap();
    sample
}

fn benchmark_fragment_validation(c: &mut Criterion) {
    let init = make_init_with_video_sample_entry(1, *b"av01");
    let track_ids = extract_av1_track_ids_from_init(&init);

    let sample_counts = [1usize, 10, 100];
    let payload_sizes = [8usize, 128];
    let modes = [
        (
            "off",
            Av1ValidationOptions {
                enforce_should_not_obus: false,
                enforce_reserved_obus: false,
            },
        ),
        (
            "strict_should_not",
            Av1ValidationOptions {
                enforce_should_not_obus: true,
                enforce_reserved_obus: false,
            },
        ),
        (
            "strict_all",
            Av1ValidationOptions {
                enforce_should_not_obus: true,
                enforce_reserved_obus: true,
            },
        ),
    ];

    for sample_count in sample_counts {
        for payload_size in payload_sizes {
            let samples = (0..sample_count)
                .map(|i| build_av1_frame_sample(payload_size, i as u8))
                .collect::<Vec<_>>();
            let media = make_media_segment_for_track_samples(1, &samples);

            c.bench_function(
                &format!(
                    "fragment_validate_against_init/samples_{sample_count}/payload_{payload_size}"
                ),
                |b| {
                    b.iter(|| {
                        validate_av1_media_segment_against_init(
                            black_box(&init),
                            black_box(&media),
                            true,
                        )
                        .unwrap()
                    });
                },
            );

            for (mode_name, options) in modes {
                c.bench_function(
                    &format!(
                        "fragment_validate_with_track_ids/{mode_name}/samples_{sample_count}/payload_{payload_size}"
                    ),
                    |b| {
                        b.iter(|| {
                            validate_av1_media_segment_with_track_ids_and_options(
                                black_box(&media),
                                black_box(&track_ids),
                                options,
                            )
                            .unwrap()
                        });
                    },
                );
            }
        }
    }
}

criterion_group!(benches, benchmark_fragment_validation);
criterion_main!(benches);
