use std::{fmt::Display, time::Duration};

#[derive(Debug, Clone)]
pub struct PipelineConfig {
    /// Maximum file size limit in bytes (0 = unlimited)
    pub max_file_size: u64,

    /// Maximum duration limit
    pub max_duration: Option<Duration>,

    /// Size of internal processing channels
    pub channel_size: usize,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            max_file_size: 0,
            max_duration: None,
            channel_size: 32,
        }
    }
}

impl Display for PipelineConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let max_size_display = if self.max_file_size == 0 {
            "unlimited".to_string()
        } else {
            format!("{} bytes", self.max_file_size)
        };

        let max_duration_display = match self.max_duration {
            Some(duration) => format!("{:.2}s", duration.as_secs_f64()),
            None => "unlimited".to_string(),
        };

        write!(
            f,
            "PipelineConfig {{ max_file_size: {}, max_duration: {}, channel_size: {} }}",
            max_size_display, max_duration_display, self.channel_size
        )
    }
}

impl PipelineConfig {
    pub fn builder() -> PipelineConfigBuilder {
        PipelineConfigBuilder::default()
    }
}

#[derive(Debug, Clone, Default)]
pub struct PipelineConfigBuilder {
    config: PipelineConfig,
}

impl PipelineConfigBuilder {
    pub fn max_file_size(mut self, max_file_size: u64) -> Self {
        self.config.max_file_size = max_file_size;
        self
    }

    pub fn max_duration(mut self, max_duration: Duration) -> Self {
        self.config.max_duration = Some(max_duration);
        self
    }

    pub fn max_duration_s(mut self, max_duration_s: f64) -> Self {
        if max_duration_s > 0.0 {
            self.config.max_duration = Some(Duration::from_secs_f64(max_duration_s));
        }
        self
    }

    pub fn channel_size(mut self, channel_size: usize) -> Self {
        self.config.channel_size = channel_size;
        self
    }

    pub fn build(self) -> PipelineConfig {
        self.config
    }
}
