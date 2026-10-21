use std::time::Duration;

use super::*;
use crate::database::models::engine::FfmpegEngineConfig;
use crate::downloader::engine::{DownloadEngine, EngineType, FfmpegEngine};

#[tokio::test]
async fn chunked_output_names_add_suffixes_only_for_existing_files() {
    let directory = tempfile::tempdir().unwrap();
    let first = reserve_output_path(directory.path(), "recording", "mp4")
        .await
        .unwrap();
    assert_eq!(first, directory.path().join("recording.mp4"));
    tokio::fs::write(&first, b"existing recording")
        .await
        .unwrap();
    let occupied = directory.path().join("recording-001.mp4");
    tokio::fs::write(&occupied, b"another recording")
        .await
        .unwrap();

    let second = reserve_output_path(directory.path(), "recording", "mp4")
        .await
        .unwrap();
    let third = reserve_output_path(directory.path(), "recording", "mp4")
        .await
        .unwrap();
    assert_eq!(second, directory.path().join("recording-002.mp4"));
    assert_eq!(third, directory.path().join("recording-003.mp4"));
    assert_eq!(tokio::fs::read(first).await.unwrap(), b"existing recording");
    assert_eq!(
        tokio::fs::read(occupied).await.unwrap(),
        b"another recording"
    );
}

