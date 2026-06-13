use futures::StreamExt;
use indicatif::ProgressStyle;
use mesio_engine::{DownloadEvent, DownloadEventStream, DownloadHandle};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use tracing::Span;
use tracing_indicatif::span_ext::IndicatifSpanExt;

use crate::utils::format_bytes;

/// Creates a progress bar style for download operations
pub fn download_progress_style() -> ProgressStyle {
    ProgressStyle::default_bar()
        .template("{span_child_prefix}{spinner:.green} {span_name} {msg}\n{span_child_prefix}[{elapsed_precise}] [{bar:40.green/white}] {bytes}/{total_bytes} @ {bytes_per_sec}")
        .unwrap()
        .progress_chars("=> ")
}

/// Creates a progress style for streaming HLS downloads without a known total.
pub fn hls_download_progress_style() -> ProgressStyle {
    ProgressStyle::default_bar()
        .template("{span_child_prefix}{spinner:.green} {span_name} {msg}\n{span_child_prefix}[{elapsed_precise}] {bytes} @ {bytes_per_sec}")
        .unwrap()
        .progress_chars("=> ")
}

/// Creates a progress bar style for processing operations
pub fn processing_progress_style() -> ProgressStyle {
    ProgressStyle::default_bar()
        .template("{span_child_prefix}{spinner:.cyan} {span_name} {msg}\n{span_child_prefix}[{elapsed_precise}] [{bar:40.cyan/white}] {pos}/{len} items")
        .unwrap()
        .progress_chars("=> ")
}

/// Creates a progress bar style for writing operations
pub fn writing_progress_style() -> ProgressStyle {
    ProgressStyle::default_bar()
        .template("{span_child_prefix}{spinner:.blue} {span_name} {msg}\n{span_child_prefix}[{elapsed_precise}] [{bar:40.blue/white}] {bytes}/{total_bytes}")
        .unwrap()
        .progress_chars("=> ")
}

/// Initialize a span with a download progress bar
pub fn init_download_span(span: &Span, message: impl Into<String>) {
    span.pb_set_style(&download_progress_style());
    let msg = message.into();
    span.pb_set_message(&msg);
}

/// Initialize a span with HLS stream progress.
pub fn init_hls_download_span(span: &Span, message: impl Into<String>) {
    span.pb_set_style(&hls_download_progress_style());
    let msg = message.into();
    span.pb_set_message(&msg);
}

/// Initialize a span with a processing progress bar
pub fn init_processing_span(span: &Span, message: impl Into<String>) {
    span.pb_set_style(&processing_progress_style());
    let msg = message.into();
    span.pb_set_message(&msg);
}

/// Initialize a span with a writing progress bar
pub fn init_writing_span(span: &Span, message: impl Into<String>) {
    span.pb_set_style(&writing_progress_style());
    let msg = message.into();
    span.pb_set_message(&msg);
}

pub async fn render_download_events(mut events: DownloadEventStream, download_span: Span) {
    let total_bytes = Arc::new(AtomicU64::new(0));
    while let Some(event) = events.next().await {
        match event {
            DownloadEvent::ResourceStarted {
                content_length: Some(length),
                ..
            } => {
                download_span.pb_set_length(length);
            }
            DownloadEvent::Progress { bytes_delta, .. } => {
                let total = total_bytes.fetch_add(bytes_delta, Ordering::Relaxed) + bytes_delta;
                download_span.pb_set_position(total);
                download_span.pb_set_message(&format!("Downloaded {}", format_bytes(total)));
            }
            DownloadEvent::ResourceFinished {
                bytes, from_cache, ..
            } if from_cache => {
                let total = total_bytes.fetch_add(bytes, Ordering::Relaxed) + bytes;
                download_span.pb_set_position(total);
                download_span.pb_set_message(&format!("Downloaded {}", format_bytes(total)));
            }
            DownloadEvent::Lagged { dropped } => {
                download_span.pb_set_message(&format!("Dropped {} progress events", dropped));
            }
            _ => {}
        }
    }
}

pub fn summarize_dropped_events(handle: &DownloadHandle, download_span: &Span) {
    let dropped = handle.dropped_events();
    if dropped == 0 {
        return;
    }

    download_span.pb_set_message(&format!("Dropped {} progress events", dropped));
    tracing::warn!(dropped_events = dropped, "download progress events dropped");
}
