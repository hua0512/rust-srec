use flv_fix::writer_task::FlvWriterTask;
use hls_fix::writer_task::HlsWriterTask;
use indicatif::{FormattedDuration, MultiProgress, ProgressBar, ProgressStyle};
use std::time::Duration;

use crate::utils::format_duration;

/// A struct that manages multiple progress bars for file operations
#[derive(Clone)]
pub struct ProgressManager {
    multi: MultiProgress,
    main_progress: ProgressBar,
    pub file_progress: Option<ProgressBar>,
    pub url_progress: Option<ProgressBar>,
    status_progress: ProgressBar,
    disabled: bool,
}

#[allow(dead_code)]
impl ProgressManager {
    /// Creates a new progress manager with a main progress bar
    pub fn new(total_size: Option<u64>) -> Self {
        Self::new_with_mode(total_size, false)
    }

    /// Creates a new progress manager with silent mode (hidden but created)
    pub fn new_with_mode(total_size: Option<u64>, silent: bool) -> Self {
        let multi = MultiProgress::new();

        // Main progress bar (for overall progress)
        let main_progress = match total_size {
            Some(size) => {
                let pb = multi.add(ProgressBar::new(size));
                if silent {
                    pb.set_draw_target(indicatif::ProgressDrawTarget::hidden());
                }
                pb.set_style(
                    ProgressStyle::default_bar()
                        .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}, {eta})")
                        .unwrap()
                        .progress_chars("#>-")
                );
                pb.set_message("Total progress");
                pb
            }
            None => {
                let pb = multi.add(ProgressBar::new_spinner());
                if silent {
                    pb.set_draw_target(indicatif::ProgressDrawTarget::hidden());
                }
                pb.set_style(
                    ProgressStyle::default_spinner()
                        .template("{spinner:.green} {elapsed_precise} {msg}")
                        .unwrap(),
                );
                pb.set_message("Processing...");
                pb.enable_steady_tick(Duration::from_millis(100));
                pb
            }
        };

        // Status bar for messages
        let status_progress = multi.add(ProgressBar::new_spinner());
        if silent {
            status_progress.set_draw_target(indicatif::ProgressDrawTarget::hidden());
        }
        status_progress.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.blue} {msg}")
                .unwrap(),
        );
        status_progress.set_message("Initializing...");
        status_progress.enable_steady_tick(Duration::from_millis(100));

        Self {
            multi,
            main_progress,
            file_progress: None,
            url_progress: None,
            status_progress,
            disabled: false,
        }
    }

    /// Creates a disabled progress manager that doesn't initialize any progress bars
    pub fn disabled() -> Self {
        // Create dummy progress bars that don't display anything
        let multi = MultiProgress::new();
        let dummy_bar = ProgressBar::hidden();

        Self {
            multi,
            main_progress: dummy_bar.clone(),
            file_progress: None,
            url_progress: None,
            status_progress: dummy_bar,
            disabled: true,
        }
    }

    /// Add a file progress bar for the current file being processed
    pub fn add_file_progress(&mut self, filename: &str) -> ProgressBar {
        // If disabled, return a hidden progress bar without doing anything
        if self.disabled {
            return ProgressBar::hidden();
        }

        // Remove the old file progress if it exists
        if let Some(old_pb) = self.file_progress.take() {
            old_pb.finish_and_clear();
        }

        let file_progress = self.multi.add(ProgressBar::new(0));
        file_progress.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{msg}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec})")
                .unwrap()
                .progress_chars("#>-")
        );
        file_progress.set_message(format!("Processing {}", filename));

        self.file_progress = Some(file_progress.clone());
        file_progress
    }

    /// Add a URL progress bar for the current URL being downloaded
    pub fn add_url_progress(&mut self, url: &str) -> ProgressBar {
        // If disabled, return a hidden progress bar without doing anything
        if self.disabled {
            return ProgressBar::hidden();
        }

        // Remove the old URL progress if it exists
        if let Some(old_pb) = self.url_progress.take() {
            old_pb.finish_and_clear();
        }

        let url_progress = self.multi.add(ProgressBar::new(0));
        url_progress.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{msg}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec})")
                .unwrap()
                .progress_chars("#>-")
        );

        // Truncate URL if it's too long for display
        let display_url = if url.len() > 50 {
            format!("{}...{}", &url[..25], &url[url.len() - 22..])
        } else {
            url.to_string()
        };

        url_progress.set_message(format!("Downloading {}", display_url));

        self.url_progress = Some(url_progress.clone());
        url_progress
    }

    /// Sets up callbacks on a FlvWriterTask to update the progress bars
    pub fn setup_writer_task_callbacks(&self, writer_task: &mut FlvWriterTask) {
        // Skip setting up callbacks if progress manager is disabled
        if self.disabled {
            return;
        }

        if let Some(file_progress) = &self.file_progress {
            let file_pb = file_progress.clone();

            // Set up status callback for continuous progress updates
            writer_task.set_status_callback(move |path, size, _rate, duration| {
                if let Some(path) = path {
                    let path_display = path
                        .file_name()
                        .unwrap_or_else(|| path.as_os_str())
                        .to_string_lossy();
                    file_pb.set_length(size);
                    file_pb.set_position(size);

                    // Display video duration prominently in the message
                    file_pb.set_message(format!(
                        "Duration: {} | {}",
                        FormattedDuration(std::time::Duration::from_millis(
                            duration.unwrap_or(0) as u64
                        )),
                        path_display,
                    ));
                }
            });

            // Set up segment open callback
            let status_pb_open = self.status_progress.clone();
            writer_task.set_on_segment_open(move |path, segment_num| {
                let filename = path
                    .file_name()
                    .unwrap_or_else(|| path.as_os_str())
                    .to_string_lossy();
                status_pb_open
                    .set_message(format!("Opened segment #{}: {}", segment_num, filename));
            });

            // Set up segment close callback
            let status_pb_close = self.status_progress.clone();
            writer_task.set_on_segment_close(move |path, segment_num, tags, duration| {
                let filename = path
                    .file_name()
                    .unwrap_or_else(|| path.as_os_str())
                    .to_string_lossy();

                // Format duration if available
                let duration_str = match duration {
                    Some(ms) => format_duration(ms as f64 / 1000.0),
                    None => "unknown duration".to_string(),
                };

                status_pb_close.set_message(format!(
                    "Closed segment #{}: {} ({} tags, {})",
                    segment_num, filename, tags, duration_str
                ));
            });
        }
    }

    /// Sets up callbacks on a FlvWriterTask to update the progress bars
    pub fn setup_hls_writer_task_callbacks(&self, writer_task: &mut HlsWriterTask) {
        // Skip setting up callbacks if progress manager is disabled
        if self.disabled {
            return;
        }

        if let Some(file_progress) = &self.file_progress {
            let file_pb = file_progress.clone();

            // Set up status callback for continuous progress updates
            writer_task.set_status_callback(move |path, size, _rate, duration| {
                if let Some(path) = path {
                    let path_display = path
                        .file_name()
                        .unwrap_or_else(|| path.as_os_str())
                        .to_string_lossy();
                    file_pb.set_length(size);
                    file_pb.set_position(size);

                    // Display video duration prominently in the message
                    file_pb.set_message(format!(
                        "Duration: {} | {}",
                        FormattedDuration(std::time::Duration::from_millis(
                            duration.unwrap_or(0) as u64
                        )),
                        path_display,
                    ));
                }
            });

            // Set up segment open callback
            let status_pb_open = self.status_progress.clone();
            writer_task.set_on_segment_open(move |path, segment_type| {
                let filename = path
                    .file_name()
                    .unwrap_or_else(|| path.as_os_str())
                    .to_string_lossy();
                status_pb_open.set_message(format!(
                    "Opened segment type #{:?}: {}",
                    segment_type, filename
                ));
            });

            // Set up segment close callback
            let status_pb_close = self.status_progress.clone();
            writer_task.set_on_segment_close(move |path, segment_num, tags, duration| {
                let filename = path
                    .file_name()
                    .unwrap_or_else(|| path.as_os_str())
                    .to_string_lossy();

                // Format duration if available
                let duration_str = format_duration(duration as f64 / 1000.0);

                status_pb_close.set_message(format!(
                    "Closed segment #{:?}: {} ({} tags, {})",
                    segment_num, filename, tags, duration_str
                ));
            });
        }
    }

    /// Updates the main progress bar position
    pub fn update_main_progress(&self, position: u64) {
        if self.disabled {
            return;
        }

        if self.main_progress.length().unwrap_or(0) > 0 {
            self.main_progress.set_position(position);
        }
    }

    /// Updates the status message
    pub fn set_status(&self, msg: &str) {
        if !self.disabled {
            self.status_progress.set_message(msg.to_string());
        }
    }

    /// Finish all progress bars with a final message
    pub fn finish(&self, msg: &str) {
        if self.disabled {
            return;
        }

        self.main_progress.finish_with_message(msg.to_string());
        if let Some(file_progress) = &self.file_progress {
            file_progress.finish();
        }
        if let Some(url_progress) = &self.url_progress {
            url_progress.finish();
        }
        self.status_progress.finish_with_message(msg.to_string());
    }

    /// Finish just the file progress bar
    pub fn finish_file(&self, msg: &str) {
        if self.disabled {
            return;
        }

        if let Some(file_progress) = &self.file_progress {
            file_progress.finish_with_message(msg.to_string());
        }
    }

    /// Finish just the URL progress bar
    pub fn finish_url(&self, msg: &str) {
        if self.disabled {
            return;
        }

        if let Some(url_progress) = &self.url_progress {
            url_progress.finish_with_message(msg.to_string());
        }
    }

    /// Get access to the main progress bar
    pub fn get_main_progress(&self) -> &ProgressBar {
        &self.main_progress
    }

    /// Get access to the status progress bar
    pub fn get_status_progress(&self) -> &ProgressBar {
        &self.status_progress
    }

    /// Get access to the file progress bar if it exists
    pub fn get_file_progress(&self) -> Option<&ProgressBar> {
        self.file_progress.as_ref()
    }

    /// Get access to the URL progress bar if it exists
    pub fn get_url_progress(&self) -> Option<&ProgressBar> {
        self.url_progress.as_ref()
    }

    /// Check if the progress manager is disabled
    pub fn is_disabled(&self) -> bool {
        self.disabled
    }
}
