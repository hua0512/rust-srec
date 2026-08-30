use super::*;

impl<CR, SR> PipelineManager<CR, SR>
where
    CR: ConfigRepository + Send + Sync + 'static,
    SR: StreamerRepository + Send + Sync + 'static,
{
    pub async fn handle_download_event(&self, event: DownloadManagerEvent) -> Result<()> {
        match event {
            DownloadManagerEvent::Progress(DownloadProgressEvent::SegmentCompleted {
                streamer_id,
                session_id,
                segment_path,
                segment_index,
                started_at,
                completed_at,
                duration_secs,
                size_bytes,
                split_reason_code,
                split_reason_details_json,
                ..
            }) => {
                debug!(
                    "Segment completed for {} (session: {}): {}",
                    streamer_id, session_id, segment_path
                );
                let session_segment = crate::database::models::SessionSegmentDbModel::new(
                    &session_id,
                    segment_index,
                    &segment_path,
                    duration_secs,
                    size_bytes,
                    SessionSegmentLifecycle::new(
                        started_at.as_ref().map(chrono::DateTime::timestamp_millis),
                        Some(completed_at.timestamp_millis()),
                    ),
                    SessionSegmentSplitReason::new(
                        split_reason_code.clone(),
                        split_reason_details_json.clone(),
                    ),
                );
                self.persist_segment(&session_segment).await?;

                let merged_config = if let Some(config_service) = &self.config_service {
                    config_service
                        .get_config_for_streamer(&streamer_id)
                        .await
                        .ok()
                } else {
                    None
                };

                let pipeline_config = merged_config.as_ref().and_then(|c| c.pipeline.clone());
                let session_complete_pipeline = merged_config
                    .as_ref()
                    .and_then(|c| c.session_complete_pipeline.clone());
                let paired_segment_pipeline = merged_config
                    .as_ref()
                    .and_then(|c| c.paired_segment_pipeline.clone());
                let danmu_enabled = merged_config
                    .as_ref()
                    .map(|c| c.record_danmu)
                    .unwrap_or(false);

                // Avoid creating the built-in thumbnail DAG when the configured
                // segment pipeline already creates one.
                let pipeline_has_thumbnail = if let Some(dag) = &pipeline_config {
                    dag.steps.iter().any(|node| match &node.step {
                        PipelineStep::Inline { processor, .. } => processor == "thumbnail",
                        // Match exact preset names or those with thumbnail_ prefix
                        PipelineStep::Preset { name } => {
                            name == "thumbnail"
                                || name.starts_with("thumbnail_")
                                || name == "thumbnail_native"
                                || name == "thumbnail_hd"
                        }
                        // Match exact workflow names or those with thumbnail prefix
                        PipelineStep::Workflow { name } => {
                            name == "thumbnail" || name.starts_with("thumbnail_")
                        }
                    })
                } else {
                    false
                };

                // Check if auto_thumbnail is enabled in global settings (defaults to true)
                let auto_thumbnail_enabled = merged_config
                    .as_ref()
                    .map(|c| c.auto_thumbnail)
                    .unwrap_or(true);

                // Generate automatic thumbnail for first segment only if:
                // 1. This is the first segment (segment_index == 0)
                // 2. User's pipeline doesn't already include a thumbnail step
                // 3. Auto thumbnail generation is enabled in global settings
                if segment_index == 0 && !pipeline_has_thumbnail && auto_thumbnail_enabled {
                    self.maybe_create_automatic_thumbnail_dag(
                        &streamer_id,
                        &session_id,
                        &segment_path,
                    )
                    .await;
                }

                let mut commands = self
                    .pipeline_coordinator
                    .apply_event(PipelineCoordinationEvent::ConfigureSession {
                        session_id: session_id.clone(),
                        streamer_id: streamer_id.clone(),
                        danmu_enabled,
                        segment_pipeline: pipeline_config,
                        paired_segment_pipeline: paired_segment_pipeline.filter(|_| danmu_enabled),
                        session_complete_pipeline,
                    })
                    .await;
                commands.extend(
                    self.pipeline_coordinator
                        .apply_event(PipelineCoordinationEvent::VideoSegmentCompleted {
                            session_id,
                            streamer_id,
                            segment_index,
                            path: PathBuf::from(segment_path),
                        })
                        .await,
                );
                self.execute_pipeline_commands(commands).await;
            }
            DownloadManagerEvent::Terminal(_) => {
                // Terminal events are owned by `session::SessionLifecycle`,
                // which converts them into `SessionTransition::Ended`. The
                // session-complete trigger is driven by `handle_session_transition`.
            }
            // All other Progress variants are no-ops here (DownloadStarted,
            // Progress, SegmentStarted, ConfigUpdated, ConfigUpdateFailed).
            // The segment pipeline only reacts to SegmentCompleted. Terminal
            // events are consumed by SessionLifecycle and replayed here as
            // SessionTransition values.
            DownloadManagerEvent::Progress(
                DownloadProgressEvent::DownloadQueued { .. }
                | DownloadProgressEvent::DownloadDequeued { .. }
                | DownloadProgressEvent::DownloadStarted { .. }
                | DownloadProgressEvent::Progress { .. }
                | DownloadProgressEvent::SegmentStarted { .. }
                | DownloadProgressEvent::ConfigUpdated { .. }
                | DownloadProgressEvent::ConfigUpdateFailed { .. },
            ) => {}
        }
        Ok(())
    }

    /// Handle a session lifecycle transition. Only
    /// [`SessionTransition::Ended`](crate::session::SessionTransition::Ended)
    /// is acted on. The session-complete pipeline fires iff
    /// [`TerminalCause::should_run_session_complete_pipeline`](crate::session::TerminalCause::should_run_session_complete_pipeline)
    /// returns true for the cause carried by the transition.
    pub async fn handle_session_transition(&self, event: crate::session::SessionTransition) {
        let crate::session::SessionTransition::Ended {
            session_id,
            streamer_id,
            cause,
            ..
        } = event
        else {
            return;
        };

        info!(
            streamer_id = %streamer_id,
            session_id = %session_id,
            cause = %cause.as_str(),
            "Session ended"
        );

        let mut commands = Vec::new();
        let should_run_session_complete = cause.should_run_session_complete_pipeline();
        if let Some(config_service) = &self.config_service
            && let Ok(config) = config_service.get_config_for_streamer(&streamer_id).await
        {
            commands.extend(
                self.pipeline_coordinator
                    .apply_event(PipelineCoordinationEvent::ConfigureSession {
                        session_id: session_id.clone(),
                        streamer_id: streamer_id.clone(),
                        danmu_enabled: config.record_danmu,
                        segment_pipeline: config.pipeline.clone(),
                        paired_segment_pipeline: config
                            .paired_segment_pipeline
                            .clone()
                            .filter(|_| config.record_danmu),
                        session_complete_pipeline: config.session_complete_pipeline.clone(),
                    })
                    .await,
            );
        }

        commands.extend(
            self.pipeline_coordinator
                .apply_event(PipelineCoordinationEvent::SessionEnded {
                    session_id: session_id.clone(),
                    streamer_id,
                    should_run_session_complete,
                })
                .await,
        );

        if should_run_session_complete {
            // `SessionLifecycle::end_session_only` / `end_for_disable` /
            // `end_for_out_of_schedule` await the `end_time` DB commit
            // before broadcasting `SessionTransition::Ended` (the ordering
            // is enforced by
            // `session::lifecycle::tests::end_for_disable_broadcast_after_commit_and_memory_update`),
            // so by the time we read this transition the row is already
            // persisted. Apply `SessionEndPersisted` directly instead of
            // re-reading `sessions.end_time`.
            commands.extend(
                self.pipeline_coordinator
                    .apply_event(PipelineCoordinationEvent::SessionEndPersisted {
                        session_id: session_id.clone(),
                    })
                    .await,
            );
        } else {
            debug!(
                session_id = %session_id,
                cause = %cause.as_str(),
                "Terminal cause does not trigger session-complete pipeline"
            );
        }

        self.execute_pipeline_commands(commands).await;
    }

    /// Handle danmu service events.
    ///
    /// Processes `DanmuEvent::SegmentCompleted` events by:
    /// 1. Persisting the danmu segment to the database as a media output
    /// 2. Creating pipeline jobs if a pipeline is configured for the streamer
    pub async fn handle_danmu_event(&self, event: crate::danmu::DanmuEvent) -> Result<()> {
        use crate::danmu::DanmuControlEvent;
        use crate::danmu::DanmuEvent;
        use crate::database::models::TitleEntry;

        match event {
            DanmuEvent::CollectionStarted {
                session_id,
                streamer_id,
            } => {
                let mut commands = Vec::new();
                if let Some(config_service) = &self.config_service
                    && let Ok(config) = config_service.get_config_for_streamer(&streamer_id).await
                {
                    commands.extend(
                        self.pipeline_coordinator
                            .apply_event(PipelineCoordinationEvent::ConfigureSession {
                                session_id: session_id.clone(),
                                streamer_id: streamer_id.clone(),
                                danmu_enabled: config.record_danmu,
                                segment_pipeline: config.pipeline.clone(),
                                paired_segment_pipeline: config
                                    .paired_segment_pipeline
                                    .clone()
                                    .filter(|_| config.record_danmu),
                                session_complete_pipeline: config.session_complete_pipeline.clone(),
                            })
                            .await,
                    );
                }
                commands.extend(
                    self.pipeline_coordinator
                        .apply_event(PipelineCoordinationEvent::DanmuCollectionStarted {
                            session_id,
                            streamer_id,
                        })
                        .await,
                );
                self.execute_pipeline_commands(commands).await;
            }
            DanmuEvent::Control {
                session_id,
                streamer_id,
                control:
                    DanmuControlEvent::RoomInfoChanged {
                        title: Some(title), ..
                    },
                ..
            } => {
                // Apply title changes immediately so session titles stay accurate even when
                // the monitor polling interval is long.
                let Some(repo) = &self.session_repo else {
                    return Ok(());
                };
                match repo.get_session(&session_id).await {
                    Ok(session) => {
                        let now = chrono::Utc::now();
                        let mut titles: Vec<TitleEntry> = match session.titles.as_deref() {
                            Some(json) => serde_json::from_str(json).unwrap_or_default(),
                            None => Vec::new(),
                        };

                        let needs_update = titles.last().map(|t| t.title != title).unwrap_or(true);
                        if needs_update {
                            titles.push(TitleEntry {
                                ts: now.timestamp_millis(),
                                title: title.clone(),
                            });
                            match serde_json::to_string(&titles) {
                                Ok(updated) => {
                                    if let Err(e) =
                                        repo.update_session_titles(&session_id, &updated).await
                                    {
                                        warn!(
                                            streamer_id = %streamer_id,
                                            session_id = %session_id,
                                            error = %e,
                                            "Failed to persist session title update from danmu control event"
                                        );
                                    }
                                }
                                Err(e) => warn!(
                                    streamer_id = %streamer_id,
                                    session_id = %session_id,
                                    error = %e,
                                    "Failed to serialize session titles for danmu control title update"
                                ),
                            }
                        }
                    }
                    Err(e) => warn!(
                        streamer_id = %streamer_id,
                        session_id = %session_id,
                        error = %e,
                        "Failed to load session for danmu control title update"
                    ),
                }
            }
            DanmuEvent::CollectionStopped { session_id, .. } => {
                let commands = self
                    .pipeline_coordinator
                    .apply_event(PipelineCoordinationEvent::DanmuCollectionStopped { session_id })
                    .await;
                self.execute_pipeline_commands(commands).await;
            }
            DanmuEvent::SegmentCompleted {
                streamer_id,
                session_id,
                segment_id,
                output_path,
                message_count,
            } => {
                let segment_path = output_path.to_string_lossy().to_string();

                debug!(
                    "Danmu segment completed for {} (session: {}): {} ({} messages)",
                    streamer_id, session_id, segment_path, message_count
                );

                // Check if the danmu file still exists before processing.
                // The file may have been deleted if the corresponding video segment was too small.
                if !output_path.exists() {
                    debug!("Danmu segment file no longer exists: {}", segment_path);
                    return Ok(());
                }

                let Some(segment_index) = parse_segment_index_from_danmu(&segment_id, &output_path)
                else {
                    warn!(
                        session_id = %session_id,
                        segment_id = %segment_id,
                        path = %output_path.display(),
                        "Failed to parse danmu segment_index; skipping danmu pipeline coordination for this segment"
                    );
                    return Ok(());
                };

                // Persist danmu segment to database as a media output
                self.persist_danmu_segment(&session_id, &segment_path, message_count)
                    .await?;

                let merged_config = if let Some(config_service) = &self.config_service {
                    config_service
                        .get_config_for_streamer(&streamer_id)
                        .await
                        .ok()
                } else {
                    None
                };

                let pipeline_config = merged_config.as_ref().and_then(|c| c.pipeline.clone());
                let session_complete_pipeline = merged_config
                    .as_ref()
                    .and_then(|c| c.session_complete_pipeline.clone());
                let paired_segment_pipeline = merged_config
                    .as_ref()
                    .and_then(|c| c.paired_segment_pipeline.clone());
                let danmu_enabled = merged_config
                    .as_ref()
                    .map(|c| c.record_danmu)
                    .unwrap_or(false);

                let mut commands = self
                    .pipeline_coordinator
                    .apply_event(PipelineCoordinationEvent::ConfigureSession {
                        session_id: session_id.clone(),
                        streamer_id: streamer_id.clone(),
                        danmu_enabled,
                        segment_pipeline: pipeline_config,
                        paired_segment_pipeline: paired_segment_pipeline.filter(|_| danmu_enabled),
                        session_complete_pipeline,
                    })
                    .await;
                commands.extend(
                    self.pipeline_coordinator
                        .apply_event(PipelineCoordinationEvent::DanmuSegmentCompleted {
                            session_id,
                            streamer_id,
                            segment_index,
                            path: output_path,
                        })
                        .await,
                );
                self.execute_pipeline_commands(commands).await;
            }
            _ => {}
        }
        Ok(())
    }

    /// Check if session already has a thumbnail by querying media outputs.
    pub(super) async fn session_has_thumbnail(&self, session_id: &str) -> bool {
        if let Some(repo) = &self.session_repo
            && let Ok(outputs) = repo.get_media_outputs_for_session(session_id).await
        {
            return outputs
                .iter()
                .any(|o| o.file_type == MediaFileType::Thumbnail.as_str());
        }
        false
    }

    /// Create the built-in thumbnail DAG for the first segment when needed.
    pub(super) async fn maybe_create_automatic_thumbnail_dag(
        &self,
        streamer_id: &str,
        session_id: &str,
        segment_path: &str,
    ) {
        // Check if session already has a thumbnail (reuses existing query)
        if self.session_has_thumbnail(session_id).await {
            debug!("Session {} already has a thumbnail, skipping", session_id);
            return;
        }

        // Use thumbnail_native preset
        let step = PipelineStep::Preset {
            name: "thumbnail_native".to_string(),
        };

        // Create DAG definition
        let dag_step = DagStep::new("thumbnail", step);
        let dag_def = DagPipelineDefinition::new("Automatic Thumbnail", vec![dag_step]);

        if let Err(e) = self
            .create_dag_pipeline(
                session_id,
                streamer_id,
                vec![segment_path.to_string()],
                dag_def,
            )
            .await
        {
            tracing::error!(
                "Failed to create automatic thumbnail pipeline for session {}: {}",
                session_id,
                e
            );
        } else {
            debug!(
                "Created automatic thumbnail pipeline for first segment of session {}",
                session_id
            );
        }
    }

    /// Persist a downloaded segment to the database.
    pub(super) async fn persist_segment(
        &self,
        segment: &crate::database::models::SessionSegmentDbModel,
    ) -> Result<()> {
        if let Some(repo) = &self.session_repo {
            let output = MediaOutputDbModel::new(
                &segment.session_id,
                &segment.file_path,
                MediaFileType::Video, // Assuming video segments for now
                segment.size_bytes,
            );

            repo.create_segment_output(&output, segment).await?;
            debug!("Persisted segment for session {}", segment.session_id);
        }
        Ok(())
    }

    /// Persist a danmu segment to the database.
    pub(super) async fn persist_danmu_segment(
        &self,
        session_id: &str,
        path: &str,
        message_count: u64,
    ) -> Result<()> {
        if let Some(repo) = &self.session_repo {
            // Get actual file size from disk
            let size_bytes = match tokio::fs::metadata(path).await {
                Ok(metadata) => metadata.len() as i64,
                Err(e) => {
                    tracing::warn!(
                        "Failed to get file size for danmu segment {}: {}, using 0",
                        path,
                        e
                    );
                    0
                }
            };

            let output =
                MediaOutputDbModel::new(session_id, path, MediaFileType::DanmuXml, size_bytes);

            repo.create_media_output(&output).await?;
            debug!(
                "Persisted danmu segment for session {} ({} messages, {} bytes)",
                session_id, message_count, size_bytes
            );
        }
        Ok(())
    }

    /// Drop the `DANMU_XML` media output for `path`, if one was persisted.
    ///
    /// The discard flow in `DownloadEventProcessor` suppresses an undersized
    /// segment's danmu by consuming `discarded_segment_keys` when
    /// `DanmuEvent::SegmentCompleted` arrives *after* the video's. The danmu event
    /// can also arrive first — `CollectionRunner::shutdown` finalizes an open
    /// segment when the stream closes or the transport fails, both of which happen
    /// while the video segment is still recording — and in that order
    /// `persist_danmu_segment` has already run. This removes the row so a
    /// discarded segment leaves neither a file nor a record of one.
    pub(crate) async fn remove_danmu_segment_output(&self, session_id: &str, path: &str) {
        let Some(repo) = &self.session_repo else {
            return;
        };

        let outputs = match repo.get_media_outputs_for_session(session_id).await {
            Ok(outputs) => outputs,
            Err(error) => {
                tracing::warn!(
                    session_id,
                    path,
                    %error,
                    "Failed to look up danmu media output for a discarded segment"
                );
                return;
            }
        };

        for output in outputs
            .into_iter()
            .filter(|output| output.file_type == MediaFileType::DanmuXml.as_str())
            .filter(|output| output.file_path == path)
        {
            match repo.delete_media_output(&output.id).await {
                Ok(()) => debug!(
                    session_id,
                    path, "Removed danmu media output for discarded segment"
                ),
                Err(error) => tracing::warn!(
                    session_id,
                    path,
                    %error,
                    "Failed to remove danmu media output for a discarded segment"
                ),
            }
        }
    }
}
