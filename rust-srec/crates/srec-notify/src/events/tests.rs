use std::collections::BTreeSet;

use srec_core::domain::credential::CredentialEvent as C;

use super::*;
use crate::events::NotificationEvent as E;

/// Defines `SAMPLE_KINDS`, one entry per pattern, and `sample_kind`, which
/// matches the patterns in order. The `match` has no wildcard, so a new
/// variant fails to compile until it gets a pattern, and every pattern is a
/// kind the localization test requires `sample_events` to cover. A repeated
/// pattern is unreachable, which is denied.
macro_rules! sample_kinds {
    ($($pattern:pat),+ $(,)?) => {
        const SAMPLE_KINDS: &[&str] = &[$(stringify!($pattern)),+];

        #[deny(unreachable_patterns)]
        fn sample_kind(event: &NotificationEvent) -> &'static str {
            match event {
                $($pattern => stringify!($pattern)),+
            }
        }
    };
}

// Kinds whose rendering differs: each variant, and each credential event.
sample_kinds!(
    E::StreamOnline { .. },
    E::StreamOffline { .. },
    E::DownloadStarted { .. },
    E::DownloadCompleted { .. },
    E::DownloadError { .. },
    E::SegmentStarted { .. },
    E::SegmentCompleted { .. },
    E::DownloadCancelled { .. },
    E::DownloadRejected { .. },
    E::PipelineStarted { .. },
    E::PipelineCompleted { .. },
    E::PipelineFailed { .. },
    E::PipelineCancelled { .. },
    E::FatalError { .. },
    E::StreamerChecksFailing { .. },
    E::StreamerChecksRestored { .. },
    E::OutOfSpace { .. },
    E::OutputPathInaccessible { .. },
    E::GpuUnavailable { .. },
    E::PipelineQueueWarning { .. },
    E::PipelineQueueCritical { .. },
    E::SystemStartup { .. },
    E::SystemShutdown { .. },
    E::PlatformThrottled { .. },
    E::BigoNotificationsFailing { .. },
    E::BigoNotificationsRestored { .. },
    E::BaiduPcsReloginFailed { .. },
    E::Credential {
        event: C::Unavailable { .. }
    },
    E::Credential {
        event: C::Refreshed { .. }
    },
    E::Credential {
        event: C::RefreshFailed { .. }
    },
    E::Credential {
        event: C::Invalid { .. }
    },
    E::Credential {
        event: C::ExpiringSoon { .. }
    },
    E::Credential {
        event: C::SessionSaveFailed { .. }
    },
);

/// `OutputPathInaccessible` kinds, one i18n key each.
const OUTPUT_PATH_ERROR_KINDS: [&str; 6] = [
    "not_found",
    "storage_full",
    "permission_denied",
    "read_only",
    "timed_out",
    "other",
];