#[tokio::test]
async fn chunked_output_names_reserve_distinct_paths_for_concurrent_recordings() {
    let directory = tempfile::tempdir().unwrap();
    let barrier = Arc::new(tokio::sync::Barrier::new(8));
    let mut tasks = tokio::task::JoinSet::new();
    for index in 0u8..8 {
        let root = directory.path().to_path_buf();
        let barrier = barrier.clone();
        tasks.spawn(async move {
            barrier.wait().await;
            let path = reserve_output_path(&root, "同名.recording", "mkv")
                .await
                .unwrap();
            tokio::fs::write(&path, [index]).await.unwrap();
            (path, index)
        });
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut paths = std::collections::HashSet::new();
        while let Some(result) = tasks.join_next().await {
            let (path, index) = result.unwrap();
            assert_eq!(tokio::fs::read(&path).await.unwrap(), [index]);
            assert!(paths.insert(path));
        }
        assert_eq!(paths.len(), 8);
        assert!(paths.contains(&directory.path().join("同名.recording.mkv")));
        for suffix in 1..8 {
            assert!(
                paths.contains(
                    &directory
                        .path()
                        .join(format!("同名.recording-{suffix:03}.mkv"))
                )
            );
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn chunked_output_names_preserve_sequence_templates_in_events_and_recovery() {
    let directory = tempfile::tempdir().unwrap();
    let config = DownloadConfig::new(
        "fixture",
        directory.path(),
        "streamer",
        "Streamer",
        "session",
    )
    .with_filename_template("recording.%i")
    .with_output_format("mp4")
    .with_initial_segment_index(7);
    let (events, mut receiver) = mpsc::channel(8);
    let handle = DownloadHandle::new(
        "attempt-a",
        EngineType::Ffmpeg,
        config.clone(),
        events.clone(),
    );
    let staging = directory.path().join(".srec-chunks-attempt-a");
    tokio::fs::create_dir(&staging).await.unwrap();
    let first = Group::new(&handle, &staging, 0, Utc::now()).await.unwrap();
    let second = Group::new(&handle, &staging, 1, Utc::now()).await.unwrap();
    assert_eq!(first.path, directory.path().join("recording.007.mp4"));
    assert_eq!(second.path, directory.path().join("recording.008.mp4"));

    let other = DownloadHandle::new("attempt-b", EngineType::Ffmpeg, config, events);
    let other_staging = directory.path().join(".srec-chunks-attempt-b");
    tokio::fs::create_dir(&other_staging).await.unwrap();
    let collision = Group::new(&other, &other_staging, 0, Utc::now())
        .await
        .unwrap();
    assert_eq!(
        collision.path,
        directory.path().join("recording.007-001.mp4")
    );
    for group in [first, second, collision] {
        let SegmentEvent::SegmentStarted { path, sequence, .. } = receiver.recv().await.unwrap()
        else {
            panic!("expected segment start");
        };
        assert_eq!(path, group.path);
        assert_eq!(sequence, group.index);
        let metadata: serde_json::Value =
            serde_json::from_slice(&tokio::fs::read(&group.recovery).await.unwrap()).unwrap();
        assert_eq!(
            metadata["output_path"],
            serde_json::to_value(&group.path).unwrap()
        );
        assert_eq!(metadata["segment_index"], 7 + group.index);
    }
}

#[tokio::test]
async fn chunked_output_names_report_io_errors_without_retrying_as_collisions() {
    let directory = tempfile::tempdir().unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        reserve_output_path(&directory.path().join("missing"), "recording", "mp4"),
    )
    .await
    .unwrap();
    let error = result.unwrap_err();
    assert_eq!(error.kind, DownloadFailureKind::Io);
    assert!(error.message.contains("reserve recording file"));
}

async fn command(binary: &str, args: &[&str]) -> Vec<u8> {
    let output = tokio::time::timeout(
        Duration::from_secs(30),
        process_utils::tokio_command(binary).args(args).output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

async fn write_native<W: pipeline_common::ProtocolWriter>(mut writer: W, items: Vec<W::Item>) {
    let (sender, receiver) = mpsc::channel(32);
    let task = tokio::task::spawn_blocking(move || writer.run(receiver.into()));
    for item in items {
        sender.send(Ok(item)).await.unwrap();
    }
    drop(sender);
    tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
#[ignore = "requires local FFmpeg with libx264; uses generated local media"]
async fn real_media_manual_cut_native_pipelines_preserve_samples() {
    use pipeline_common::{PipelineError, PipelineProvider, StreamerContext};
    let binary = std::env::var("FFMPEG_PATH").unwrap_or_else(|_| "ffmpeg".into());
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.mp4");
    command(
        &binary,
        &[
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=160x90:rate=25",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000",
            "-t",
            "12",
            "-c:v",
            "libx264",
            "-g",
            "25",
            "-keyint_min",
            "25",
            "-sc_threshold",
            "0",
            "-bf",
            "2",
            "-c:a",
            "aac",
            source.to_str().unwrap(),
        ],
    )
    .await;
    let expected_video = hashes(&binary, &source, MediaHash::VideoFrames).await;
    let expected_audio = hashes(&binary, &source, MediaHash::AudioPackets).await;
    for format in ["flv", "ts", "fmp4"] {
        let input_dir = directory.path().join(format!("input-{format}"));
        let output_dir = directory.path().join(format!("output-{format}"));
        tokio::fs::create_dir(&input_dir).await.unwrap();
        tokio::fs::create_dir(&output_dir).await.unwrap();
        let control = Arc::new(pipeline_common::ManualSplitControl::default());
        control.enable();
        let context = Arc::new(
            StreamerContext::new(tokio_util::sync::CancellationToken::new())
                .with_manual_split(control.clone()),
        );
        if format == "flv" {
            let input = input_dir.join("source.flv");
            command(
                &binary,
                &[
                    "-v",
                    "error",
                    "-i",
                    source.to_str().unwrap(),
                    "-c",
                    "copy",
                    input.to_str().unwrap(),
                ],
            )
            .await;
            let mut reader = std::io::Cursor::new(tokio::fs::read(input).await.unwrap());
            let header = flv::parser::FlvParser::parse_header(&mut reader).unwrap();
            let offset = u64::from(header.data_offset);
            let mut items = vec![flv::data::FlvData::Header(header)];
            flv::parser::FlvParser::parse_tags(
                &mut reader,
                |tag, _, _| items.push(flv::data::FlvData::Tag(tag.clone())),
                offset,
            )
            .unwrap();
            let mut requested = false;
            let input = items.into_iter().map(|item| {
                if !requested
                    && matches!(&item, flv::data::FlvData::Tag(tag) if tag.timestamp_ms >= 2000)
                {
                    control.request(Duration::from_secs(30)).unwrap();
                    requested = true;
                }
                Ok::<_, PipelineError>(item)
            });
            let mut output = Vec::new();
            flv_fix::FlvPipeline::with_config(context, &Default::default(), Default::default())
                .build_pipeline()
                .run(input, &mut |item: Result<_, PipelineError>| {
                    output.push(item.unwrap())
                })
                .unwrap();
            assert_eq!(
                output
                    .iter()
                    .filter(|item| matches!(
                        item,
                        flv::data::FlvData::Split(pipeline_common::SplitReason::Manual { .. })
                    ))
                    .count(),
                1
            );
            write_native(
                flv_fix::FlvWriter::new(flv_fix::FlvWriterConfig {
                    output_dir: output_dir.clone(),
                    base_name: "cut-%i".into(),
                }),
                output,
            )
            .await;
        } else {
            let playlist_path = input_dir.join("index.m3u8");
            let playlist_argument = playlist_path.to_string_lossy().replace('\\', "/");
            let segment_pattern = input_dir.join(if format == "ts" {
                "segment-%d.ts"
            } else {
                "segment-%d.m4s"
            });
            let mut args = vec![
                "-v",
                "error",
                "-i",
                source.to_str().unwrap(),
                "-c",
                "copy",
                "-f",
                "hls",
                "-hls_time",
                "2",
                "-hls_playlist_type",
                "vod",
                "-hls_segment_filename",
                segment_pattern.to_str().unwrap(),
            ];
            if format == "fmp4" {
                args.extend([
                    "-hls_segment_type",
                    "fmp4",
                    "-hls_fmp4_init_filename",
                    "init.mp4",
                ]);
            }
            args.push(&playlist_argument);
            command(&binary, &args).await;
            let playlist =
                m3u8_rs::parse_media_playlist_res(&tokio::fs::read(playlist_path).await.unwrap())
                    .unwrap();
            let mut items = Vec::new();
            if format == "fmp4" {
                items.push(hls::HlsData::mp4_init(
                    Default::default(),
                    tokio::fs::read(input_dir.join("init.mp4"))
                        .await
                        .unwrap()
                        .into(),
                ));
            }
            for segment in playlist.segments {
                let data = tokio::fs::read(input_dir.join(&segment.uri))
                    .await
                    .unwrap()
                    .into();
                items.push(if format == "ts" {
                    hls::HlsData::ts(segment, data)
                } else {
                    hls::HlsData::mp4_segment(segment, data)
                });
            }
            let input = items.into_iter().enumerate().map(|(index, item)| {
                if index == 3 {
                    control.request(Duration::from_secs(30)).unwrap();
                }
                Ok::<_, PipelineError>(item)
            });
            let mut output = Vec::new();
            hls_fix::HlsPipeline::with_config(context, &Default::default(), Default::default())
                .build_pipeline()
                .run(input, &mut |item: Result<_, PipelineError>| {
                    output.push(item.unwrap())
                })
                .unwrap();
            assert_eq!(
                output
                    .iter()
                    .filter(|item| matches!(
                        item,
                        hls::HlsData::EndMarker(Some(pipeline_common::SplitReason::Manual { .. }))
                    ))
                    .count(),
                1,
                "{format}"
            );
            write_native(
                hls_fix::HlsWriter::new(hls_fix::HlsWriterConfig {
                    output_dir: output_dir.clone(),
                    base_name: "cut-%i".into(),
                    extension: if format == "ts" { "ts" } else { "m4s" }.into(),
                    max_file_size: None,
                }),
                output,
            )
            .await;
        }
        let mut paths: Vec<_> = std::fs::read_dir(&output_dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        paths.sort();
        assert_eq!(paths.len(), 2, "{format}");
        let mut video = Vec::new();
        let mut audio = Vec::new();
        for path in &paths {
            video.extend(hashes(&binary, path, MediaHash::VideoFrames).await);
            audio.extend(hashes(&binary, path, MediaHash::AudioPackets).await);
        }
        assert!(
            video == expected_video,
            "video changed for native {format}: counts {} vs {}, first difference {:?}",
            video.len(),
            expected_video.len(),
            video.iter().zip(&expected_video).position(|(a, b)| a != b)
        );
        assert!(
            audio == expected_audio,
            "audio changed for native {format}: counts {} vs {}, first difference {:?}",
            audio.len(),
            expected_audio.len(),
            audio.iter().zip(&expected_audio).position(|(a, b)| a != b)
        );
    }
}

enum MediaHash {
    VideoFrames,
    VideoPackets,
    AudioPackets,
}

async fn hashes(binary: &str, path: &Path, kind: MediaHash) -> Vec<String> {
    let mut args = vec!["-v", "error", "-i", path.to_str().unwrap(), "-map"];
    args.extend_from_slice(match kind {
        MediaHash::VideoFrames => &["0:v", "-fps_mode", "passthrough", "-f", "framemd5", "-"],
        MediaHash::VideoPackets => &[
            "0:v",
            "-c",
            "copy",
            "-bsf:v",
            "h264_mp4toannexb",
            "-f",
            "framehash",
            "-",
        ],
        MediaHash::AudioPackets => &[
            "0:a",
            "-c",
            "copy",
            "-bsf:a",
            "aac_adtstoasc",
            "-f",
            "framehash",
            "-",
        ],
    });
    String::from_utf8(command(binary, &args).await)
        .unwrap()
        .lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
        .map(|line| line.split(',').nth(5).unwrap().trim().to_owned())
        .collect()
}

#[tokio::test]
#[ignore = "requires local FFmpeg with libx264; uses only generated local media"]
async fn real_ffmpeg_manual_cut_preserves_video_frames_and_audio_packets() {
    let binary = std::env::var("FFMPEG_PATH").unwrap_or_else(|_| "ffmpeg".into());
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.mp4");
    command(
        &binary,
        &[
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=160x90:rate=25",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000",
            "-t",
            "12",
            "-c:v",
            "libx264",
            "-g",
            "25",
            "-keyint_min",
            "25",
            "-sc_threshold",
            "0",
            "-bf",
            "2",
            "-c:a",
            "aac",
            source.to_str().unwrap(),
        ],
    )
    .await;
    let expected_video = hashes(&binary, &source, MediaHash::VideoFrames).await;
    let expected_audio = hashes(&binary, &source, MediaHash::AudioPackets).await;
    let direct_output = directory.path().join("direct");
    tokio::fs::create_dir(&direct_output).await.unwrap();
    let direct_engine = FfmpegEngine::with_config_async(FfmpegEngineConfig {
        binary_path: binary.clone(),
        ..Default::default()
    })
    .await;
    assert_direct_recording(
        &direct_engine,
        DownloadConfig::new(
            source.to_str().unwrap(),
            &direct_output,
            "streamer",
            "Streamer",
            "session",
        )
        .with_output_format("mp4"),
    )
    .await;
    for mode in ["mp4", "mkv", "flv", "ts", "mov", "stop", "automatic"] {
        let format = if matches!(mode, "stop" | "automatic") {
            "mp4"
        } else {
            mode
        };
        let expected_segments = if mode == "automatic" { 3 } else { 2 };
        let output = directory.path().join(mode);
        tokio::fs::create_dir(&output).await.unwrap();
        let unrelated_partial = output.join(format!("recording.partial.{format}"));
        tokio::fs::write(&unrelated_partial, b"existing file")
            .await
            .unwrap();
        let mut config = DownloadConfig::new(
            source.to_str().unwrap(),
            &output,
            "streamer",
            "Streamer",
            "session",
        )
        .with_output_format(format)
        .with_filename_template("recording");
        if mode == "automatic" {
            config.max_segment_duration_secs = 4;
        }
        let engine = FfmpegEngine::with_config_async(FfmpegEngineConfig {
            enable_lossless_cutting: true,
            binary_path: binary.clone(),
            input_args: vec![
                "-readrate".into(),
                if mode == "mkv" { "1" } else { "8" }.into(),
            ],
            ..Default::default()
        })
        .await;
        let (events, mut receiver) = mpsc::channel(64);
        let handle = Arc::new(DownloadHandle::new(
            mode,
            EngineType::Ffmpeg,
            config,
            events,
        ));
        let recording = handle.clone();
        let task = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
            engine.run(recording).await
        }));
        let mut requested = false;
        let mut files = Vec::new();
        let mut starts = Vec::new();
        let mut live_rates = Vec::new();
        let mut tick = tokio::time::interval(Duration::from_millis(10));
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                tokio::select! {
                    _ = tick.tick(), if !requested => {
                        if handle.manual_split.snapshot().supported {
                            handle.manual_split.request(Duration::from_secs(30)).unwrap();
                            requested = true;
                        }
                    }
                    event = receiver.recv() => match event.unwrap() {
                        SegmentEvent::SegmentStarted { path, sequence, .. } => { starts.push((path, sequence)); if mode == "stop" && sequence == 1 { handle.cancel(); } }
                        SegmentEvent::SegmentCompleted(info) => files.push(info),
                        SegmentEvent::Progress(progress) if mode == "mkv" && progress.duration_secs >= 5.0 => live_rates.push(progress.speed_bytes_per_sec),
                        SegmentEvent::DownloadCompleted { total_segments, .. } => { assert_eq!(total_segments, expected_segments); break; }
                        SegmentEvent::DownloadFailed { message, .. } => panic!("{message}"),
                        _ => {}
                    }
                }
            }
        }).await.unwrap();
        task.await.unwrap().unwrap();
        if mode == "mkv" {
            // Real-time acquisition must not report zero speed between buffered
            // chunk writes once the measurement window has filled.
            assert!(live_rates.len() >= 5, "missing real-time progress samples");
            assert!(live_rates.iter().all(|rate| *rate > 0), "{live_rates:?}");
        }
        assert!(requested);
        assert_eq!(starts.len(), expected_segments as usize);
        assert_eq!(files.len(), expected_segments as usize);
        assert_eq!(files[0].split_reason_code.as_deref(), Some("manual"));
        assert_eq!(starts[0].1, 0);
        assert_eq!(starts[1].1, 1);
        for (index, file) in files.iter().enumerate() {
            let expected_name = if index == 0 {
                format!("recording.{format}")
            } else {
                format!("recording-{index:03}.{format}")
            };
            assert_eq!(file.path, output.join(expected_name));
            assert_eq!(starts[index].0, file.path);
        }
        assert_eq!(
            tokio::fs::read(&unrelated_partial).await.unwrap(),
            b"existing file"
        );
        let mut video = Vec::new();
        let mut audio = Vec::new();
        for file in &files {
            video.extend(hashes(&binary, &file.path, MediaHash::VideoFrames).await);
            audio.extend(hashes(&binary, &file.path, MediaHash::AudioPackets).await);
        }
        if mode == "stop" {
            assert!(video.len() < expected_video.len());
            // A graceful stop can receive a future reference frame before its
            // intervening B-frames. Decoded output is then an ordered subset,
            // while encoded packets must still be an exact prefix of the source.
            let mut remaining = expected_video.iter();
            assert!(
                video
                    .iter()
                    .all(|frame| remaining.any(|item| item == frame))
            );
            let reference_packets = hashes(&binary, &source, MediaHash::VideoPackets).await;
            let mut packets = Vec::new();
            for file in &files {
                packets.extend(hashes(&binary, &file.path, MediaHash::VideoPackets).await);
            }
            assert!(!packets.is_empty());
            assert!(
                reference_packets.starts_with(&packets),
                "video packets changed before stop"
            );
        } else {
            assert!(video == expected_video, "video changed for {mode}");
        }
        let audio_reference = if mode == "stop" {
            &expected_audio[..audio.len()]
        } else {
            &expected_audio[..]
        };
        assert!(
            audio == audio_reference,
            "audio changed for {format}: counts {} vs {}, first difference {:?}",
            audio.len(),
            expected_audio.len(),
            audio.iter().zip(&expected_audio).position(|(a, b)| a != b)
        );
        assert!(!output.join(format!(".srec-chunks-{mode}")).exists());
    }
}

