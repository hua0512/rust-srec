use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use pipeline_common::progress::ProgressEvent as PipelineProgressEvent;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

fn download_style() -> ProgressStyle {
    ProgressStyle::default_bar()
        .template("{spinner:.green} {msg}\n[{elapsed_precise}] [{bar:40.green/white}] {bytes}/{total_bytes} @ {bytes_per_sec}")
        .unwrap()
        .progress_chars("=> ")
}

#[derive(Clone)]
pub struct ProgressManager {
    multi: MultiProgress,
    bars: Arc<Mutex<HashMap<PathBuf, ProgressBar>>>,
    disabled: bool,
}

impl ProgressManager {
    pub fn new(multi: MultiProgress) -> Self {
        let bars: Arc<Mutex<HashMap<PathBuf, ProgressBar>>> = Arc::new(Mutex::new(HashMap::new()));

        Self {
            multi,
            bars,
            disabled: false,
        }
    }

    pub fn new_disabled(multi: MultiProgress) -> Self {
        Self {
            multi,
            bars: Arc::new(Mutex::new(HashMap::new())),
            disabled: true,
        }
    }

    pub fn handle_event(&self, event: PipelineProgressEvent) {
        if self.disabled {
            return;
        }

        let mut bars = self.bars.lock().unwrap();
        match event {
            PipelineProgressEvent::FileOpened { path } => {
                let bar = self.multi.add(ProgressBar::new(0));
                bar.set_style(download_style());
                bar.set_message(format!("Processing {}", path.to_string_lossy()));
                bar.enable_steady_tick(Duration::from_millis(500));
                bars.insert(path.to_path_buf(), bar);
            }
            PipelineProgressEvent::ProgressUpdate { path, progress } => {
                if let Some(bar) = bars.get(path.as_ref()) {
                    if let Some(total) = progress.total_bytes {
                        bar.set_length(total);
                    }
                    bar.set_position(progress.bytes_written);
                }
            }
            PipelineProgressEvent::FileClosed { path } => {
                if let Some(bar) = bars.remove(path.as_ref()) {
                    bar.finish_with_message(format!("Finished {}", path.to_string_lossy()));
                }
            }
        }
    }

    #[inline]
    #[allow(unused)]
    pub fn is_disabled(&self) -> bool {
        self.disabled
    }
}
