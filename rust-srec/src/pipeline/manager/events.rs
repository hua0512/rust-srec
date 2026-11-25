use super::*;

impl PipelineManager {
    /// Apply one post-commit recorder notification to the pipeline coordinator.
    ///
    /// Video and danmu segment database rows are already persisted before this
    /// method is invoked, so this path drives only thumbnail / DAG /
    /// session-complete coordination.
    pub async fn handle_recorder_notification(
        &self,
        notification: PipelineNotification,
    ) -> Result<()> {
        match notification {
            PipelineNotification::VideoSegmentCompleted {
                session_id,
                streamer_id,
                segment_index,
                path,
                auto_thumbnail_candidate,
            } => {
                let segment_path = path.to_string_lossy().to_string();
                let merged_config = if let Some(config_port) = &self.config_port {
                    config_port
                        .get_session_pipeline_config(&streamer_id)
                        .await
                        .ok()
                } else {
                    None
                };

                let pipeline_config = merged_config
                    .as_ref()
                    .and_then(|c| c.segment_pipeline.clone());
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

                let pipeline_has_thumbnail = if let Some(dag) = &pipeline_config {
                    dag.steps.iter().any(|node| match &node.step {
                        PipelineStep::Inline { processor, .. } => processor == "thumbnail",
                        PipelineStep::Preset { name } => {
                            name == "thumbnail"
                                || name.starts_with("thumbnail_")
                                || name == "thumbnail_native"
                                || name == "thumbnail_hd"
                        }
                        PipelineStep::Workflow { name } => {
                            name == "thumbnail" || name.starts_with("thumbnail_")
                        }
                    })
                } else {
                    false
                };

                let auto_thumbnail_enabled = merged_config
                    .as_ref()
                    .map(|c| c.auto_thumbnail)
                    .unwrap_or(true);

                if auto_thumbnail_candidate && !pipeline_has_thumbnail && auto_thumbnail_enabled {
                    self.maybe_create_automatic_thumbnail_dag(
                        &streamer_id,
                        &session_id,
                        &segment_path,
                    )
                    .await;
                }

                let mut commands = Vec::new();
                if merged_config.is_some() {
                    commands.extend(self.pipeline_coordinator.apply_event(
                        PipelineCoordinationEvent::ConfigureSession {
                            session_id: session_id.clone(),
                            streamer_id: streamer_id.clone(),
                            danmu_enabled,
                            segment_pipeline: pipeline_config,
                            paired_segment_pipeline:
                                paired_segment_pipeline.filter(|_| danmu_enabled),
                            session_complete_pipeline,
                        },
                    ));
                }
                commands.extend(self.pipeline_coordinator.apply_event(
                    PipelineCoordinationEvent::VideoSegmentCompleted {
                        session_id,
                        streamer_id,
                        segment_index,
                        path,
                    },
                ));
                self.execute_pipeline_commands(commands).await;
            }
            PipelineNotification::DanmuCollectionStarted {
                session_id,
                streamer_id,
            } => {
                let mut commands = Vec::new();
                if let Some(config_port) = &self.config_port
                    && let Ok(config) = config_port.get_session_pipeline_config(&streamer_id).await
                {
                    commands.extend(
                        self.pipeline_coordinator.apply_event(
                            PipelineCoordinationEvent::ConfigureSession {
                                session_id: session_id.clone(),
                                streamer_id: streamer_id.clone(),
                                danmu_enabled: config.record_danmu,
                                segment_pipeline: config.segment_pipeline.clone(),
                                paired_segment_pipeline: config
                                    .paired_segment_pipeline
                                    .clone()
                                    .filter(|_| config.record_danmu),
                                session_complete_pipeline: config.session_complete_pipeline.clone(),
                            },
                        ),
                    );
                }
                commands.extend(self.pipeline_coordinator.apply_event(
                    PipelineCoordinationEvent::DanmuCollectionStarted {
                        session_id,
                        streamer_id,
                    },
                ));
                self.execute_pipeline_commands(commands).await;
            }
            PipelineNotification::DanmuSegmentCompleted {
                session_id,
                streamer_id,
                segment_index,
                path,
                ..
            } => {
                let merged_config = if let Some(config_port) = &self.config_port {
                    config_port
                        .get_session_pipeline_config(&streamer_id)
                        .await
                        .ok()
                } else {
                    None
                };

                let pipeline_config = merged_config
                    .as_ref()
                    .and_then(|c| c.segment_pipeline.clone());
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

                let mut commands = Vec::new();
                if merged_config.is_some() {
                    commands.extend(self.pipeline_coordinator.apply_event(
                        PipelineCoordinationEvent::ConfigureSession {
                            session_id: session_id.clone(),
                            streamer_id: streamer_id.clone(),
                            danmu_enabled,
                            segment_pipeline: pipeline_config,
                            paired_segment_pipeline:
                                paired_segment_pipeline.filter(|_| danmu_enabled),
                            session_complete_pipeline,
                        },
                    ));
                }
                commands.extend(self.pipeline_coordinator.apply_event(
                    PipelineCoordinationEvent::DanmuSegmentCompleted {
                        session_id,
                        streamer_id,
                        segment_index,
                        path,
                    },
                ));
                self.execute_pipeline_commands(commands).await;
            }
            PipelineNotification::DanmuCollectionStopped { session_id, .. } => {
                let commands = self
                    .pipeline_coordinator
                    .apply_event(PipelineCoordinationEvent::DanmuCollectionStopped { session_id });
                self.execute_pipeline_commands(commands).await;
            }
            PipelineNotification::SessionEnded {
                session_id,
                streamer_id,
                cause,
                should_run_session_complete,
            } => {
                info!(
                    streamer_id = %streamer_id,
                    session_id = %session_id,
                    cause = %cause,
                    "Session ended"
                );

                let mut commands = Vec::new();
                if should_run_session_complete {
                    let config = match &self.config_port {
                        Some(port) => match port.get_session_pipeline_config(&streamer_id).await {
                            Ok(config) => Some(config),
                            Err(error) => {
                                if self
                                    .pipeline_coordinator
                                    .has_configured_session(&session_id)
                                {
                                    warn!(
                                        %session_id,
                                        %streamer_id,
                                        %error,
                                        "Failed to refresh streamer config at session end; using the configuration the session already captured"
                                    );
                                    None
                                } else {
                                    warn!(
                                        %session_id,
                                        %streamer_id,
                                        %error,
                                        "Session pipeline configuration remains unresolved"
                                    );
                                    return Ok(());
                                }
                            }
                        },
                        None => None,
                    };
                    if let Some(config) = config {
                        commands.extend(
                            self.pipeline_coordinator.apply_event(
                                PipelineCoordinationEvent::ConfigureSession {
                                    session_id: session_id.clone(),
                                    streamer_id: streamer_id.clone(),
                                    danmu_enabled: config.record_danmu,
                                    segment_pipeline: config.segment_pipeline.clone(),
                                    paired_segment_pipeline: config
                                        .paired_segment_pipeline
                                        .clone()
                                        .filter(|_| config.record_danmu),
                                    session_complete_pipeline: config
                                        .session_complete_pipeline
                                        .clone(),
                                },
                            ),
                        );
                    }
                }

                commands.extend(self.pipeline_coordinator.apply_event(
                    PipelineCoordinationEvent::SessionEnded {
                        session_id: session_id.clone(),
                        streamer_id,
                        should_run_session_complete,
                    },
                ));

                if should_run_session_complete {
                    commands.extend(self.pipeline_coordinator.apply_event(
                        PipelineCoordinationEvent::SessionEndPersisted {
                            session_id: session_id.clone(),
                        },
                    ));
                } else {
                    debug!(
                        session_id = %session_id,
                        cause = %cause,
                        "Terminal cause does not trigger session-complete pipeline"
                    );
                }

                self.execute_pipeline_commands(commands).await;
                if should_run_session_complete {
                    self.record_session_pipeline_settlement(&session_id).await;
                } else {
                    self.mark_session_complete_dispatched(&session_id).await;
                }
            }
        }
        Ok(())
    }

    /// Check if session already has a thumbnail by querying media outputs.
    pub(super) async fn session_has_thumbnail(&self, session_id: &str) -> bool {
        let Some(port) = &self.session_port else {
            return false;
        };
        matches!(port.session_has_thumbnail(session_id).await, Ok(true))
    }

    /// Create the built-in thumbnail DAG for the first segment when needed.
    pub(super) async fn maybe_create_automatic_thumbnail_dag(
        &self,
        streamer_id: &str,
        session_id: &str,
        segment_path: &str,
    ) {
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
}
