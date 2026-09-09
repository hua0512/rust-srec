use std::collections::HashMap;
use std::sync::Arc;

use super::NotificationEvent;
use crate::credentials::CredentialEvent;

impl NotificationEvent {
    /// Get a human-readable title for this event, in the process-wide locale.
    ///
    /// For a channel that carries its own locale, call [`Self::title_in`] instead: two channels
    /// can be delivering the same event in different languages at once.
    pub fn title(&self) -> String {
        self.title_in(&crate::i18n::current_locale())
    }

    /// Get a human-readable title for this event, in `locale`.
    ///
    /// `locale` falls back to the embedded `en` YAML when it names a locale with no file under
    /// `rust-srec/locales/`; currently `en` and `zh-CN` are supported.
    ///
    /// Numeric placeholders (`segment_index`, `queue_depth`, etc.) are
    /// stringified with `.to_string()` before passing to the macro because
    /// `rust_i18n` placeholders take `&str`. The YAML stays free of
    /// Rust-specific formatting.
    pub fn title_in(&self, locale: &str) -> String {
        match self {
            Self::StreamOnline { streamer_name, .. } => crate::t_str_in!(
                locale,
                "notification.stream_online.title",
                streamer_name = streamer_name.as_str(),
            ),
            Self::StreamOffline { streamer_name, .. } => crate::t_str_in!(
                locale,
                "notification.stream_offline.title",
                streamer_name = streamer_name.as_str(),
            ),
            Self::DownloadStarted { streamer_name, .. } => crate::t_str_in!(
                locale,
                "notification.download_started.title",
                streamer_name = streamer_name.as_str(),
            ),
            Self::DownloadCompleted { streamer_name, .. } => crate::t_str_in!(
                locale,
                "notification.download_completed.title",
                streamer_name = streamer_name.as_str(),
            ),
            Self::DownloadError { streamer_name, .. } => crate::t_str_in!(
                locale,
                "notification.download_error.title",
                streamer_name = streamer_name.as_str(),
            ),
            Self::SegmentStarted {
                streamer_name,
                segment_index,
                ..
            } => crate::t_str_in!(
                locale,
                "notification.segment_started.title",
                streamer_name = streamer_name.as_str(),
                segment_index = segment_index.to_string().as_str(),
            ),
            Self::SegmentCompleted {
                streamer_name,
                segment_index,
                ..
            } => crate::t_str_in!(
                locale,
                "notification.segment_completed.title",
                streamer_name = streamer_name.as_str(),
                segment_index = segment_index.to_string().as_str(),
            ),
            Self::DownloadCancelled { streamer_name, .. } => crate::t_str_in!(
                locale,
                "notification.download_cancelled.title",
                streamer_name = streamer_name.as_str(),
            ),
            Self::DownloadRejected { streamer_name, .. } => crate::t_str_in!(
                locale,
                "notification.download_rejected.title",
                streamer_name = streamer_name.as_str(),
            ),
            Self::ConfigUpdated { streamer_name, .. } => crate::t_str_in!(
                locale,
                "notification.config_updated.title",
                streamer_name = streamer_name.as_str(),
            ),
            Self::PipelineStarted { job_type, .. } => crate::t_str_in!(
                locale,
                "notification.pipeline_started.title",
                job_type = job_type.as_str(),
            ),
            Self::PipelineCompleted { job_type, .. } => crate::t_str_in!(
                locale,
                "notification.pipeline_completed.title",
                job_type = job_type.as_str(),
            ),
            Self::PipelineFailed { job_type, .. } => crate::t_str_in!(
                locale,
                "notification.pipeline_failed.title",
                job_type = job_type.as_str(),
            ),
            Self::PipelineCancelled { job_type, .. } => crate::t_str_in!(
                locale,
                "notification.pipeline_cancelled.title",
                job_type = job_type.as_str(),
            ),
            Self::FatalError {
                streamer_name,
                error_type,
                ..
            } => crate::t_str_in!(
                locale,
                "notification.fatal_error.title",
                streamer_name = streamer_name.as_str(),
                error_type = error_type.as_str(),
            ),
            Self::OutOfSpace { path, .. } => {
                crate::t_str_in!(
                    locale,
                    "notification.out_of_space.title",
                    path = path.as_str(),
                )
            }
            Self::OutputPathInaccessible { path, .. } => crate::t_str_in!(
                locale,
                "notification.output_path_inaccessible.title",
                path = path.as_str(),
            ),
            Self::GpuUnavailable { error_kind, .. } => crate::t_str_in!(
                locale,
                "notification.gpu_unavailable.title",
                error_kind = error_kind.as_str(),
            ),
            Self::PipelineQueueWarning { queue_depth, .. } => crate::t_str_in!(
                locale,
                "notification.pipeline_queue_warning.title",
                queue_depth = queue_depth.to_string().as_str(),
            ),
            Self::PipelineQueueCritical { queue_depth, .. } => crate::t_str_in!(
                locale,
                "notification.pipeline_queue_critical.title",
                queue_depth = queue_depth.to_string().as_str(),
            ),
            Self::SystemStartup { version, .. } => crate::t_str_in!(
                locale,
                "notification.system_startup.title",
                version = version.as_str(),
            ),
            Self::SystemShutdown { reason, .. } => crate::t_str_in!(
                locale,
                "notification.system_shutdown.title",
                reason = reason.as_str(),
            ),
            Self::Credential { event } => credential_title(event, locale),
            Self::BaiduPcsReloginFailed { config_dir, .. } => crate::t_str_in!(
                locale,
                "notification.baidupcs_relogin_failed.title",
                config_dir = config_dir.as_str(),
            ),
        }
    }

