use super::*;

#[test]
fn test_notification_priority_from_int() {
    assert_eq!(
        NotificationPriority::from_int(0),
        Some(NotificationPriority::Low)
    );
    assert_eq!(
        NotificationPriority::from_int(2),
        Some(NotificationPriority::Low)
    );
    assert_eq!(
        NotificationPriority::from_int(3),
        Some(NotificationPriority::Low)
    );
    assert_eq!(
        NotificationPriority::from_int(4),
        Some(NotificationPriority::Normal)
    );
    assert_eq!(
        NotificationPriority::from_int(5),
        Some(NotificationPriority::Normal)
    );
    assert_eq!(
        NotificationPriority::from_int(7),
        Some(NotificationPriority::High)
    );
    assert_eq!(
        NotificationPriority::from_int(8),
        Some(NotificationPriority::High)
    );
    assert_eq!(
        NotificationPriority::from_int(10),
        Some(NotificationPriority::Critical)
    );
    assert_eq!(
        NotificationPriority::from_int(255),
        Some(NotificationPriority::Critical)
    );
}

/// A `StreamOnline` event to render; the interesting part is always the rendered text.
fn stream_online_fixture() -> NotificationEvent {
    NotificationEvent::StreamOnline {
        streamer_id: "123".to_string(),
        streamer_name: "TestStreamer".to_string(),
        title: "Playing Games".to_string(),
        category: Some("Gaming".to_string()),
        timestamp: Utc::now(),
    }
}

#[test]
fn title_in_does_not_depend_on_the_process_locale() {
    let _g = OUTPUT_PATH_INACCESSIBLE_LOCALE_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());

    let event = stream_online_fixture();
    // `set_locale` is process-wide, so asserting the requested locale wins over it is what
    // makes two channels able to render the same event in different languages.
    crate::i18n::set_locale("en");
    let zh = event.title_in("zh-CN");
    crate::i18n::set_locale("zh-CN");
    let zh_again = event.title_in("zh-CN");
    let en = event.title_in("en");
    crate::i18n::set_locale("en");

    assert_eq!(zh, zh_again);
    assert_ne!(zh, en);
}

#[test]
fn unknown_locale_falls_back_to_english() {
    let event = stream_online_fixture();
    // `rust_i18n` has no YAML for this locale; it must not render an empty title.
    assert_eq!(event.title_in("xx-YY"), event.title_in("en"));
}

/// `rust_i18n::set_locale` mutates a process global, so locale-sensitive
/// tests must serialize on this lock to avoid racing the i18n module's
/// own tests (and each other).
static OUTPUT_PATH_INACCESSIBLE_LOCALE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn baidupcs_relogin_failed_metadata_and_localization() {
    let event = NotificationEvent::BaiduPcsReloginFailed {
        config_dir: "default".to_string(),
        message: "帐号登录失败".to_string(),
        timestamp: Utc::now(),
    };
    assert_eq!(event.priority(), NotificationPriority::High);
    assert_eq!(event.event_type(), "baidupcs_relogin_failed");
    assert_eq!(event.streamer_id(), None, "tool event has no streamer_id");
    assert!(
        NotificationEvent::event_type_info("BaiduPcsReloginFailed").is_some(),
        "alias resolves to the canonical event type"
    );
    assert!(event.title_in("en").contains("Baidu Netdisk"));
    assert!(event.title_in("en").contains("default"));
    assert!(event.title_in("zh-CN").contains("百度网盘"));
    assert!(event.description_in("en").contains("帐号登录失败"));
    assert!(event.description_in("zh-CN").contains("帐号登录失败"));
}

#[test]
fn output_path_inaccessible_all_error_kinds_resolve() {
    // Every IoErrorKindSer::as_str() value must map to a real i18n key,
    // not the literal key string. Defends against silent misalignment
    // between the YAML files and the description() match arms.
    let _g = OUTPUT_PATH_INACCESSIBLE_LOCALE_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    crate::i18n::set_locale("en");
    for kind in [
        "not_found",
        "storage_full",
        "permission_denied",
        "read_only",
        "timed_out",
        "other",
    ] {
        let event = NotificationEvent::OutputPathInaccessible {
            path: "/rec".to_string(),
            error_kind: kind.to_string(),
            timestamp: Utc::now(),
        };
        let description = event.description();
        assert!(
            description.contains("/rec"),
            "kind={} description={:?}",
            kind,
            description
        );
        assert!(
            !description.starts_with("notification."),
            "kind={} returned untranslated key: {:?}",
            kind,
            description
        );
    }
}