/// Samples of every kind `sample_kind` distinguishes, plus every value of
/// the fields that select their own i18n key (error kinds, routes, Bigo
/// problems), so each key is rendered at least once.
fn sample_events() -> Vec<NotificationEvent> {
    use srec_core::domain::credential::{CredentialEvent, CredentialScope, UnavailableReason};
    let now = jiff::Timestamp::now();
    let mut events = vec![
        NotificationEvent::StreamOnline {
            streamer_id: "s1".into(),
            streamer_name: "TestStreamer".into(),
            title: "Test title".into(),
            category: Some("Gaming".into()),
            timestamp: now,
        },
        NotificationEvent::StreamOffline {
            streamer_id: "s1".into(),
            streamer_name: "TestStreamer".into(),
            duration_secs: Some(1234.0),
            timestamp: now,
        },
        NotificationEvent::DownloadStarted {
            streamer_id: "s1".into(),
            streamer_name: "TestStreamer".into(),
            session_id: "sess-1".into(),
            timestamp: now,
        },
        NotificationEvent::DownloadCompleted {
            streamer_id: "s1".into(),
            streamer_name: "TestStreamer".into(),
            session_id: "sess-1".into(),
            file_size_bytes: 1024 * 1024 * 100,
            duration_secs: 3600.0,
            timestamp: now,
        },
        NotificationEvent::DownloadError {
            streamer_id: "s1".into(),
            streamer_name: "TestStreamer".into(),
            error_message: "timeout".into(),
            recoverable: true,
            timestamp: now,
        },
        NotificationEvent::SegmentStarted {
            streamer_id: "s1".into(),
            streamer_name: "TestStreamer".into(),
            session_id: "sess-1".into(),
            segment_path: "/rec/seg-0001.mp4".into(),
            segment_index: 1,
            timestamp: now,
        },
        NotificationEvent::SegmentCompleted {
            streamer_id: "s1".into(),
            streamer_name: "TestStreamer".into(),
            session_id: "sess-1".into(),
            segment_path: "/rec/seg-0001.mp4".into(),
            segment_index: 1,
            size_bytes: 1024 * 1024,
            duration_secs: 10.0,
            timestamp: now,
        },
        NotificationEvent::DownloadCancelled {
            streamer_id: "s1".into(),
            streamer_name: "TestStreamer".into(),
            session_id: "sess-1".into(),
            timestamp: now,
        },
        NotificationEvent::DownloadRejected {
            streamer_id: "s1".into(),
            streamer_name: "TestStreamer".into(),
            session_id: "sess-1".into(),
            reason: "circuit breaker open".into(),
            timestamp: now,
        },
        NotificationEvent::PipelineStarted {
            job_id: "job-1".into(),
            job_type: "remux".into(),
            streamer_id: "s1".into(),
            timestamp: now,
        },
        NotificationEvent::PipelineCompleted {
            job_id: "job-1".into(),
            job_type: "remux".into(),
            output_path: Some("/out/final.mp4".into()),
            duration_secs: 42.0,
            timestamp: now,
        },
        NotificationEvent::PipelineFailed {
            job_id: "job-1".into(),
            job_type: "remux".into(),
            error_message: "ffmpeg exited with code 1".into(),
            timestamp: now,
        },
        NotificationEvent::PipelineCancelled {
            job_id: "job-1".into(),
            job_type: "remux".into(),
            pipeline_id: Some("pipeline-xyz".into()),
            timestamp: now,
        },
        NotificationEvent::FatalError {
            streamer_id: "s1".into(),
            streamer_name: "TestStreamer".into(),
            error_type: "ProtocolError".into(),
            message: "connection reset".into(),
            timestamp: now,
        },
        NotificationEvent::StreamerChecksFailing {
            streamer_id: "s1".into(),
            streamer_name: "Alice".into(),
            platform: "twitch".into(),
            last_error: "connection reset".into(),
            consecutive_errors: 3,
            timestamp: now,
        },
        NotificationEvent::StreamerChecksRestored {
            streamer_id: "s1".into(),
            streamer_name: "Alice".into(),
            timestamp: now,
        },
        NotificationEvent::OutOfSpace {
            path: "/rec".into(),
            available_bytes: 1024 * 1024,
            threshold_bytes: 1024 * 1024 * 1024,
            timestamp: now,
        },
        NotificationEvent::PipelineQueueWarning {
            queue_depth: 120,
            threshold: 100,
            timestamp: now,
        },
        NotificationEvent::PipelineQueueCritical {
            queue_depth: 500,
            threshold: 200,
            timestamp: now,
        },
        NotificationEvent::SystemStartup {
            version: "0.2.1".into(),
            timestamp: now,
        },
        NotificationEvent::SystemShutdown {
            reason: "SIGTERM".into(),
            timestamp: now,
        },
        NotificationEvent::BigoNotificationsFailing {
            problem: BigoNotificationProblem::NotArriving,
            message: None,
            timestamp: now,
        },
        NotificationEvent::BigoNotificationsFailing {
            problem: BigoNotificationProblem::Disconnected,
            message: Some("connection reset".into()),
            timestamp: now,
        },
        NotificationEvent::BigoNotificationsFailing {
            problem: BigoNotificationProblem::Disconnected,
            message: None,
            timestamp: now,
        },
        NotificationEvent::BigoNotificationsRestored {
            problem: BigoNotificationProblem::NotArriving,
            timestamp: now,
        },
        NotificationEvent::BigoNotificationsRestored {
            problem: BigoNotificationProblem::Disconnected,
            timestamp: now,
        },
        NotificationEvent::Credential {
            event: CredentialEvent::Refreshed {
                profile_id: None,
                profile_label: None,
                scope: CredentialScope::Platform {
                    platform_id: "bilibili".into(),
                    platform_name: "bilibili".into(),
                },
                platform: "bilibili".into(),
                expires_at: Some(now),
                timestamp: now,
            },
        },
        NotificationEvent::Credential {
            event: CredentialEvent::RefreshFailed {
                profile_id: None,
                profile_label: None,
                scope: CredentialScope::Platform {
                    platform_id: "bilibili".into(),
                    platform_name: "bilibili".into(),
                },
                platform: "bilibili".into(),
                error: "401 Unauthorized".into(),
                requires_relogin: true,
                failure_count: 3,
                timestamp: now,
            },
        },
        NotificationEvent::Credential {
            event: CredentialEvent::RefreshFailed {
                profile_id: None,
                profile_label: None,
                scope: CredentialScope::Platform {
                    platform_id: "bilibili".into(),
                    platform_name: "bilibili".into(),
                },
                platform: "bilibili".into(),
                error: "429 Rate Limited".into(),
                requires_relogin: false,
                failure_count: 1,
                timestamp: now,
            },
        },
        NotificationEvent::Credential {
            event: CredentialEvent::Invalid {
                profile_id: None,
                profile_label: None,
                scope: CredentialScope::Platform {
                    platform_id: "bilibili".into(),
                    platform_name: "bilibili".into(),
                },
                platform: "bilibili".into(),
                reason: "token revoked".into(),
                error_code: Some(-412),
                timestamp: now,
            },
        },
        NotificationEvent::Credential {
            event: CredentialEvent::Unavailable {
                scope: CredentialScope::Platform {
                    platform_id: "bilibili".into(),
                    platform_name: "bilibili".into(),
                },
                platform: "bilibili".into(),
                reason_code: UnavailableReason::LoginRequired,
                timestamp: now,
            },
        },
        NotificationEvent::Credential {
            event: CredentialEvent::ExpiringSoon {
                profile_id: None,
                profile_label: None,
                scope: CredentialScope::Platform {
                    platform_id: "bilibili".into(),
                    platform_name: "bilibili".into(),
                },
                platform: "bilibili".into(),
                expires_at: now,
                days_remaining: 5,
                timestamp: now,
            },
        },
        NotificationEvent::Credential {
            event: CredentialEvent::SessionSaveFailed {
                profile_id: None,
                profile_label: None,
                scope: CredentialScope::Platform {
                    platform_id: "soop".into(),
                    platform_name: "SOOP".into(),
                },
                platform: "soop".into(),
                error: "database is locked".into(),
                timestamp: now,
            },
        },
        NotificationEvent::BaiduPcsReloginFailed {
            config_dir: "default".into(),
            message: "帐号登录失败".into(),
            timestamp: now,
        },
    ];
    events.extend(OUTPUT_PATH_ERROR_KINDS.iter().map(|kind| {
        NotificationEvent::OutputPathInaccessible {
            path: "/rec".into(),
            error_kind: (*kind).into(),
            timestamp: now,
        }
    }));
    events.extend(
        [
            "nvml_unknown_error",
            "driver_mismatch",
            "no_device",
            "timed_out",
            "not_installed",
            "other",
        ]
        .iter()
        .map(|kind| NotificationEvent::GpuUnavailable {
            error_kind: (*kind).into(),
            message: "nvidia-smi failed".into(),
            timestamp: now,
        }),
    );
    events.extend(
        [
            ("direct", None),
            ("system", None),
            ("proxy", Some("Office")),
        ]
        .iter()
        .map(|(route, proxy_name)| NotificationEvent::PlatformThrottled {
            platform: "bilibili".into(),
            route: (*route).into(),
            proxy_name: proxy_name.map(Into::into),
            retry_at: now,
            timestamp: now,
        }),
    );
    events
}