    /// Title in the channel's own locale, or the process-wide one when it has none.
    ///
    /// The shape channels want: a configured locale is optional per channel, and spelling the
    /// fallback out at each `send` would repeat it once per channel implementation.
    pub fn title_for(&self, locale: Option<&str>) -> String {
        match locale {
            Some(locale) => self.title_in(locale),
            None => self.title(),
        }
    }

    /// Description counterpart to [`Self::title_for`].
    pub fn description_for(&self, locale: Option<&str>) -> String {
        match locale {
            Some(locale) => self.description_in(locale),
            None => self.description(),
        }
    }

    /// Get a detailed description of this event, in the process-wide locale.
    pub fn description(&self) -> String {
        self.description_in(&crate::i18n::current_locale())
    }

    /// Get a detailed description of this event, in `locale`.
    pub fn description_in(&self, locale: &str) -> String {
        match self {
            Self::StreamOnline {
                title, category, ..
            } => match category {
                Some(cat) => crate::t_str_in!(
                    locale,
                    "notification.stream_online.description.with_category",
                    title = title.as_str(),
                    category = cat.as_str(),
                ),
                None => crate::t_str_in!(
                    locale,
                    "notification.stream_online.description.plain",
                    title = title.as_str(),
                ),
            },
            Self::StreamOffline { duration_secs, .. } => match duration_secs {
                Some(secs) => crate::t_str_in!(
                    locale,
                    "notification.stream_offline.description.with_duration",
                    duration = format_duration(*secs).as_str(),
                ),
                None => crate::t_str_in!(locale, "notification.stream_offline.description.plain"),
            },
            Self::DownloadStarted { session_id, .. } => crate::t_str_in!(
                locale,
                "notification.download_started.description",
                session_id = session_id.as_str(),
            ),
            Self::DownloadCompleted {
                file_size_bytes,
                duration_secs,
                ..
            } => crate::t_str_in!(
                locale,
                "notification.download_completed.description",
                size = format_bytes(*file_size_bytes).as_str(),
                duration = format_duration(*duration_secs).as_str(),
            ),
            Self::DownloadError {
                error_message,
                recoverable,
                ..
            } => {
                let key = if *recoverable {
                    "notification.download_error.description.recoverable"
                } else {
                    "notification.download_error.description.unrecoverable"
                };
                crate::t_str_in!(locale, key, error_message = error_message.as_str())
            }
            Self::SegmentStarted { segment_path, .. } => crate::t_str_in!(
                locale,
                "notification.segment_started.description",
                segment_path = segment_path.as_str(),
            ),
            Self::SegmentCompleted {
                segment_path,
                size_bytes,
                duration_secs,
                ..
            } => crate::t_str_in!(
                locale,
                "notification.segment_completed.description",
                segment_path = segment_path.as_str(),
                size = format_bytes(*size_bytes).as_str(),
                duration = format_duration(*duration_secs).as_str(),
            ),
            Self::DownloadCancelled { session_id, .. } => crate::t_str_in!(
                locale,
                "notification.download_cancelled.description",
                session_id = session_id.as_str(),
            ),
            Self::DownloadRejected { reason, .. } => crate::t_str_in!(
                locale,
                "notification.download_rejected.description",
                reason = reason.as_str(),
            ),
            Self::ConfigUpdated { update_type, .. } => crate::t_str_in!(
                locale,
                "notification.config_updated.description",
                update_type = update_type.as_str(),
            ),
            Self::PipelineStarted { job_id, .. } => crate::t_str_in!(
                locale,
                "notification.pipeline_started.description",
                job_id = job_id.as_str(),
            ),
            Self::PipelineCompleted {
                output_path,
                duration_secs,
                ..
            } => {
                let duration = format_duration(*duration_secs);
                match output_path {
                    Some(path) => crate::t_str_in!(
                        locale,
                        "notification.pipeline_completed.description.with_output",
                        output_path = path.as_str(),
                        duration = duration.as_str(),
                    ),
                    None => crate::t_str_in!(
                        locale,
                        "notification.pipeline_completed.description.without_output",
                        duration = duration.as_str(),
                    ),
                }
            }
            Self::PipelineFailed { error_message, .. } => crate::t_str_in!(
                locale,
                "notification.pipeline_failed.description",
                error_message = error_message.as_str(),
            ),
            Self::PipelineCancelled {
                job_id,
                pipeline_id,
                ..
            } => match pipeline_id {
                Some(pid) => crate::t_str_in!(
                    locale,
                    "notification.pipeline_cancelled.description.with_pipeline",
                    job_id = job_id.as_str(),
                    pipeline_id = pid.as_str(),
                ),
                None => crate::t_str_in!(
                    locale,
                    "notification.pipeline_cancelled.description.plain",
                    job_id = job_id.as_str(),
                ),
            },
            Self::FatalError { message, .. } => crate::t_str_in!(
                locale,
                "notification.fatal_error.description",
                message = message.as_str(),
            ),
            Self::OutOfSpace {
                available_bytes,
                threshold_bytes,
                ..
            } => crate::t_str_in!(
                locale,
                "notification.out_of_space.description",
                available = format_bytes(*available_bytes).as_str(),
                threshold = format_bytes(*threshold_bytes).as_str(),
            ),
            Self::OutputPathInaccessible {
                path, error_kind, ..
            } => {
                // Map the kind string to a per-kind i18n description; falls
                // back to the `other` key if we don't have a dedicated
                // branch. The kind strings here MUST stay in sync with
                // `IoErrorKindSer::as_str` (covered by a unit test in
                // traits.rs).
                let key = match error_kind.as_str() {
                    "not_found" => "notification.output_path_inaccessible.description.not_found",
                    "storage_full" => {
                        "notification.output_path_inaccessible.description.storage_full"
                    }
                    "permission_denied" => {
                        "notification.output_path_inaccessible.description.permission_denied"
                    }
                    "read_only" => "notification.output_path_inaccessible.description.read_only",
                    "timed_out" => "notification.output_path_inaccessible.description.timed_out",
                    _ => "notification.output_path_inaccessible.description.other",
                };
                crate::t_str_in!(
                    locale,
                    key,
                    path = path.as_str(),
                    kind = error_kind.as_str()
                )
            }
            Self::GpuUnavailable {
                error_kind,
                message,
                ..
            } => {
                // Per-kind i18n description; the kind strings here MUST stay
                // in sync with `crate::metrics::gpu_health::GpuErrorKind::as_str`
                // (covered by a unit test in gpu_health.rs).
                let key = match error_kind.as_str() {
                    "nvml_unknown_error" => {
                        "notification.gpu_unavailable.description.nvml_unknown_error"
                    }
                    "driver_mismatch" => "notification.gpu_unavailable.description.driver_mismatch",
                    "no_device" => "notification.gpu_unavailable.description.no_device",
                    "timed_out" => "notification.gpu_unavailable.description.timed_out",
                    "not_installed" => "notification.gpu_unavailable.description.not_installed",
                    _ => "notification.gpu_unavailable.description.other",
                };
                crate::t_str_in!(
                    locale,
                    key,
                    kind = error_kind.as_str(),
                    message = message.as_str(),
                )
            }
            Self::PipelineQueueWarning {
                queue_depth,
                threshold,
                ..
            } => crate::t_str_in!(
                locale,
                "notification.pipeline_queue_warning.description",
                queue_depth = queue_depth.to_string().as_str(),
                threshold = threshold.to_string().as_str(),
            ),
            Self::PipelineQueueCritical {
                queue_depth,
                threshold,
                ..
            } => crate::t_str_in!(
                locale,
                "notification.pipeline_queue_critical.description",
                queue_depth = queue_depth.to_string().as_str(),
                threshold = threshold.to_string().as_str(),
            ),
            Self::SystemStartup { version, .. } => crate::t_str_in!(
                locale,
                "notification.system_startup.description",
                version = version.as_str(),
            ),
            Self::SystemShutdown { reason, .. } => crate::t_str_in!(
                locale,
                "notification.system_shutdown.description",
                reason = reason.as_str(),
            ),
            Self::Credential { event } => event.to_message_in(locale),
            Self::BaiduPcsReloginFailed { message, .. } => crate::t_str_in!(
                locale,
                "notification.baidupcs_relogin_failed.description",
                message = message.as_str(),
            ),
        }
    }
}