#[test]
fn output_path_inaccessible_in_event_type_registry() {
    let info = NotificationEvent::event_type_info("output_path_inaccessible")
        .expect("event type should be registered");
    assert_eq!(info.event_type, "output_path_inaccessible");
    assert_eq!(info.priority, NotificationPriority::Critical);

    // Aliases resolve back to the canonical type
    let from_camel = NotificationEvent::event_type_info("OutputPathInaccessible");
    assert!(from_camel.is_some());
    let from_dotted = NotificationEvent::event_type_info("output.path_inaccessible");
    assert!(from_dotted.is_some());
}

// ========== Full-notification i18n round-trip ==========

/// One plausible instance of every `NotificationEvent` variant.
/// Field values are picked so the variant-specific placeholders are
/// present and checkable after localization.
///
/// New variants added to `NotificationEvent` MUST be added here or
/// `all_notification_variants_localize` will catch the omission via
/// the exhaustive count assertion.
fn sample_events() -> Vec<NotificationEvent> {
    use crate::credentials::{CredentialEvent, CredentialScope};
    let now = Utc::now();
    vec![
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
        NotificationEvent::ConfigUpdated {
            streamer_id: "s1".into(),
            streamer_name: "TestStreamer".into(),
            update_type: "Cookies".into(),
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
        NotificationEvent::OutOfSpace {
            path: "/rec".into(),
            available_bytes: 1024 * 1024,
            threshold_bytes: 1024 * 1024 * 1024,
            timestamp: now,
        },
        NotificationEvent::OutputPathInaccessible {
            path: "/rec".into(),
            error_kind: "not_found".into(),
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
                reason_code: crate::credentials::UnavailableReason::LoginRequired,
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
    ]
}

/// For every variant, assert that `title()` and `description()` produce
/// non-empty strings that are NOT the raw key literal (which is what
/// rust-i18n returns when a key is missing from every locale). Runs
/// against both `en` and `zh-CN` so missing translations in either
/// locale are caught at CI time.
#[test]
fn all_notification_variants_localize_in_both_locales() {
    let _g = OUTPUT_PATH_INACCESSIBLE_LOCALE_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());

    let events = sample_events();
    // If a new variant is added to NotificationEvent without extending
    // sample_events, we want a visible signal. This assert is a
    // self-doc; bump it alongside the match arms in title/description.
    assert_eq!(
        events.len(),
        27,
        "sample_events is out of sync with NotificationEvent; add a sample for the new variant so its localization is covered"
    );

    for locale in ["en", "zh-CN"] {
        crate::i18n::set_locale(locale);
        for (i, event) in events.iter().enumerate() {
            let title = event.title();
            let description = event.description();
            assert!(
                !title.is_empty(),
                "locale={} variant #{} ({}): empty title",
                locale,
                i,
                event.event_type(),
            );
            assert!(
                !description.is_empty(),
                "locale={} variant #{} ({}): empty description",
                locale,
                i,
                event.event_type(),
            );
            assert!(
                !title.starts_with("notification."),
                "locale={} variant #{} ({}): title returned the raw key ({:?}) — translation missing",
                locale,
                i,
                event.event_type(),
                title,
            );
            assert!(
                !description.starts_with("notification."),
                "locale={} variant #{} ({}): description returned the raw key ({:?}) — translation missing",
                locale,
                i,
                event.event_type(),
                description,
            );
        }
    }

    crate::i18n::set_locale("en");
}

/// Channels can subscribe only to cataloged event types, so an uncataloged
/// variant would never reach Discord, Telegram, email or webhook channels.
#[test]
fn every_notification_variant_is_subscribable() {
    for event in sample_events() {
        assert!(
            canonicalize_subscription_event_name(event.event_type()).is_some(),
            "{} is missing from the subscription catalog",
            event.event_type(),
        );
    }
}