/// Every sample renders a non-empty title and description in both locales,
/// never the raw key that rust-i18n returns for a key missing from every
/// locale, and its event type is in the subscription catalog: channels can
/// subscribe only to cataloged types, so an uncataloged variant would never
/// reach Discord, Telegram, email or webhook channels. A key missing only
/// from zh-CN renders the English text instead, so the two catalogs must
/// define the same keys.
#[test]
fn all_notification_variants_localize_in_both_locales() {
    let events = sample_events();
    assert_eq!(
        events.iter().map(sample_kind).collect::<BTreeSet<_>>(),
        SAMPLE_KINDS.iter().copied().collect::<BTreeSet<_>>(),
        "sample_events must cover every pattern in sample_kinds!"
    );

    for event in &events {
        let event_type = event.event_type();
        for locale in ["en", "zh-CN"] {
            for (part, text) in [
                ("title", event.title_in(locale)),
                ("description", event.description_in(locale)),
            ] {
                assert!(!text.is_empty(), "{locale} {event_type}: empty {part}");
                assert!(
                    !text.starts_with("notification."),
                    "{locale} {event_type}: {part} is the raw key {text:?}"
                );
            }
        }
        assert!(
            canonicalize_subscription_event_name(event_type).is_some(),
            "{event_type} is missing from the subscription catalog"
        );
    }

    for kind in OUTPUT_PATH_ERROR_KINDS {
        let event = NotificationEvent::OutputPathInaccessible {
            path: "/rec".to_string(),
            error_kind: kind.to_string(),
            timestamp: jiff::Timestamp::now(),
        };
        let en = event.description_in("en");
        let zh = event.description_in("zh-CN");
        for description in [&en, &zh] {
            assert!(description.contains("/rec"), "{kind}: {description:?}");
        }
        assert_ne!(en, zh, "{kind} has no zh-CN translation");
    }

    let backend = crate::_rust_i18n_backend();
    let keys = |locale: &str| -> BTreeSet<String> {
        backend
            .messages_for_locale(locale)
            .unwrap_or_else(|| panic!("{locale} catalog is embedded"))
            .into_iter()
            .map(|(key, _)| key.into_owned())
            .collect()
    };
    let en = keys("en");
    let zh = keys("zh-CN");
    assert_eq!(
        en.difference(&zh).collect::<Vec<_>>(),
        Vec::<&String>::new(),
        "keys missing from zh-CN.yml"
    );
    assert_eq!(
        zh.difference(&en).collect::<Vec<_>>(),
        Vec::<&String>::new(),
        "keys missing from en.yml"
    );
}