/// Build the localized title for a [`CredentialEvent`] when wrapped in a
/// [`NotificationEvent::Credential`]. Split out because the credential
/// variant has two nested match layers (`CredentialEvent` variant +
/// `requires_relogin` branching in `RefreshFailed`); inlining it would
/// have made the main `title()` match unreadable.
fn credential_title(event: &CredentialEvent, locale: &str) -> String {
    match event {
        CredentialEvent::Refreshed {
            platform, scope, ..
        } => crate::t_str_in!(
            locale,
            "notification.credential.refreshed.title",
            platform = platform.as_str(),
            scope = scope.describe().as_str(),
        ),
        CredentialEvent::RefreshFailed {
            platform,
            scope,
            requires_relogin,
            ..
        } => {
            let key = if *requires_relogin {
                "notification.credential.refresh_failed.title.requires_relogin"
            } else {
                "notification.credential.refresh_failed.title.retrying"
            };
            crate::t_str_in!(
                locale,
                key,
                platform = platform.as_str(),
                scope = scope.describe().as_str(),
            )
        }
        CredentialEvent::Invalid {
            platform, scope, ..
        } => crate::t_str_in!(
            locale,
            "notification.credential.invalid.title",
            platform = platform.as_str(),
            scope = scope.describe().as_str(),
        ),
        CredentialEvent::ExpiringSoon {
            platform, scope, ..
        } => crate::t_str_in!(
            locale,
            "notification.credential.expiring_soon.title",
            platform = platform.as_str(),
            scope = scope.describe().as_str(),
        ),
    }
}