#[tokio::test]
async fn manual_cut_chunk_list_rejects_paths_outside_the_staging_directory() {
    let directory = tempfile::tempdir().unwrap();
    tokio::fs::write(directory.path().join(LIST_NAME), "../recording.mkv,0,2\n")
        .await
        .unwrap();
    assert!(
        ChunkList::default()
            .next(directory.path(), Path::new("../recording.mkv"))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn manual_cut_chunk_list_reads_only_complete_ordered_records() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(LIST_NAME);
    tokio::fs::write(
        &path,
        "chunk-00000000.mkv,0,2.08\nchunk-00000001.mkv,2.08,4.08\n",
    )
    .await
    .unwrap();
    let mut list = ChunkList::default();
    let first = list
        .next(directory.path(), Path::new("chunk-00000000.mkv"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!((first.start, first.end), (0.0, 2.08));
    let second = list
        .next(directory.path(), Path::new("chunk-00000001.mkv"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!((second.start, second.end), (2.08, 4.08));
    // A partially written record is not consumed until its newline arrives.
    tokio::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .await
        .unwrap()
        .write_all(b"chunk-00000002.mkv,4.08")
        .await
        .unwrap();
    assert!(
        list.next(directory.path(), Path::new("chunk-00000002.mkv"))
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn manual_cut_finalizer_failure_stops_acquisition_and_keeps_recovery_files() {
    let directory = tempfile::tempdir().unwrap();
    let config = DownloadConfig::new(
        "fixture",
        directory.path(),
        "streamer",
        "Streamer",
        "session",
    )
    .with_output_format("mp4");
    let (sender, mut events) = mpsc::channel(32);
    let handle = Arc::new(DownloadHandle::new(
        "recovery",
        EngineType::Ffmpeg,
        config,
        sender,
    ));
    let result = tokio::time::timeout(
        Duration::from_secs(3),
        run(
            handle.clone(),
            "missing-recording-finalizer",
            |inner| async move {
                let dir = inner.config_snapshot().output_dir;
                let path = dir.join("chunk-00000000.mkv");
                tokio::fs::write(&path, b"retained media fixture")
                    .await
                    .unwrap();
                tokio::fs::write(dir.join(LIST_NAME), "chunk-00000000.mkv,0,2\n")
                    .await
                    .unwrap();
                send(
                    &inner,
                    SegmentEvent::SegmentStarted {
                        path: path.clone(),
                        sequence: 0,
                        started_at: Utc::now(),
                    },
                )
                .await?;
                inner.manual_split.enable();
                inner.manual_split.request(Duration::from_secs(30)).unwrap();
                send(
                    &inner,
                    SegmentEvent::SegmentCompleted(SegmentInfo {
                        path,
                        index: 0,
                        duration_secs: 2.0,
                        size_bytes: 22,
                        started_at: Some(Utc::now()),
                        completed_at: Utc::now(),
                        split_reason_code: None,
                        split_reason_details_json: None,
                    }),
                )
                .await?;
                send(
                    &inner,
                    SegmentEvent::SegmentStarted {
                        path: dir.join("chunk-00000001.mkv"),
                        sequence: 1,
                        started_at: Utc::now(),
                    },
                )
                .await?;
                inner.cancellation_token.cancelled().await;
                Ok(())
            },
        ),
    )
    .await
    .unwrap();
    assert!(result.is_err());
    assert!(handle.is_cancelled());
    let staging = directory.path().join(".srec-chunks-recovery");
    assert_eq!(
        tokio::fs::read(staging.join("chunk-00000000.mkv"))
            .await
            .unwrap(),
        b"retained media fixture"
    );
    assert!(staging.join("group-0.ffconcat").exists());
    assert!(staging.join("group-0.json").exists());
    let mut reserved = None;
    while let Ok(event) = events.try_recv() {
        assert!(!matches!(
            event,
            SegmentEvent::SegmentCompleted(_) | SegmentEvent::DownloadCompleted { .. }
        ));
        if let SegmentEvent::SegmentStarted {
            path, sequence: 0, ..
        } = event
        {
            reserved = Some(path);
        }
    }
    assert!(
        !reserved.unwrap().exists(),
        "unused reservation must be released"
    );
}

#[tokio::test]
#[ignore = "requires local Streamlink and FFmpeg; serves generated HLS on loopback"]
async fn real_streamlink_manual_cut_preserves_media_without_restarting_acquisition() {
    use crate::database::models::engine::StreamlinkEngineConfig;
    use crate::downloader::engine::StreamlinkEngine;
    use axum::{
        Router, extract::Path as RoutePath, http::StatusCode, response::IntoResponse, routing::get,
    };

    let binary = std::env::var("FFMPEG_PATH").unwrap_or_else(|_| "ffmpeg".into());
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.mp4");
    command(
        &binary,
        &[
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=640x360:rate=25",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000",
            "-t",
            "12",
            "-c:v",
            "libx264",
            "-b:v",
            "8M",
            "-minrate",
            "8M",
            "-maxrate",
            "8M",
            "-bufsize",
            "8M",
            "-x264-params",
            "nal-hrd=cbr",
            "-g",
            "25",
            "-keyint_min",
            "25",
            "-sc_threshold",
            "0",
            "-bf",
            "2",
            "-c:a",
            "aac",
            source.to_str().unwrap(),
        ],
    )
    .await;
    let playlist = directory
        .path()
        .join("index.m3u8")
        .to_string_lossy()
        .replace('\\', "/");
    let pattern = directory
        .path()
        .join("segment-%d.ts")
        .to_string_lossy()
        .replace('\\', "/");
    command(
        &binary,
        &[
            "-v",
            "error",
            "-i",
            source.to_str().unwrap(),
            "-c",
            "copy",
            "-f",
            "hls",
            "-hls_time",
            "2",
            "-hls_playlist_type",
            "vod",
            "-hls_segment_filename",
            &pattern,
            &playlist,
        ],
    )
    .await;
    let mut data = std::collections::HashMap::new();
    for entry in std::fs::read_dir(directory.path()).unwrap() {
        let path = entry.unwrap().path();
        if path
            .extension()
            .is_some_and(|ext| ext == "ts" || ext == "m3u8")
        {
            data.insert(
                path.file_name().unwrap().to_string_lossy().into_owned(),
                bytes::Bytes::from(tokio::fs::read(path).await.unwrap()),
            );
        }
    }
    let data = Arc::new(data);
    let (release, gate) = tokio::sync::watch::channel(false);
    let requests = Arc::new(std::sync::Mutex::new(std::collections::HashMap::<
        String,
        usize,
    >::new()));
    let observed = requests.clone();
    let app = Router::new().route(
        "/{file}",
        get(move |RoutePath(file): RoutePath<String>| {
            let data = data.clone();
            let mut gate = gate.clone();
            let requests = observed.clone();
            async move {
                *requests.lock().unwrap().entry(file.clone()).or_default() += 1;
                if file
                    .strip_prefix("segment-")
                    .and_then(|name| name.strip_suffix(".ts"))
                    .and_then(|index| index.parse::<u32>().ok())
                    .is_some_and(|index| index >= 4)
                {
                    while !*gate.borrow_and_update() {
                        if gate.changed().await.is_err() {
                            break;
                        }
                    }
                }
                match data.get(&file) {
                    Some(bytes) => (
                        [(
                            "content-type",
                            if file.ends_with("m3u8") {
                                "application/vnd.apple.mpegurl"
                            } else {
                                "video/MP2T"
                            },
                        )],
                        bytes.clone(),
                    )
                        .into_response(),
                    None => StatusCode::NOT_FOUND.into_response(),
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let _server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    }));
    let output_dir = directory.path().join("output");
    tokio::fs::create_dir(&output_dir).await.unwrap();
    let config = DownloadConfig::new(
        format!("hls://http://{address}/index.m3u8"),
        &output_dir,
        "streamer",
        "Streamer",
        "session",
    )
    .with_output_format("mp4");
    let direct_output = directory.path().join("direct");
    tokio::fs::create_dir(&direct_output).await.unwrap();
    let direct_engine = StreamlinkEngine::with_config_async(StreamlinkEngineConfig {
        ffmpeg_path: Some(binary.clone()),
        ..Default::default()
    })
    .await;
    let mut direct_config = config.clone();
    direct_config.output_dir = direct_output;
    release.send_replace(true);
    assert_direct_recording(&direct_engine, direct_config).await;
    requests.lock().unwrap().clear();
    release.send_replace(false);
    let engine = StreamlinkEngine::with_config_async(StreamlinkEngineConfig {
        enable_lossless_cutting: true,
        ffmpeg_path: Some(binary.clone()),
        ..Default::default()
    })
    .await;
    let (sender, mut events) = mpsc::channel(64);
    let handle = Arc::new(DownloadHandle::new(
        "streamlink-cut",
        EngineType::Streamlink,
        config,
        sender,
    ));
    let recording = handle.clone();
    let mut task = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
        engine.run(recording).await
    }));
    let mut producer_finished = false;
    let mut requested = false;
    let mut files = Vec::new();
    let mut tick = tokio::time::interval(Duration::from_millis(10));
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            tokio::select! {
                result = &mut task, if !producer_finished => { result.unwrap().unwrap(); producer_finished = true; },
                _ = tick.tick(), if !requested => {
                    if handle.manual_split.snapshot().supported {
                        handle.manual_split.request(Duration::from_secs(30)).unwrap();
                        requested = true;
                        release.send_replace(true);
                    }
                }
                event = events.recv() => match event.unwrap() {
                    SegmentEvent::SegmentCompleted(info) => files.push(info),
                    SegmentEvent::DownloadCompleted { .. } => break,
                    SegmentEvent::DownloadFailed { message, .. } => panic!("{message}"),
                    _ => {}
                }
            }
        }
    }).await.unwrap();
    if !producer_finished {
        task.await.unwrap().unwrap();
    }
    assert!(requested);
    assert_eq!(files.len(), 2);
    let mut video = Vec::new();
    let mut audio = Vec::new();
    for file in files {
        video.extend(hashes(&binary, &file.path, MediaHash::VideoFrames).await);
        audio.extend(hashes(&binary, &file.path, MediaHash::AudioPackets).await);
    }
    assert!(video == hashes(&binary, &source, MediaHash::VideoFrames).await);
    assert!(audio == hashes(&binary, &source, MediaHash::AudioPackets).await);
    assert!(
        requests
            .lock()
            .unwrap()
            .iter()
            .filter(|(path, _)| path.ends_with(".ts"))
            .all(|(_, count)| *count == 1)
    );
}

async fn assert_direct_recording(engine: &dyn DownloadEngine, config: DownloadConfig) {
    let output = config.output_dir.clone();
    let (sender, mut events) = mpsc::channel(64);
    let handle = Arc::new(DownloadHandle::new(
        "direct-recording",
        engine.engine_type(),
        config,
        sender,
    ));
    // The handle keeps a sender alive, so the event stream never closes by
    // itself: an engine error that sends no terminal event must end the wait.
    let files = tokio::time::timeout(Duration::from_secs(30), async {
        let run = engine.run(handle.clone());
        tokio::pin!(run);
        let mut finished = false;
        let mut files = Vec::new();
        loop {
            tokio::select! {
                result = &mut run, if !finished => {
                    result.unwrap();
                    finished = true;
                }
                event = events.recv() => {
                    assert!(!handle.manual_split.snapshot().supported);
                    match event.unwrap() {
                        SegmentEvent::SegmentCompleted(info) => files.push(info.path),
                        SegmentEvent::DownloadCompleted { .. } => break,
                        SegmentEvent::DownloadFailed { message, .. } => panic!("{message}"),
                        _ => {}
                    }
                }
            }
        }
        if !finished {
            run.await.unwrap();
        }
        files
    })
    .await
    .unwrap();
    assert_eq!(files.len(), 1);
    assert!(tokio::fs::metadata(&files[0]).await.unwrap().len() > 0);
    assert!(
        handle
            .manual_split
            .request(Duration::from_secs(30))
            .is_err()
    );
    let mut entries = tokio::fs::read_dir(output).await.unwrap();
    while let Some(entry) = entries.next_entry().await.unwrap() {
        assert!(
            !entry
                .file_name()
                .to_string_lossy()
                .starts_with(".srec-chunks-")
        );
    }
}

#[test]
fn chunk_pattern_escapes_percent_in_the_output_directory() {
    let mut config = DownloadConfig::new(
        "fixture",
        "root/50%d/.srec-chunks-attempt",
        "streamer",
        "Streamer",
        "session",
    );
    config.output_format = "mkv".into();
    let mut args = vec![
        "-f".to_string(),
        "segment".to_string(),
        "output".to_string(),
    ];
    configure_args(&mut args, &config);
    assert_eq!(
        args.last().unwrap(),
        "root/50%%d/.srec-chunks-attempt/chunk-%08d.mkv"
    );
    // The list path is opened verbatim, not expanded as a pattern.
    let list = args.iter().position(|arg| arg == "-segment_list").unwrap();
    assert_eq!(args[list + 1], "root/50%d/.srec-chunks-attempt/chunks.csv");
}

#[tokio::test]
async fn unpublished_group_releases_only_an_empty_reservation() {
    let directory = tempfile::tempdir().unwrap();
    let config = DownloadConfig::new(
        "fixture",
        directory.path(),
        "streamer",
        "Streamer",
        "session",
    )
    .with_filename_template("recording")
    .with_output_format("mkv");
    let (events, _receiver) = mpsc::channel(8);
    let handle = DownloadHandle::new("attempt", EngineType::Ffmpeg, config, events);
    let staging = directory.path().join(".srec-chunks-attempt");
    tokio::fs::create_dir(&staging).await.unwrap();

    let empty = Group::new(&handle, &staging, 0, Utc::now()).await.unwrap();
    let empty_path = empty.path.clone();
    assert!(empty_path.exists());
    drop(empty);
    assert!(!empty_path.exists());

    let replaced = Group::new(&handle, &staging, 1, Utc::now()).await.unwrap();
    let replaced_path = replaced.path.clone();
    tokio::fs::write(&replaced_path, b"someone else's media")
        .await
        .unwrap();
    drop(replaced);
    assert_eq!(
        tokio::fs::read(&replaced_path).await.unwrap(),
        b"someone else's media"
    );
}

fn chunk_info(path: PathBuf, index: u32) -> SegmentInfo {
    SegmentInfo {
        path,
        index,
        duration_secs: 2.0,
        size_bytes: 10,
        started_at: Some(Utc::now()),
        completed_at: Utc::now(),
        split_reason_code: None,
        split_reason_details_json: None,
    }
}

/// Writes `count` chunk files, timing records for the first `recorded`, and
/// the events a segment producer emits for them.
///
/// Records are appended like FFmpeg's segment muxer does with
/// `-segment_list_size 0`: it never rewrites the list, and flushes each record
/// before opening the next chunk. The producer runs ahead of ingestion, so
/// rewriting the list here would let ingestion read it mid-truncation.
async fn produce_chunks(
    inner: &DownloadHandle,
    count: u32,
    recorded: u32,
) -> Result<(), EngineStartError> {
    let dir = inner.config_snapshot().output_dir;
    let mut list = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(LIST_NAME))
        .await
        .unwrap();
    for index in 0..count {
        let name = format!("chunk-{index:08}.mkv");
        let path = dir.join(&name);
        tokio::fs::write(&path, b"chunk data").await.unwrap();
        if index < recorded {
            let start = f64::from(index) * 2.0;
            list.write_all(format!("{name},{start},{}\n", start + 2.0).as_bytes())
                .await
                .unwrap();
            list.flush().await.unwrap();
        }
        send(
            inner,
            SegmentEvent::SegmentStarted {
                path: path.clone(),
                sequence: index,
                started_at: Utc::now(),
            },
        )
        .await?;
        send(
            inner,
            SegmentEvent::SegmentCompleted(chunk_info(path, index)),
        )
        .await?;
    }
    Ok(())
}

fn fixture_handle(
    directory: &Path,
    id: &str,
) -> (Arc<DownloadHandle>, mpsc::Receiver<SegmentEvent>) {
    let config = DownloadConfig::new("fixture", directory, "streamer", "Streamer", "session")
        .with_filename_template("recording")
        .with_output_format("mkv");
    let (sender, events) = mpsc::channel(64);
    let handle = Arc::new(DownloadHandle::new(id, EngineType::Ffmpeg, config, sender));
    (handle, events)
}

#[cfg(unix)]
fn drain(events: &mut mpsc::Receiver<SegmentEvent>) -> Vec<SegmentEvent> {
    let mut drained = Vec::new();
    while let Ok(event) = events.try_recv() {
        drained.push(event);
    }
    drained
}

/// A finalizer that writes the output named by its last argument after `delay`.
#[cfg(unix)]
fn fake_finalizer(directory: &Path, delay: &str) -> String {
    crate::downloader::engine::utils::test_support::script(
        directory,
        "finalizer",
        &format!("sleep {delay}\nfor last; do :; done\nprintf 'final media' > \"$last\"\n"),
    )
}

#[cfg(unix)]
#[tokio::test]
async fn crashed_producer_still_finalizes_recorded_chunks_and_keeps_its_failure_kind() {
    let tools = tempfile::tempdir().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let finalizer = fake_finalizer(tools.path(), "0");
    let (handle, mut events) = fixture_handle(directory.path(), "crash");
    tokio::time::timeout(
        Duration::from_secs(10),
        run(handle.clone(), &finalizer, |inner| async move {
            // A killed producer never records its last, unclosed chunk.
            produce_chunks(&inner, 3, 2).await?;
            send(
                &inner,
                SegmentEvent::DownloadFailed {
                    kind: DownloadFailureKind::ProcessExit { code: Some(255) },
                    message: "ffmpeg exited with code 255".into(),
                },
            )
            .await
        }),
    )
    .await
    .unwrap()
    .unwrap();

    let events = drain(&mut events);
    let completed: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            SegmentEvent::SegmentCompleted(info) => Some(info),
            _ => None,
        })
        .collect();
    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0].duration_secs, 4.0);
    assert_eq!(
        tokio::fs::read(&completed[0].path).await.unwrap(),
        b"final media"
    );
    assert!(matches!(
        events.last(),
        Some(SegmentEvent::DownloadFailed {
            kind: DownloadFailureKind::ProcessExit { code: Some(255) },
            ..
        })
    ));
    assert!(DownloadFailureKind::ProcessExit { code: Some(255) }.is_recoverable());
    let staging = directory.path().join(".srec-chunks-crash");
    assert!(staging.join("chunk-00000002.mkv").exists());
    assert!(!staging.join("chunk-00000000.mkv").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn stopping_waits_for_finalization_of_captured_media() {
    let tools = tempfile::tempdir().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let finalizer = fake_finalizer(tools.path(), "1");
    let (handle, mut events) = fixture_handle(directory.path(), "stop");
    let stopper = handle.clone();
    tokio::time::timeout(
        Duration::from_secs(10),
        run(handle.clone(), &finalizer, |inner| async move {
            produce_chunks(&inner, 1, 1).await?;
            stopper.cancel();
            send(
                &inner,
                SegmentEvent::DownloadCompleted {
                    total_bytes: 10,
                    total_duration_secs: 2.0,
                    total_segments: 1,
                    engine_signal: crate::downloader::EngineEndSignal::SubprocessExitZero,
                },
            )
            .await
        }),
    )
    .await
    .unwrap()
    .unwrap();

    let events = drain(&mut events);
    let Some(SegmentEvent::SegmentCompleted(info)) = events
        .iter()
        .find(|event| matches!(event, SegmentEvent::SegmentCompleted(_)))
    else {
        panic!("expected a finalized recording");
    };
    assert_eq!(tokio::fs::read(&info.path).await.unwrap(), b"final media");
    assert!(matches!(
        events.last(),
        Some(SegmentEvent::DownloadCompleted {
            total_segments: 1,
            ..
        })
    ));
    assert!(!directory.path().join(".srec-chunks-stop").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn shutdown_deadline_abandons_finalization_but_keeps_recovery_files() {
    let tools = tempfile::tempdir().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let finalizer = fake_finalizer(tools.path(), "30");
    let (handle, mut events) = fixture_handle(directory.path(), "shutdown");
    let stopper = handle.clone();
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        run(handle.clone(), &finalizer, |inner| async move {
            produce_chunks(&inner, 1, 1).await?;
            stopper.set_stop_deadline(tokio::time::Instant::now() + Duration::from_millis(200));
            stopper.cancel();
            send(
                &inner,
                SegmentEvent::DownloadCompleted {
                    total_bytes: 10,
                    total_duration_secs: 2.0,
                    total_segments: 1,
                    engine_signal: crate::downloader::EngineEndSignal::SubprocessExitZero,
                },
            )
            .await
        }),
    )
    .await
    .unwrap();

    assert!(result.unwrap_err().message.contains("shutdown deadline"));
    let events = drain(&mut events);
    let Some(SegmentEvent::SegmentStarted { path, .. }) = events.first() else {
        panic!("expected the recording file to start");
    };
    assert!(!path.exists(), "unused reservation must be released");
    assert!(!events.iter().any(|event| matches!(
        event,
        SegmentEvent::SegmentCompleted(_) | SegmentEvent::DownloadCompleted { .. }
    )));
    let staging = directory.path().join(".srec-chunks-shutdown");
    assert!(staging.join("chunk-00000000.mkv").exists());
    assert!(staging.join("group-0.ffconcat").exists());
    assert!(staging.join("group-0.json").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn slow_finalization_queues_groups_without_stopping_acquisition() {
    let tools = tempfile::tempdir().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let finalizer = fake_finalizer(tools.path(), "0.2");
    let (handle, mut events) = fixture_handle(directory.path(), "backlog");
    // Every 2 s chunk exceeds the limit, so each closes its own group and the
    // producer queues groups far faster than the finalizer publishes them.
    handle.update_config(|config| config.max_segment_duration_secs = 1);
    tokio::time::timeout(
        Duration::from_secs(20),
        run(handle.clone(), &finalizer, |inner| async move {
            produce_chunks(&inner, 6, 6).await?;
            send(
                &inner,
                SegmentEvent::DownloadCompleted {
                    total_bytes: 60,
                    total_duration_secs: 12.0,
                    total_segments: 6,
                    engine_signal: crate::downloader::EngineEndSignal::SubprocessExitZero,
                },
            )
            .await
        }),
    )
    .await
    .unwrap()
    .unwrap();

    assert!(!handle.is_cancelled());
    let events = drain(&mut events);
    let completed: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            SegmentEvent::SegmentCompleted(info) => Some(info),
            _ => None,
        })
        .collect();
    assert_eq!(completed.len(), 6);
    for info in &completed {
        assert_eq!(tokio::fs::read(&info.path).await.unwrap(), b"final media");
    }
    assert!(matches!(
        events.last(),
        Some(SegmentEvent::DownloadCompleted {
            total_segments: 6,
            ..
        })
    ));
}

/// Runs ingestion over two chunks with a manual request waiting and
/// `backlog` groups already queued, returning the first closed group.
async fn first_group_with_backlog(backlog: usize) -> (Group, pipeline_common::ManualSplitSnapshot) {
    let directory = tempfile::tempdir().unwrap();
    let (handle, _outer) = fixture_handle(directory.path(), "manual-backlog");
    handle.update_config(|config| config.max_segment_duration_secs = 1);
    let staging = directory.path().join("staging");
    tokio::fs::create_dir(&staging).await.unwrap();
    let (events_tx, events_rx) = mpsc::channel(32);
    let inner = handle.with_output(
        DownloadConfig::new("fixture", &staging, "streamer", "Streamer", "session"),
        events_tx,
    );
    let (jobs_tx, mut jobs_rx) = mpsc::unbounded_channel();
    let queued = AtomicUsize::new(backlog);
    let completed = AtomicU32::new(0);
    handle.manual_split.enable();
    // A backlogged request is never claimed, so it may expire immediately;
    // a claimable one must outlive the run.
    let timeout = if backlog >= MANUAL_CUT_BACKLOG {
        Duration::ZERO
    } else {
        Duration::from_secs(30)
    };
    handle.manual_split.request(timeout).unwrap();
    produce_chunks(&inner, 2, 2).await.unwrap();
    let ingest = ingest(&handle, &staging, events_rx, jobs_tx, &queued, &completed);
    tokio::pin!(ingest);
    let group = tokio::select! {
        group = jobs_rx.recv() => group.unwrap(),
        result = &mut ingest => panic!("ingestion ended early: {result:?}"),
    };
    // The group closed at the second chunk, so that boundary was examined.
    handle.manual_split.expire();
    (group, handle.manual_split.snapshot())
}

#[tokio::test]
async fn manual_cut_waits_while_finalization_is_backlogged() {
    let (group, split) = first_group_with_backlog(MANUAL_CUT_BACKLOG).await;
    assert_eq!(group.reason, Some("duration_limit"));
    assert_eq!(group.request_id, None);
    assert_eq!(split.status, pipeline_common::ManualSplitStatus::Expired);
    assert_eq!(
        split.expiry_reason,
        Some(pipeline_common::ManualSplitExpiryReason::FinalizationBacklog)
    );

    let (group, split) = first_group_with_backlog(MANUAL_CUT_BACKLOG - 1).await;
    assert_eq!(group.reason, Some("manual"));
    assert_eq!(group.request_id, Some(split.request_id));
}
