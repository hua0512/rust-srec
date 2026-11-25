use std::collections::HashMap;
use std::sync::Arc;

use super::{BigoNotificationProblem, NotificationEvent};
use crate::domain::credential::{CredentialEvent, UnavailableReason};
use crate::i18n::t_str_in;
use crate::utils::text::{format_bytes, format_duration};

/// The paused connection as the description names it.
fn throttled_route(locale: &str, route: &str, proxy_name: Option<&str>) -> String {
    match (route, proxy_name) {
        ("proxy", Some(name)) => t_str_in!(
            locale,
            "notification.platform_throttled.route.proxy",
            name = name,
        ),
        ("system", _) => t_str_in!(locale, "notification.platform_throttled.route.system"),
        _ => t_str_in!(locale, "notification.platform_throttled.route.direct"),
    }
}

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
            Self::StreamOnline { streamer_name, .. } => t_str_in!(
                locale,
                "notification.stream_online.title",
                streamer_name = streamer_name.as_str(),
            ),
            Self::StreamOffline { streamer_name, .. } => t_str_in!(
                locale,
                "notification.stream_offline.title",
                streamer_name = streamer_name.as_str(),
            ),
            Self::DownloadStarted { streamer_name, .. } => t_str_in!(
                locale,
                "notification.download_started.title",
                streamer_name = streamer_name.as_str(),
            ),
            Self::DownloadCompleted { streamer_name, .. } => t_str_in!(
                locale,
                "notification.download_completed.title",
                streamer_name = streamer_name.as_str(),
            ),
            Self::DownloadError { streamer_name, .. } => t_str_in!(
                locale,
                "notification.download_error.title",
                streamer_name = streamer_name.as_str(),
            ),
            Self::SegmentStarted {
                streamer_name,
                segment_index,
                ..
            } => t_str_in!(
                locale,
                "notification.segment_started.title",
                streamer_name = streamer_name.as_str(),
                segment_index = segment_index.to_string().as_str(),
            ),
            Self::SegmentCompleted {
                streamer_name,
                segment_index,
                ..
            } => t_str_in!(
                locale,
                "notification.segment_completed.title",
                streamer_name = streamer_name.as_str(),
                segment_index = segment_index.to_string().as_str(),
            ),
            Self::DownloadCancelled { streamer_name, .. } => t_str_in!(
                locale,
                "notification.download_cancelled.title",
                streamer_name = streamer_name.as_str(),
            ),
            Self::DownloadRejected { streamer_name, .. } => t_str_in!(
                locale,
                "notification.download_rejected.title",
                streamer_name = streamer_name.as_str(),
            ),
            Self::PipelineStarted { job_type, .. } => t_str_in!(
                locale,
                "notification.pipeline_started.title",
                job_type = job_type.as_str(),
            ),
            Self::PipelineCompleted { job_type, .. } => t_str_in!(
                locale,
                "notification.pipeline_completed.title",
                job_type = job_type.as_str(),
            ),
            Self::PipelineFailed { job_type, .. } => t_str_in!(
                locale,
                "notification.pipeline_failed.title",
                job_type = job_type.as_str(),
            ),
            Self::PipelineCancelled { job_type, .. } => t_str_in!(
                locale,
                "notification.pipeline_cancelled.title",
                job_type = job_type.as_str(),
            ),
            Self::FatalError {
                streamer_name,
                error_type,
                ..
            } => t_str_in!(
                locale,
                "notification.fatal_error.title",
                streamer_name = streamer_name.as_str(),
                error_type = error_type.as_str(),
            ),
            Self::StreamerChecksFailing { streamer_name, .. } => t_str_in!(
                locale,
                "notification.streamer_checks_failing.title",
                streamer_name = streamer_name.as_str(),
            ),
            Self::StreamerChecksRestored { streamer_name, .. } => t_str_in!(
                locale,
                "notification.streamer_checks_restored.title",
                streamer_name = streamer_name.as_str(),
            ),
            Self::OutOfSpace { path, .. } => {
                t_str_in!(
                    locale,
                    "notification.out_of_space.title",
                    path = path.as_str(),
                )
            }
            Self::OutputPathInaccessible { path, .. } => t_str_in!(
                locale,
                "notification.output_path_inaccessible.title",
                path = path.as_str(),
            ),
            Self::GpuUnavailable { error_kind, .. } => t_str_in!(
                locale,
                "notification.gpu_unavailable.title",
                error_kind = error_kind.as_str(),
            ),
            Self::PipelineQueueWarning { queue_depth, .. } => t_str_in!(
                locale,
                "notification.pipeline_queue_warning.title",
                queue_depth = queue_depth.to_string().as_str(),
            ),
            Self::PipelineQueueCritical { queue_depth, .. } => t_str_in!(
                locale,
                "notification.pipeline_queue_critical.title",
                queue_depth = queue_depth.to_string().as_str(),
            ),
            Self::SystemStartup { version, .. } => t_str_in!(
                locale,
                "notification.system_startup.title",
                version = version.as_str(),
            ),
            Self::SystemShutdown { reason, .. } => t_str_in!(
                locale,
                "notification.system_shutdown.title",
                reason = reason.as_str(),
            ),
            Self::PlatformThrottled { platform, .. } => t_str_in!(
                locale,
                "notification.platform_throttled.title",
                platform = platform.as_str(),
            ),
            Self::BigoNotificationsFailing { problem, .. } => match problem {
                BigoNotificationProblem::NotArriving => {
                    t_str_in!(
                        locale,
                        "notification.bigo_notifications_failing.title.not_arriving"
                    )
                }
                BigoNotificationProblem::Disconnected => {
                    t_str_in!(
                        locale,
                        "notification.bigo_notifications_failing.title.disconnected"
                    )
                }
            },
            Self::BigoNotificationsRestored { problem, .. } => match problem {
                BigoNotificationProblem::NotArriving => {
                    t_str_in!(
                        locale,
                        "notification.bigo_notifications_restored.title.not_arriving"
                    )
                }
                BigoNotificationProblem::Disconnected => {
                    t_str_in!(
                        locale,
                        "notification.bigo_notifications_restored.title.disconnected"
                    )
                }
            },
            Self::Credential { event } => credential_title(event, locale),
            Self::BaiduPcsReloginFailed { config_dir, .. } => t_str_in!(
                locale,
                "notification.baidupcs_relogin_failed.title",
                config_dir = config_dir.as_str(),
            ),
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
                Some(cat) => t_str_in!(
                    locale,
                    "notification.stream_online.description.with_category",
                    title = title.as_str(),
                    category = cat.as_str(),
                ),
                None => t_str_in!(
                    locale,
                    "notification.stream_online.description.plain",
                    title = title.as_str(),
                ),
            },
            Self::StreamOffline { duration_secs, .. } => match duration_secs {
                Some(secs) => t_str_in!(
                    locale,
                    "notification.stream_offline.description.with_duration",
                    duration = format_duration(*secs).as_str(),
                ),
                None => t_str_in!(locale, "notification.stream_offline.description.plain"),
            },
            Self::DownloadStarted { session_id, .. } => t_str_in!(
                locale,
                "notification.download_started.description",
                session_id = session_id.as_str(),
            ),
            Self::DownloadCompleted {
                file_size_bytes,
                duration_secs,
                ..
            } => t_str_in!(
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
                t_str_in!(locale, key, error_message = error_message.as_str())
            }
            Self::SegmentStarted { segment_path, .. } => t_str_in!(
                locale,
                "notification.segment_started.description",
                segment_path = segment_path.as_str(),
            ),
            Self::SegmentCompleted {
                segment_path,
                size_bytes,
                duration_secs,
                ..
            } => t_str_in!(
                locale,
                "notification.segment_completed.description",
                segment_path = segment_path.as_str(),
                size = format_bytes(*size_bytes).as_str(),
                duration = format_duration(*duration_secs).as_str(),
            ),
            Self::DownloadCancelled { session_id, .. } => t_str_in!(
                locale,
                "notification.download_cancelled.description",
                session_id = session_id.as_str(),
            ),
            Self::DownloadRejected { reason, .. } => t_str_in!(
                locale,
                "notification.download_rejected.description",
                reason = reason.as_str(),
            ),
            Self::PipelineStarted { job_id, .. } => t_str_in!(
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
                    Some(path) => t_str_in!(
                        locale,
                        "notification.pipeline_completed.description.with_output",
                        output_path = path.as_str(),
                        duration = duration.as_str(),
                    ),
                    None => t_str_in!(
                        locale,
                        "notification.pipeline_completed.description.without_output",
                        duration = duration.as_str(),
                    ),
                }
            }
            Self::PipelineFailed { error_message, .. } => t_str_in!(
                locale,
                "notification.pipeline_failed.description",
                error_message = error_message.as_str(),
            ),
            Self::PipelineCancelled {
                job_id,
                pipeline_id,
                ..
            } => match pipeline_id {
                Some(pid) => t_str_in!(
                    locale,
                    "notification.pipeline_cancelled.description.with_pipeline",
                    job_id = job_id.as_str(),
                    pipeline_id = pid.as_str(),
                ),
                None => t_str_in!(
                    locale,
                    "notification.pipeline_cancelled.description.plain",
                    job_id = job_id.as_str(),
                ),
            },
            Self::FatalError { message, .. } => t_str_in!(
                locale,
                "notification.fatal_error.description",
                message = message.as_str(),
            ),
            Self::StreamerChecksFailing {
                platform,
                last_error,
                consecutive_errors,
                ..
            } => t_str_in!(
                locale,
                "notification.streamer_checks_failing.description",
                platform = platform.as_str(),
                count = consecutive_errors.to_string().as_str(),
                last_error = last_error.as_str(),
            ),
            Self::StreamerChecksRestored { .. } => {
                t_str_in!(locale, "notification.streamer_checks_restored.description")
            }
            Self::OutOfSpace {
                available_bytes,
                threshold_bytes,
                ..
            } => t_str_in!(
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
                t_str_in!(
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
                // in sync with `GpuErrorKind::as_str` (covered by a unit test
                // in `gpu_health.rs`).
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
                t_str_in!(
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
            } => t_str_in!(
                locale,
                "notification.pipeline_queue_warning.description",
                queue_depth = queue_depth.to_string().as_str(),
                threshold = threshold.to_string().as_str(),
            ),
            Self::PipelineQueueCritical {
                queue_depth,
                threshold,
                ..
            } => t_str_in!(
                locale,
                "notification.pipeline_queue_critical.description",
                queue_depth = queue_depth.to_string().as_str(),
                threshold = threshold.to_string().as_str(),
            ),
            Self::SystemStartup { version, .. } => t_str_in!(
                locale,
                "notification.system_startup.description",
                version = version.as_str(),
            ),
            Self::SystemShutdown { reason, .. } => t_str_in!(
                locale,
                "notification.system_shutdown.description",
                reason = reason.as_str(),
            ),
            Self::PlatformThrottled {
                platform,
                route,
                proxy_name,
                retry_at,
                ..
            } => t_str_in!(
                locale,
                "notification.platform_throttled.description",
                platform = platform.as_str(),
                route = throttled_route(locale, route, proxy_name.as_deref()).as_str(),
                retry_at = retry_at
                    .strftime("%Y-%m-%d %H:%M:%S UTC")
                    .to_string()
                    .as_str(),
            ),
            Self::BigoNotificationsFailing {
                problem, message, ..
            } => match (problem, message) {
                (BigoNotificationProblem::NotArriving, _) => t_str_in!(
                    locale,
                    "notification.bigo_notifications_failing.description.not_arriving"
                ),
                (BigoNotificationProblem::Disconnected, Some(message)) => t_str_in!(
                    locale,
                    "notification.bigo_notifications_failing.description.disconnected_with_error",
                    message = message.as_str(),
                ),
                (BigoNotificationProblem::Disconnected, None) => t_str_in!(
                    locale,
                    "notification.bigo_notifications_failing.description.disconnected"
                ),
            },
            Self::BigoNotificationsRestored { problem, .. } => match problem {
                BigoNotificationProblem::NotArriving => t_str_in!(
                    locale,
                    "notification.bigo_notifications_restored.description.not_arriving"
                ),
                BigoNotificationProblem::Disconnected => t_str_in!(
                    locale,
                    "notification.bigo_notifications_restored.description.disconnected"
                ),
            },
            Self::Credential { event } => credential_description(event, locale),
            Self::BaiduPcsReloginFailed { message, .. } => t_str_in!(
                locale,
                "notification.baidupcs_relogin_failed.description",
                message = message.as_str(),
            ),
        }
    }
}