/// Format bytes into human-readable string.
pub(super) fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// Format duration in seconds into human-readable string.
pub(super) fn format_duration(secs: f64) -> String {
    let total_secs = secs as u64;
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;

    if hours > 0 {
        format!("{}h {}m {}s", hours, minutes, seconds)
    } else if minutes > 0 {
        format!("{}m {}s", minutes, seconds)
    } else {
        format!("{}s", seconds)
    }
}

/// Localized text shared by recipients of an event. Transport-specific escaping,
/// markup and truncation are applied by the channel, after rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedEvent {
    pub title: String,
    pub description: String,
}

impl RenderedEvent {
    pub fn new(event: &NotificationEvent, locale: &str) -> Self {
        Self {
            title: event.title_in(locale),
            description: event.description_in(locale),
        }
    }
}

/// One delivery pass owns its locale snapshot and cache. Retrying starts a new
/// pass, while every title/body pair within a pass uses the same resolved locale.
pub(crate) struct RenderCache {
    process_locale: String,
    rendered: HashMap<String, Arc<RenderedEvent>>,
}

impl RenderCache {
    pub(crate) fn new() -> Self {
        Self {
            process_locale: crate::i18n::current_locale(),
            rendered: HashMap::new(),
        }
    }

    pub(crate) fn get(
        &mut self,
        event: &NotificationEvent,
        locale: Option<&str>,
    ) -> Arc<RenderedEvent> {
        let locale = locale.unwrap_or(&self.process_locale);
        self.rendered
            .entry(locale.to_owned())
            .or_insert_with(|| Arc::new(RenderedEvent::new(event, locale)))
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pass_reuses_text_by_effective_locale_and_holds_its_fallback_snapshot() {
        let event = NotificationEvent::SystemStartup {
            version: "contract".into(),
            timestamp: chrono::Utc::now(),
        };
        let mut cache = RenderCache {
            process_locale: "en".into(),
            rendered: HashMap::new(),
        };
        let fallback = cache.get(&event, None);
        let explicit = cache.get(&event, Some("en"));
        let chinese = cache.get(&event, Some("zh-CN"));
        assert!(Arc::ptr_eq(&fallback, &explicit));
        assert!(!Arc::ptr_eq(&fallback, &chinese));
        assert_ne!(fallback.title, chinese.title);
        assert!(Arc::ptr_eq(&chinese, &cache.get(&event, Some("zh-CN"))));
        assert!(Arc::ptr_eq(&fallback, &cache.get(&event, None)));
        assert_eq!(cache.rendered.len(), 2);
    }
}
