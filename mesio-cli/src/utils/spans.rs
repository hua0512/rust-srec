use indicatif::ProgressStyle;
use tracing::Span;
use tracing_indicatif::span_ext::IndicatifSpanExt;

/// Creates a progress bar style for download operations
pub fn download_progress_style() -> ProgressStyle {
    ProgressStyle::default_bar()
        .template("{span_child_prefix}{spinner:.green} {span_name} {msg}\n{span_child_prefix}[{elapsed_precise}] [{bar:40.green/white}] {bytes}/{total_bytes} @ {bytes_per_sec}")
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

/// Creates a spinner style for operations without known progress
#[allow(dead_code)]
pub fn spinner_style() -> ProgressStyle {
    ProgressStyle::default_spinner()
        .template("{span_child_prefix}{spinner:.green} {span_name} {msg} [{elapsed_precise}]")
        .unwrap()
}

/// Initialize a span with a download progress bar
pub fn init_download_span(span: &Span, message: impl Into<String>) {
    span.pb_set_style(&download_progress_style());
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

/// Initialize a span with a spinner (for unknown duration operations)
#[allow(dead_code)]
pub fn init_spinner_span(span: &Span, message: impl Into<String>) {
    span.pb_set_style(&spinner_style());
    let msg = message.into();
    span.pb_set_message(&msg);
}