/// Unavailable notices read as text rather than codes, and
/// profile events name the account beside the owner it belongs to.
#[test]
fn credential_notices_render_reasons_and_account_labels() {
    use srec_core::domain::credential::{
        CredentialEvent, CredentialScope, UNAVAILABLE_REASON_CODES,
    };

    let scope = CredentialScope::Template {
        template_id: "template-1".to_string(),
        template_name: "Night shift".to_string(),
    };
    for locale in ["en", "zh-CN"] {
        for code in UNAVAILABLE_REASON_CODES {
            let notification = NotificationEvent::Credential {
                event: CredentialEvent::Unavailable {
                    scope: scope.clone(),
                    platform: "bilibili".to_string(),
                    reason_code: *code,
                    timestamp: jiff::Timestamp::now(),
                },
            };
            let message = notification.description_in(locale);
            let code = code.as_str();
            assert!(!message.contains(code), "{locale}/{code}: {message}");
            assert!(
                !message.contains("notification."),
                "{locale}/{code}: {message}"
            );
            assert!(message.contains("Night shift"), "{message}");
        }
        let invalid = NotificationEvent::Credential {
            event: CredentialEvent::Invalid {
                profile_id: Some("profile-1".to_string()),
                profile_label: Some("Backup account".to_string()),
                scope: scope.clone(),
                platform: "bilibili".to_string(),
                reason: "login_required".to_string(),
                error_code: None,
                timestamp: jiff::Timestamp::now(),
            },
        };
        let title = invalid.title_in(locale);
        let description = invalid.description_in(locale);
        assert!(title.contains("Template: Night shift"), "{title}");
        assert!(title.contains("Backup account"), "{title}");
        assert!(
            description.contains("Backup account"),
            "{locale}: {description}"
        );
    }
}