/// The scope as shown to users, naming the account for profile events.
fn credential_scope_text(event: &CredentialEvent, locale: &str) -> String {
    let (scope, label) = event.scope_and_label();
    match label {
        Some(label) => t_str_in!(
            locale,
            "notification.credential.scope_with_account",
            scope = scope.describe().as_str(),
            account = label,
        ),
        None => scope.describe(),
    }
}

/// Notification text key for each unavailable reason.
fn unavailable_reason_key(reason: UnavailableReason) -> &'static str {
    match reason {
        UnavailableReason::LoginRequired => {
            "notification.credential.unavailable.reason.login_required"
        }
        UnavailableReason::ProfilesDisabled => {
            "notification.credential.unavailable.reason.profiles_disabled"
        }
        UnavailableReason::BoundProfileUnavailable => {
            "notification.credential.unavailable.reason.bound_profile_unavailable"
        }
        UnavailableReason::BindingPolicyChanged => {
            "notification.credential.unavailable.reason.binding_policy_changed"
        }
        UnavailableReason::AttemptsExhausted => {
            "notification.credential.unavailable.reason.attempts_exhausted"
        }
    }
}

/// Build the localized description for a [`CredentialEvent`] when wrapped in a
/// [`NotificationEvent::Credential`].
fn credential_description(event: &CredentialEvent, locale: &str) -> String {
    let scope_text = credential_scope_text(event, locale);
    match event {
        CredentialEvent::Unavailable {
            platform,
            reason_code,
            ..
        } => {
            let reason = t_str_in!(locale, unavailable_reason_key(*reason_code));
            t_str_in!(
                locale,
                "notification.credential.unavailable.message",
                platform = platform.as_str(),
                scope = scope_text.as_str(),
                reason = reason.as_str(),
            )
        }
        CredentialEvent::Refreshed { platform, .. } => t_str_in!(
            locale,
            "notification.credential.refreshed.message",
            platform = platform.as_str(),
            scope = scope_text.as_str(),
        ),
        CredentialEvent::RefreshFailed {
            platform,
            error,
            requires_relogin,
            failure_count,
            ..
        } => {
            let key = if *requires_relogin {
                "notification.credential.refresh_failed.message.requires_relogin"
            } else {
                "notification.credential.refresh_failed.message.retrying"
            };
            t_str_in!(
                locale,
                key,
                platform = platform.as_str(),
                scope = scope_text.as_str(),
                error = error.as_str(),
                failure_count = failure_count.to_string().as_str(),
            )
        }
        CredentialEvent::Invalid {
            platform,
            reason,
            error_code,
            ..
        } => {
            // Pre-format the optional error_code as a string so the YAML
            // can just interpolate without conditional syntax. "N/A" is
            // intentional — the zh-CN translation renders it verbatim,
            // avoiding a branch on Option inside the YAML.
            let error_code = error_code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "N/A".to_string());
            t_str_in!(
                locale,
                "notification.credential.invalid.message",
                platform = platform.as_str(),
                scope = scope_text.as_str(),
                reason = reason.as_str(),
                error_code = error_code.as_str(),
            )
        }
        CredentialEvent::ExpiringSoon {
            platform,
            days_remaining,
            expires_at,
            ..
        } => {
            let expires_at = expires_at.strftime("%Y-%m-%d").to_string();
            t_str_in!(
                locale,
                "notification.credential.expiring_soon.message",
                platform = platform.as_str(),
                scope = scope_text.as_str(),
                days_remaining = days_remaining.to_string().as_str(),
                expires_at = expires_at.as_str(),
            )
        }
        CredentialEvent::SessionSaveFailed {
            platform, error, ..
        } => t_str_in!(
            locale,
            "notification.credential.session_save_failed.message",
            platform = platform.as_str(),
            scope = scope_text.as_str(),
            error = error.as_str(),
        ),
    }
}

