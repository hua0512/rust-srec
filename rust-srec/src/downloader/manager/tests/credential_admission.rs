use super::*;

fn binding() -> crate::credentials::CredentialBinding {
    crate::credentials::CredentialBinding {
        identity: crate::credentials::CredentialIdentity::Anonymous,
        revision: 0,
        policy: crate::credentials::ResolvedCredentialPolicy::new(
            "platform".into(),
            crate::credentials::CredentialOwner::Platform {
                platform_id: "platform".into(),
            },
            crate::credentials::CredentialSelection::None,
        )
        .unwrap(),
        epoch: 1,
    }
}

#[tokio::test]
async fn binding_is_revalidated_after_maintenance_admission_wait() {
    let directory = tempfile::tempdir().unwrap();
    let manager = DownloadManager::new();
    let changed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let callback_changed = changed.clone();
    manager.set_credential_start_validator(Arc::new(move |_, _, _| {
        let changed = callback_changed.clone();
        Box::pin(async move {
            if changed.load(Ordering::SeqCst) {
                Err(crate::credentials::ProfileError::SourceChanged.into())
            } else {
                Ok(())
            }
        })
    }));
    let mut config =
        test_download_config(directory.path().to_path_buf(), "changed-during-admission");
    config.managed_credentials = true;
    config.credential_binding = Some(binding());
    let slot = manager
        .acquire_slot(
            AcquireRequest {
                session_id: config.session_id.clone(),
                streamer_id: config.streamer_id.clone(),
                streamer_name: config.streamer_name.clone(),
                engine_type: EngineType::Ffmpeg,
                priority: Priority::Normal,
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let maintenance = manager.try_admit_maintenance(0).unwrap();
    let engine = EngineHandle {
        engine: Arc::new(ScriptedSegmentEngine::with_shutdown_tail(
            Vec::new(),
            Vec::new(),
        )),
        engine_type: EngineType::Ffmpeg,
        engine_key: EngineKey::global(EngineType::Ffmpeg),
    };
    let mut start = Box::pin(manager.start_with_slot(slot, config, engine));
    assert!(futures::poll!(start.as_mut()).is_pending());
    changed.store(true, Ordering::SeqCst);
    drop(maintenance);
    let outcome = tokio::time::timeout(Duration::from_secs(2), start)
        .await
        .unwrap();
    assert!(matches!(
        outcome,
        Err(crate::Error::CredentialProfile(
            crate::credentials::ProfileError::SourceChanged
        ))
    ));
    assert_eq!(manager.active_count(), 0);
}
