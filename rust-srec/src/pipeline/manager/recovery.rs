use super::*;
use std::collections::{BTreeMap, HashMap, HashSet};

use crate::database::models::DagExecutionDbModel;

const COORDINATOR_RECOVERY_PAGE_LIMIT: u32 = 500;

struct RecoveredCoordinatorSession {
    session: crate::database::models::LiveSessionDbModel,
    has_in_flight_coordination_dag: bool,
}

impl<CR, SR> PipelineManager<CR, SR>
where
    CR: ConfigRepository + Send + Sync + 'static,
    SR: StreamerRepository + Send + Sync + 'static,
{
    /// Recover jobs from database on startup.
    /// Resets PROCESSING jobs to PENDING for re-execution.
    /// For sequential pipelines, no special handling is needed since only one job
    /// per pipeline exists at a time.
    pub async fn recover_jobs(&self) -> Result<usize> {
        info!("Recovering jobs from database...");
        let recovered = self.job_queue.recover_jobs().await?;
        if let Err(e) = self.recover_pipeline_coordination().await {
            warn!(
                error = %e,
                "Failed to recover pipeline coordination state; continuing with empty coordinator"
            );
        }
        if recovered > 0 {
            info!("Recovered {} jobs from database", recovered);
        } else {
            debug!("No jobs to recover from database");
        }
        Ok(recovered)
    }

    pub(super) async fn recover_pipeline_coordination(&self) -> Result<()> {
        let Some(session_repo) = &self.session_repo else {
            return Ok(());
        };

        let mut sessions = BTreeMap::<String, RecoveredCoordinatorSession>::new();
        let filters = SessionFilters {
            active_only: Some(true),
            include_empty: Some(true),
            ..SessionFilters::new()
        };
        let mut offset = 0;
        loop {
            let pagination = Pagination::new(COORDINATOR_RECOVERY_PAGE_LIMIT, offset);
            let (page, _) = session_repo
                .list_sessions_filtered(&filters, &pagination)
                .await?;
            let page_len = page.len();
            for session in page {
                sessions.insert(
                    session.id.clone(),
                    RecoveredCoordinatorSession {
                        session,
                        has_in_flight_coordination_dag: false,
                    },
                );
            }

            if page_len < COORDINATOR_RECOVERY_PAGE_LIMIT as usize {
                break;
            }
            offset = offset.saturating_add(COORDINATOR_RECOVERY_PAGE_LIMIT);
        }

        if let Some(dag_repo) = &self.dag_repository {
            for status in [DagExecutionStatus::Pending, DagExecutionStatus::Processing] {
                for dag in self
                    .list_recoverable_coordination_dags(dag_repo, status, None)
                    .await
                {
                    let Some(session_id) = dag.session_id.as_deref() else {
                        continue;
                    };

                    if !sessions.contains_key(session_id) {
                        match session_repo.get_session(session_id).await {
                            Ok(session) => {
                                sessions.insert(
                                    session_id.to_string(),
                                    RecoveredCoordinatorSession {
                                        session,
                                        has_in_flight_coordination_dag: true,
                                    },
                                );
                            }
                            Err(e) => {
                                warn!(
                                    session_id = %session_id,
                                    dag_id = %dag.id,
                                    status = %status.as_str(),
                                    error = %e,
                                    "Skipping pipeline coordinator recovery for DAG with missing session"
                                );
                            }
                        }
                    } else if let Some(recovered) = sessions.get_mut(session_id) {
                        recovered.has_in_flight_coordination_dag = true;
                    }
                }
            }
        }

        let mut recovered = 0usize;

        for recovered_session in sessions.into_values() {
            let session = recovered_session.session;
            let recover_ended_session =
                session.end_time.is_some() && recovered_session.has_in_flight_coordination_dag;
            let streamer_id = session.streamer_id.clone();
            let session_id = session.id.clone();
            let config = if let Some(config_service) = &self.config_service {
                config_service
                    .get_config_for_streamer(&streamer_id)
                    .await
                    .ok()
            } else {
                None
            };

            let danmu_enabled = config
                .as_ref()
                .map(|config| config.record_danmu)
                .unwrap_or(false);
            let mut commands = self
                .pipeline_coordinator
                .apply_event(PipelineCoordinationEvent::ConfigureSession {
                    session_id: session_id.clone(),
                    streamer_id: streamer_id.clone(),
                    danmu_enabled,
                    segment_pipeline: config.as_ref().and_then(|config| config.pipeline.clone()),
                    paired_segment_pipeline: config
                        .as_ref()
                        .and_then(|config| config.paired_segment_pipeline.clone())
                        .filter(|_| danmu_enabled),
                    session_complete_pipeline: config
                        .as_ref()
                        .and_then(|config| config.session_complete_pipeline.clone()),
                })
                .await;

            let coordination_dags = if let Some(dag_repo) = &self.dag_repository {
                let mut dags = Vec::new();
                for status in [
                    DagExecutionStatus::Pending,
                    DagExecutionStatus::Processing,
                    DagExecutionStatus::Completed,
                    DagExecutionStatus::Failed,
                    DagExecutionStatus::Cancelled,
                ] {
                    dags.extend(
                        self.list_recoverable_coordination_dags(
                            dag_repo,
                            status,
                            Some(&session_id),
                        )
                        .await,
                    );
                }
                dags
            } else {
                Vec::new()
            };
            let mut recovered_danmu_activity = coordination_dags
                .iter()
                .any(|dag| dag.segment_source.as_deref() == Some("danmu"));

            for dag in &coordination_dags {
                if dag.segment_source.as_deref() != Some("paired") {
                    continue;
                }
                let Some(segment_index) = dag_segment_index(dag) else {
                    continue;
                };
                match dag.get_status() {
                    Some(DagExecutionStatus::Pending | DagExecutionStatus::Processing) => {
                        commands.extend(
                            self.pipeline_coordinator
                                .apply_event(PipelineCoordinationEvent::PairedDagStarted {
                                    session_id: session_id.clone(),
                                    streamer_id: streamer_id.clone(),
                                    segment_index,
                                })
                                .await,
                        );
                    }
                    Some(
                        DagExecutionStatus::Completed
                        | DagExecutionStatus::Failed
                        | DagExecutionStatus::Cancelled,
                    ) => {
                        commands.extend(
                            self.pipeline_coordinator
                                .apply_event(PipelineCoordinationEvent::RecoverPairedDagTriggered {
                                    session_id: session_id.clone(),
                                    streamer_id: streamer_id.clone(),
                                    segment_index,
                                })
                                .await,
                        );
                    }
                    None => {}
                }
            }

            for dag in &coordination_dags {
                let Some(segment_source) = dag.segment_source.as_deref() else {
                    continue;
                };
                if !matches!(segment_source, "video" | "danmu") {
                    continue;
                }
                if !matches!(
                    dag.get_status(),
                    Some(DagExecutionStatus::Pending | DagExecutionStatus::Processing)
                ) {
                    continue;
                }
                let Some(segment_index) = dag_segment_index(dag) else {
                    continue;
                };
                let source = if segment_source == "video" {
                    SourceType::Video
                } else {
                    SourceType::Danmu
                };
                commands.extend(
                    self.pipeline_coordinator
                        .apply_event(PipelineCoordinationEvent::SegmentDagStarted {
                            session_id: session_id.clone(),
                            streamer_id: streamer_id.clone(),
                            segment_index,
                            source,
                        })
                        .await,
                );
            }

            match session_repo
                .list_session_segments_for_session(&session_id, 10_000)
                .await
            {
                Ok(segments) => {
                    for segment in segments {
                        let Ok(segment_index) = u32::try_from(segment.segment_index) else {
                            warn!(
                                session_id = %session_id,
                                segment_index = %segment.segment_index,
                                "Skipping recovered segment with invalid index"
                            );
                            continue;
                        };

                        commands.extend(
                            self.pipeline_coordinator
                                .apply_event(PipelineCoordinationEvent::RecoverSourceArtifact {
                                    session_id: session_id.clone(),
                                    streamer_id: streamer_id.clone(),
                                    segment_index,
                                    source: SourceType::Video,
                                    path: PathBuf::from(segment.file_path),
                                })
                                .await,
                        );
                    }
                }
                Err(e) => warn!(
                    session_id = %session_id,
                    error = %e,
                    "Failed to recover session segments for pipeline coordinator"
                ),
            }

            match session_repo
                .get_media_outputs_for_session(&session_id)
                .await
            {
                Ok(outputs) => {
                    for output in outputs {
                        if output.file_type != MediaFileType::DanmuXml.as_str() {
                            continue;
                        }
                        let path = PathBuf::from(&output.file_path);
                        let Some(segment_index) = parse_segment_index_from_danmu(&output.id, &path)
                        else {
                            trace!(
                                session_id = %session_id,
                                path = %path.display(),
                                "Skipping recovered danmu output without segment index"
                            );
                            continue;
                        };
                        recovered_danmu_activity = true;
                        commands.extend(
                            self.pipeline_coordinator
                                .apply_event(PipelineCoordinationEvent::RecoverSourceArtifact {
                                    session_id: session_id.clone(),
                                    streamer_id: streamer_id.clone(),
                                    segment_index,
                                    source: SourceType::Danmu,
                                    path,
                                })
                                .await,
                        );
                    }
                }
                Err(e) => warn!(
                    session_id = %session_id,
                    error = %e,
                    "Failed to recover media outputs for pipeline coordinator"
                ),
            }

            for dag in &coordination_dags {
                let Some(segment_source) = dag.segment_source.as_deref() else {
                    continue;
                };
                if !matches!(segment_source, "video" | "danmu") {
                    continue;
                }
                let Some(segment_index) = dag_segment_index(dag) else {
                    continue;
                };
                let source = if segment_source == "video" {
                    SourceType::Video
                } else {
                    SourceType::Danmu
                };

                match dag.get_status() {
                    Some(DagExecutionStatus::Completed) => {
                        let outputs = self.recover_leaf_outputs(dag).await;
                        commands.extend(
                            self.pipeline_coordinator
                                .apply_event(
                                    PipelineCoordinationEvent::RecoverSegmentDagCompleted {
                                        session_id: session_id.clone(),
                                        streamer_id: streamer_id.clone(),
                                        segment_index,
                                        source,
                                        outputs,
                                    },
                                )
                                .await,
                        );
                    }
                    Some(DagExecutionStatus::Failed | DagExecutionStatus::Cancelled) => {
                        commands.extend(
                            self.pipeline_coordinator
                                .apply_event(PipelineCoordinationEvent::RecoverSegmentDagFailed {
                                    session_id: session_id.clone(),
                                    segment_index,
                                    source,
                                })
                                .await,
                        );
                    }
                    Some(DagExecutionStatus::Pending | DagExecutionStatus::Processing) | None => {}
                }
            }

            if recover_ended_session {
                commands.extend(
                    self.pipeline_coordinator
                        .apply_event(PipelineCoordinationEvent::SessionEnded {
                            session_id: session_id.clone(),
                            streamer_id: streamer_id.clone(),
                            should_run_session_complete: true,
                        })
                        .await,
                );
                if recovered_danmu_activity {
                    commands.extend(
                        self.pipeline_coordinator
                            .apply_event(PipelineCoordinationEvent::DanmuCollectionStopped {
                                session_id: session_id.clone(),
                            })
                            .await,
                    );
                }
                commands.extend(
                    self.pipeline_coordinator
                        .apply_event(PipelineCoordinationEvent::SessionEndPersisted {
                            session_id: session_id.clone(),
                        })
                        .await,
                );
            }

            self.execute_pipeline_commands(commands).await;
            recovered += 1;
        }

        if recovered > 0 {
            info!(
                sessions = %recovered,
                "Recovered pipeline coordination state"
            );
        }

        Ok(())
    }

    async fn list_recoverable_coordination_dags(
        &self,
        dag_repo: &Arc<dyn DagRepository>,
        status: DagExecutionStatus,
        session_id: Option<&str>,
    ) -> Vec<DagExecutionDbModel> {
        let mut dags = Vec::new();
        let mut offset = 0;

        loop {
            match dag_repo
                .list_dags(
                    Some(status.as_str()),
                    session_id,
                    COORDINATOR_RECOVERY_PAGE_LIMIT,
                    offset,
                )
                .await
            {
                Ok(page) => {
                    let page_len = page.len();
                    dags.extend(page.into_iter().filter(|dag| {
                        matches!(
                            dag.segment_source.as_deref(),
                            Some("video" | "danmu" | "paired")
                        )
                    }));

                    if page_len < COORDINATOR_RECOVERY_PAGE_LIMIT as usize {
                        break;
                    }
                    offset = offset.saturating_add(COORDINATOR_RECOVERY_PAGE_LIMIT);
                }
                Err(e) => {
                    warn!(
                        session_id = ?session_id,
                        status = %status.as_str(),
                        error = %e,
                        "Failed to list DAGs for pipeline coordinator recovery"
                    );
                    break;
                }
            }
        }

        dags
    }

    async fn recover_leaf_outputs(&self, dag: &DagExecutionDbModel) -> Vec<PathBuf> {
        let Some(dag_repo) = &self.dag_repository else {
            return Vec::new();
        };

        let Some(definition) = dag.get_dag_definition() else {
            warn!(
                dag_id = %dag.id,
                session_id = ?dag.session_id,
                "Failed to parse DAG definition during pipeline coordinator recovery"
            );
            return Vec::new();
        };

        let steps = match dag_repo.get_steps_by_dag(&dag.id).await {
            Ok(steps) => steps,
            Err(e) => {
                warn!(
                    dag_id = %dag.id,
                    session_id = ?dag.session_id,
                    error = %e,
                    "Failed to recover DAG leaf outputs for pipeline coordinator"
                );
                return Vec::new();
            }
        };

        let mut steps_by_id = HashMap::with_capacity(steps.len());
        for step in &steps {
            steps_by_id.insert(step.step_id.as_str(), step);
        }

        let mut seen = HashSet::<String>::new();
        let mut outputs = Vec::new();
        for leaf in definition.leaf_steps() {
            let Some(step) = steps_by_id.get(leaf.id.as_str()) else {
                continue;
            };
            for output in step.get_outputs() {
                let key = if cfg!(windows) {
                    output.to_lowercase()
                } else {
                    output.clone()
                };
                if seen.insert(key) {
                    outputs.push(PathBuf::from(output));
                }
            }
        }

        outputs
    }
}

fn dag_segment_index(dag: &DagExecutionDbModel) -> Option<u32> {
    let Some(raw_index) = dag.segment_index else {
        warn!(
            dag_id = %dag.id,
            session_id = ?dag.session_id,
            segment_source = ?dag.segment_source,
            "Skipping recovered DAG without segment_index"
        );
        return None;
    };

    match u32::try_from(raw_index) {
        Ok(segment_index) => Some(segment_index),
        Err(_) => {
            warn!(
                dag_id = %dag.id,
                session_id = ?dag.session_id,
                segment_source = ?dag.segment_source,
                segment_index = %raw_index,
                "Skipping recovered DAG with invalid segment_index"
            );
            None
        }
    }
}