/// Build the localized title for a [`CredentialEvent`] when wrapped in a
/// [`NotificationEvent::Credential`]. Split out because the credential
/// variant has two nested match layers (`CredentialEvent` variant +
/// `requires_relogin` branching in `RefreshFailed`); inlining it would
/// have made the main `title()` match unreadable.
fn credential_title(event: &CredentialEvent, locale: &str) -> String {
    let scope_text = credential_scope_text(event, locale);
    match event {
        CredentialEvent::Unavailable { platform, .. } => t_str_in!(
            locale,
            "notification.credential.unavailable.title",
            platform = platform.as_str(),
            scope = scope_text.as_str()
        ),
        CredentialEvent::Refreshed { platform, .. } => t_str_in!(
            locale,
            "notification.credential.refreshed.title",
            platform = platform.as_str(),
            scope = scope_text.as_str(),
        ),
        CredentialEvent::RefreshFailed {
            platform,
            requires_relogin,
            ..
        } => {
            let key = if *requires_relogin {
                "notification.credential.refresh_failed.title.requires_relogin"
            } else {
                "notification.credential.refresh_failed.title.retrying"
            };
            t_str_in!(
                locale,
                key,
                platform = platform.as_str(),
                scope = scope_text.as_str(),
            )
        }
        CredentialEvent::Invalid { platform, .. } => t_str_in!(
            locale,
            "notification.credential.invalid.title",
            platform = platform.as_str(),
            scope = scope_text.as_str(),
        ),
        CredentialEvent::ExpiringSoon { platform, .. } => t_str_in!(
            locale,
            "notification.credential.expiring_soon.title",
            platform = platform.as_str(),
            scope = scope_text.as_str(),
        ),
        CredentialEvent::SessionSaveFailed { platform, .. } => t_str_in!(
            locale,
            "notification.credential.session_save_failed.title",
            platform = platform.as_str(),
            scope = scope_text.as_str(),
        ),
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
