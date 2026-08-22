use super::*;
use chrono::{DateTime, Utc};

impl<CR, SR> PipelineManager<CR, SR>
where
    CR: ConfigRepository + Send + Sync + 'static,
    SR: StreamerRepository + Send + Sync + 'static,
{
    pub(super) async fn handle_dag_completion(&self, completion: DagCompletionInfo) {
        let dag_id = completion.dag_id.clone();

        debug!(
            dag_id = %completion.dag_id,
            streamer_id = ?completion.streamer_id,
            session_id = ?completion.session_id,
            succeeded = completion.succeeded,
            leaf_outputs = %completion.leaf_outputs.len(),
            "DAG completion received"
        );

        if self
            .handled_dag_completions
            .insert(dag_id.clone(), std::time::Instant::now())
            .is_some()
        {
            trace!(dag_id = %dag_id, "Ignoring duplicate DAG completion");
            return;
        }

        if let Some((_, ctx)) = self.dag_segment_contexts.remove(&dag_id) {
            if let Some(session_id) = completion.session_id.as_deref()
                && session_id != ctx.session_id
            {
                warn!(
                    dag_id = %completion.dag_id,
                    completion_session_id = %session_id,
                    context_session_id = %ctx.session_id,
                    "DAG completion session_id mismatch"
                );
            }

            let commands = if completion.succeeded {
                let leaf_outputs: Vec<PathBuf> = completion
                    .leaf_outputs
                    .into_iter()
                    .map(PathBuf::from)
                    .collect();

                debug!(
                    dag_id = %dag_id,
                    session_id = %ctx.session_id,
                    streamer_id = %ctx.streamer_id,
                    segment_index = %ctx.segment_index,
                    source = ?ctx.source,
                    leaf_outputs = %leaf_outputs.len(),
                    "Processing successful DAG completion with segment context"
                );
                self.pipeline_coordinator
                    .apply_event(PipelineCoordinationEvent::SegmentDagCompleted {
                        session_id: ctx.session_id.clone(),
                        streamer_id: ctx.streamer_id.clone(),
                        segment_index: ctx.segment_index,
                        source: ctx.source,
                        outputs: leaf_outputs,
                    })
                    .await
            } else {
                warn!(
                    dag_id = %dag_id,
                    session_id = %ctx.session_id,
                    streamer_id = %ctx.streamer_id,
                    segment_index = %ctx.segment_index,
                    source = ?ctx.source,
                    "DAG failed for segment context"
                );
                self.pipeline_coordinator
                    .apply_event(PipelineCoordinationEvent::SegmentDagFailed {
                        session_id: ctx.session_id.clone(),
                        segment_index: ctx.segment_index,
                        source: ctx.source,
                    })
                    .await
            };

            self.execute_pipeline_commands(commands).await;
            return;
        }

        if let Some((_, ctx)) = self.paired_dag_contexts.remove(&dag_id) {
            if let Some(session_id) = completion.session_id.as_deref()
                && session_id != ctx.session_id
            {
                warn!(
                    dag_id = %completion.dag_id,
                    completion_session_id = %session_id,
                    context_session_id = %ctx.session_id,
                    "Paired DAG completion session_id mismatch"
                );
            }

            let commands = if completion.succeeded {
                trace!(
                    dag_id = %completion.dag_id,
                    session_id = %ctx.session_id,
                    streamer_id = %ctx.streamer_id,
                    segment_index = %ctx.segment_index,
                    "Paired-segment DAG completed"
                );
                self.pipeline_coordinator
                    .apply_event(PipelineCoordinationEvent::PairedDagCompleted {
                        session_id: ctx.session_id.clone(),
                    })
                    .await
            } else {
                trace!(
                    dag_id = %completion.dag_id,
                    session_id = %ctx.session_id,
                    streamer_id = %ctx.streamer_id,
                    segment_index = %ctx.segment_index,
                    "Paired-segment DAG failed"
                );
                self.pipeline_coordinator
                    .apply_event(PipelineCoordinationEvent::PairedDagFailed {
                        session_id: ctx.session_id.clone(),
                    })
                    .await
            };

            self.execute_pipeline_commands(commands).await;
            return;
        }

        if self.handle_dag_completion_without_context(completion).await {
            return;
        }

        let _ = self.handled_dag_completions.remove(&dag_id);
        debug!(
            dag_id = %dag_id,
            "Ignoring DAG completion without segment context"
        );
    }

    pub(super) async fn handle_dag_completion_without_context(
        &self,
        completion: DagCompletionInfo,
    ) -> bool {
        let Some(session_id) = completion.session_id.as_deref() else {
            return false;
        };

        let Some(repo) = &self.dag_repository else {
            trace!(
                dag_id = %completion.dag_id,
                session_id = %session_id,
                "DAG repository not configured; cannot recover completion context"
            );
            return false;
        };

        let dag = match repo.get_dag(&completion.dag_id).await {
            Ok(dag) => dag,
            Err(e) => {
                warn!(
                    dag_id = %completion.dag_id,
                    error = %e,
                    "Failed to load DAG execution for completion recovery"
                );
                return false;
            }
        };

        if let Some(db_session_id) = dag.session_id.as_deref()
            && db_session_id != session_id
        {
            warn!(
                dag_id = %completion.dag_id,
                completion_session_id = %session_id,
                db_session_id = %db_session_id,
                "DAG completion session_id mismatch (db vs completion)"
            );
        }

        let Some(segment_source) = dag.segment_source.as_deref() else {
            trace!(
                dag_id = %completion.dag_id,
                session_id = %session_id,
                "DAG completion has no segment metadata; ignoring"
            );
            return false;
        };

        match segment_source {
            "video" | "danmu" => {
                let Some(raw_index) = dag.segment_index else {
                    warn!(
                        dag_id = %completion.dag_id,
                        session_id = %session_id,
                        segment_source = %segment_source,
                        "DAG completion missing segment_index metadata"
                    );
                    return false;
                };
                let Ok(segment_index) = u32::try_from(raw_index) else {
                    warn!(
                        dag_id = %completion.dag_id,
                        session_id = %session_id,
                        segment_source = %segment_source,
                        segment_index = %raw_index,
                        "DAG completion has invalid segment_index metadata"
                    );
                    return false;
                };

                let source = match segment_source {
                    "video" => SourceType::Video,
                    "danmu" => SourceType::Danmu,
                    _ => unreachable!("match guards ensure only video/danmu"),
                };

                let commands = if completion.succeeded {
                    let leaf_outputs: Vec<PathBuf> = completion
                        .leaf_outputs
                        .into_iter()
                        .map(PathBuf::from)
                        .collect();

                    let streamer_id = dag.streamer_id.clone().or(completion.streamer_id.clone());
                    let Some(streamer_id) = streamer_id else {
                        warn!(
                            dag_id = %completion.dag_id,
                            session_id = %session_id,
                            segment_index = %segment_index,
                            segment_source = %segment_source,
                            "Missing streamer_id for recovered segment DAG completion"
                        );
                        return false;
                    };

                    self.pipeline_coordinator
                        .apply_event(PipelineCoordinationEvent::SegmentDagCompleted {
                            session_id: session_id.to_string(),
                            streamer_id,
                            segment_index,
                            source,
                            outputs: leaf_outputs,
                        })
                        .await
                } else {
                    self.pipeline_coordinator
                        .apply_event(PipelineCoordinationEvent::SegmentDagFailed {
                            session_id: session_id.to_string(),
                            segment_index,
                            source,
                        })
                        .await
                };

                self.execute_pipeline_commands(commands).await;
                true
            }
            "paired" => {
                let commands = if completion.succeeded {
                    trace!(
                        dag_id = %completion.dag_id,
                        session_id = %session_id,
                        "Paired-segment DAG completed (recovered context)"
                    );
                    self.pipeline_coordinator
                        .apply_event(PipelineCoordinationEvent::PairedDagCompleted {
                            session_id: session_id.to_string(),
                        })
                        .await
                } else {
                    trace!(
                        dag_id = %completion.dag_id,
                        session_id = %session_id,
                        "Paired-segment DAG failed (recovered context)"
                    );
                    self.pipeline_coordinator
                        .apply_event(PipelineCoordinationEvent::PairedDagFailed {
                            session_id: session_id.to_string(),
                        })
                        .await
                };

                self.execute_pipeline_commands(commands).await;
                true
            }
            other => {
                trace!(
                    dag_id = %completion.dag_id,
                    session_id = %session_id,
                    segment_source = %other,
                    "Unknown DAG segment_source; ignoring"
                );
                false
            }
        }
    }

    pub(super) async fn run_session_complete_pipeline(
        &self,
        outputs: SessionOutputs,
        pipeline_def: DagPipelineDefinition,
    ) {
        debug!(
            session_id = %outputs.session_id,
            streamer_id = %outputs.streamer_id,
            pipeline_name = %pipeline_def.name,
            pipeline_steps = %pipeline_def.steps.len(),
            "Entered run_session_complete_pipeline"
        );

        // Skip if pipeline has no steps configured
        if pipeline_def.is_empty() {
            debug!(
                session_id = %outputs.session_id,
                "Skipping session-complete pipeline: no steps configured"
            );
            return;
        }

        #[derive(Serialize)]
        struct SessionCompleteManifest {
            session_id: String,
            streamer_id: String,
            video_inputs: Vec<String>,
            danmu_inputs: Vec<String>,
        }

        let video_paths = outputs.get_sorted_video_outputs();
        let danmu_paths = outputs.get_sorted_danmu_outputs();

        let mut input_paths: Vec<String> = Vec::new();

        if let Some(base_dir) = video_paths
            .first()
            .or_else(|| danmu_paths.first())
            .and_then(|p| p.parent())
        {
            let manifest = SessionCompleteManifest {
                session_id: outputs.session_id.clone(),
                streamer_id: outputs.streamer_id.clone(),
                video_inputs: video_paths
                    .iter()
                    .map(|p| p.to_string_lossy().to_string())
                    .collect(),
                danmu_inputs: danmu_paths
                    .iter()
                    .map(|p| p.to_string_lossy().to_string())
                    .collect(),
            };

            let manifest_name = format!(
                "session_{}_inputs.json",
                sanitize_filename(&outputs.session_id)
            );
            let manifest_path = base_dir.join(manifest_name);

            match serde_json::to_vec_pretty(&manifest) {
                Ok(json) => {
                    if let Err(e) = tokio::fs::write(&manifest_path, json).await {
                        warn!(
                            session_id = %outputs.session_id,
                            path = %manifest_path.display(),
                            error = %e,
                            "Failed to write session input manifest (continuing without manifest)"
                        );
                    } else {
                        input_paths.push(manifest_path.to_string_lossy().to_string());
                    }
                }
                Err(e) => {
                    warn!(
                        session_id = %outputs.session_id,
                        error = %e,
                        "Failed to serialize session input manifest (continuing without manifest)"
                    );
                }
            }
        }

        input_paths.extend(
            video_paths
                .into_iter()
                .map(|p| p.to_string_lossy().to_string()),
        );
        input_paths.extend(
            danmu_paths
                .into_iter()
                .map(|p| p.to_string_lossy().to_string()),
        );

        info!(
            session_id = %outputs.session_id,
            streamer_id = %outputs.streamer_id,
            inputs = %input_paths.len(),
            "Triggering session-complete pipeline"
        );

        if let Err(e) = self
            .create_dag_pipeline(
                &outputs.session_id,
                &outputs.streamer_id,
                input_paths,
                pipeline_def,
            )
            .await
        {
            tracing::error!(
                "Failed to create session-complete pipeline for session {}: {}",
                outputs.session_id,
                e
            );
        }
    }

    pub(super) async fn run_paired_segment_pipeline(
        &self,
        outputs: PairedSegmentOutputs,
        pipeline_def: DagPipelineDefinition,
    ) -> Vec<PipelineCommand> {
        // Skip if pipeline has no steps configured
        if pipeline_def.is_empty() {
            debug!(
                session_id = %outputs.session_id,
                segment_index = %outputs.segment_index,
                "Skipping paired-segment pipeline: no steps configured"
            );
            return Vec::new();
        }

        #[derive(Serialize)]
        struct PairedSegmentManifest {
            session_id: String,
            streamer_id: String,
            segment_index: u32,
            video_inputs: Vec<String>,
            danmu_inputs: Vec<String>,
        }

        let video_inputs: Vec<String> = outputs
            .video_outputs
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        let danmu_inputs: Vec<String> = outputs
            .danmu_outputs
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();

        let mut input_paths: Vec<String> = Vec::new();

        if let Some(base_dir) = outputs
            .video_outputs
            .first()
            .or_else(|| outputs.danmu_outputs.first())
            .and_then(|p| p.parent())
        {
            let manifest = PairedSegmentManifest {
                session_id: outputs.session_id.clone(),
                streamer_id: outputs.streamer_id.clone(),
                segment_index: outputs.segment_index,
                video_inputs,
                danmu_inputs,
            };

            let manifest_name = format!(
                "segment_{}_{}_inputs.json",
                sanitize_filename(&outputs.session_id),
                outputs.segment_index
            );
            let manifest_path = base_dir.join(manifest_name);

            match serde_json::to_vec_pretty(&manifest) {
                Ok(json) => {
                    if let Err(e) = tokio::fs::write(&manifest_path, json).await {
                        warn!(
                            session_id = %outputs.session_id,
                            segment_index = %outputs.segment_index,
                            path = %manifest_path.display(),
                            error = %e,
                            "Failed to write paired-segment input manifest (continuing without manifest)"
                        );
                    } else {
                        input_paths.push(manifest_path.to_string_lossy().to_string());
                    }
                }
                Err(e) => {
                    warn!(
                        session_id = %outputs.session_id,
                        segment_index = %outputs.segment_index,
                        error = %e,
                        "Failed to serialize paired-segment input manifest (continuing without manifest)"
                    );
                }
            }
        }

        input_paths.extend(
            outputs
                .video_outputs
                .into_iter()
                .map(|p| p.to_string_lossy().to_string()),
        );
        input_paths.extend(
            outputs
                .danmu_outputs
                .into_iter()
                .map(|p| p.to_string_lossy().to_string()),
        );

        info!(
            session_id = %outputs.session_id,
            streamer_id = %outputs.streamer_id,
            segment_index = %outputs.segment_index,
            inputs = %input_paths.len(),
            "Triggering paired-segment pipeline"
        );

        let start_commands = self
            .pipeline_coordinator
            .apply_event(PipelineCoordinationEvent::PairedDagStarted {
                session_id: outputs.session_id.clone(),
                streamer_id: outputs.streamer_id.clone(),
                segment_index: outputs.segment_index,
            })
            .await;
        if !start_commands.is_empty() {
            warn!(
                session_id = %outputs.session_id,
                segment_index = %outputs.segment_index,
                commands = %start_commands.len(),
                "Ignoring unexpected commands from paired DAG start event"
            );
        }

        let contexts = self.paired_dag_contexts.clone();
        let ctx = PairedDagContext {
            session_id: outputs.session_id.clone(),
            streamer_id: outputs.streamer_id.clone(),
            segment_index: outputs.segment_index,
            created_at: std::time::Instant::now(),
        };
        let before_root_jobs = Some(Box::new(move |dag_id: &str| {
            debug!(
                dag_id = %dag_id,
                session_id = %ctx.session_id,
                streamer_id = %ctx.streamer_id,
                segment_index = %ctx.segment_index,
                "Tracking paired-segment DAG context"
            );
            contexts.insert(dag_id.to_string(), ctx);
        }) as BeforeRootJobsHook);

        if let Err(e) = self
            .create_dag_pipeline_internal(
                &outputs.session_id,
                &outputs.streamer_id,
                input_paths,
                pipeline_def,
                before_root_jobs,
                Some(DagExecutionMetadata {
                    segment_index: Some(outputs.segment_index),
                    segment_source: Some("paired".to_string()),
                }),
            )
            .await
        {
            tracing::error!(
                "Failed to create paired-segment pipeline for session {} segment {}: {}",
                outputs.session_id,
                outputs.segment_index,
                e
            );
            return self
                .pipeline_coordinator
                .apply_event(PipelineCoordinationEvent::PairedDagFailed {
                    session_id: outputs.session_id.clone(),
                })
                .await;
        }

        Vec::new()
    }

    /// Stop the pipeline manager.
    pub(super) async fn lookup_streamer_name(&self, streamer_id: &str) -> Option<String> {
        let repo = self.streamer_repo.as_ref()?;

        match repo.get_streamer(streamer_id).await {
            Ok(streamer) => Some(streamer.name),
            Err(e) => {
                debug!(
                    streamer_id = %streamer_id,
                    error = %e,
                    "Failed to look up streamer name"
                );
                None
            }
        }
    }

    /// Look up the platform name (e.g. "Twitch") from the streamer's platform config.
    pub(super) async fn lookup_platform_name(&self, streamer_id: &str) -> Option<String> {
        let streamer_repo = self.streamer_repo.as_ref()?;
        let config_service = self.config_service.as_ref()?;

        let platform_id = match streamer_repo.get_streamer(streamer_id).await {
            Ok(streamer) => streamer.platform_config_id,
            Err(e) => {
                debug!(
                    streamer_id = %streamer_id,
                    error = %e,
                    "Failed to look up streamer platform_config_id"
                );
                return None;
            }
        };

        match config_service.get_platform_config(&platform_id).await {
            Ok(platform) => Some(platform.platform_name),
            Err(e) => {
                debug!(
                    streamer_id = %streamer_id,
                    platform_id = %platform_id,
                    error = %e,
                    "Failed to look up platform name"
                );
                None
            }
        }
    }

    /// Look up session metadata from the repository.
    /// Returns the most recent title from the titles JSON array and the session start time.
    pub(super) async fn lookup_session_meta(
        &self,
        session_id: &str,
    ) -> (Option<String>, Option<DateTime<Utc>>) {
        let Some(repo) = self.session_repo.as_ref() else {
            return (None, None);
        };

        match repo.get_session(session_id).await {
            Ok(session) => {
                let session_start = Some(crate::database::time::ms_to_datetime(session.start_time));
                // Parse the titles JSON array and get the most recent title
                let title = if let Some(titles_json) = session.titles
                    && let Ok(entries) = serde_json::from_str::<Vec<TitleEntry>>(&titles_json)
                {
                    // Return the last (most recent) title
                    entries.last().map(|e| e.title.clone())
                } else {
                    None
                };

                (title, session_start)
            }
            Err(e) => {
                debug!(
                    session_id = %session_id,
                    error = %e,
                    "Failed to look up session title"
                );
                (None, None)
            }
        }
    }

    /// Create a DAG pipeline with fan-in/fan-out support.
    ///
    /// Unlike sequential pipelines, DAG pipelines support:
    /// - Fan-out: One step can trigger multiple downstream steps
    /// - Fan-in: Multiple steps can merge their outputs before a downstream step
    /// - Fail-fast: Any step failure cancels all pending/running jobs in the DAG
    ///
    /// Returns the DAG ID and root job IDs for tracking.
    pub async fn create_dag_pipeline(
        &self,
        session_id: &str,
        streamer_id: &str,
        input_paths: Vec<String>,
        dag_definition: DagPipelineDefinition,
    ) -> Result<DagCreationResult> {
        self.create_dag_pipeline_internal(
            session_id,
            streamer_id,
            input_paths,
            dag_definition,
            None,
            None,
        )
        .await
    }

    /// Cancel a DAG execution.
    ///
    /// This also notifies the paired/session coordinators so session-complete orchestration
    /// can't get stuck waiting for a cancelled DAG to finish.
    pub async fn cancel_dag(&self, dag_id: &str) -> Result<u64> {
        let dag_scheduler = self.dag_scheduler.as_ref().ok_or_else(|| {
            crate::Error::Validation(
                "DAG scheduler not configured. Call with_dag_repository() first.".to_string(),
            )
        })?;

        let update = dag_scheduler.cancel_dag_with_completion(dag_id).await?;
        if let Some(completion) = update.completion {
            self.handle_dag_completion(completion).await;
        }

        Ok(update.cancelled_count)
    }

    pub(super) async fn execute_pipeline_commands(&self, commands: Vec<PipelineCommand>) {
        let mut pending = std::collections::VecDeque::from(commands);
        while let Some(command) = pending.pop_front() {
            match command {
                PipelineCommand::CreateSegmentDag {
                    session_id,
                    streamer_id,
                    segment_index,
                    source,
                    input_path,
                    pipeline,
                } => {
                    pending.extend(
                        self.run_segment_pipeline(
                            session_id,
                            streamer_id,
                            segment_index,
                            source,
                            input_path,
                            pipeline,
                        )
                        .await,
                    );
                }
                PipelineCommand::CreatePairedSegmentDag { outputs, pipeline } => {
                    pending.extend(self.run_paired_segment_pipeline(outputs, pipeline).await);
                }
                PipelineCommand::CreateSessionCompleteDag { outputs, pipeline } => {
                    self.run_session_complete_pipeline(outputs, pipeline).await;
                }
            }
        }
    }

    pub(super) async fn run_segment_pipeline(
        &self,
        session_id: String,
        streamer_id: String,
        segment_index: u32,
        source: SourceType,
        input_path: PathBuf,
        pipeline_def: DagPipelineDefinition,
    ) -> Vec<PipelineCommand> {
        if pipeline_def.is_empty() {
            return Vec::new();
        }

        let start_commands = self
            .pipeline_coordinator
            .apply_event(PipelineCoordinationEvent::SegmentDagStarted {
                session_id: session_id.clone(),
                streamer_id: streamer_id.clone(),
                segment_index,
                source,
            })
            .await;
        if !start_commands.is_empty() {
            warn!(
                session_id = %session_id,
                segment_index = %segment_index,
                source = ?source,
                commands = %start_commands.len(),
                "Ignoring unexpected commands from segment DAG start event"
            );
        }

        let contexts = self.dag_segment_contexts.clone();
        let ctx = SegmentDagContext {
            session_id: session_id.clone(),
            streamer_id: streamer_id.clone(),
            segment_index,
            source,
            created_at: std::time::Instant::now(),
        };
        let before_root_jobs = Some(Box::new(move |dag_id: &str| {
            debug!(
                dag_id = %dag_id,
                session_id = %ctx.session_id,
                streamer_id = %ctx.streamer_id,
                segment_index = %ctx.segment_index,
                source = ?ctx.source,
                "Tracking per-segment DAG context"
            );
            contexts.insert(dag_id.to_string(), ctx);
        }) as BeforeRootJobsHook);

        if let Err(e) = self
            .create_dag_pipeline_internal(
                &session_id,
                &streamer_id,
                vec![input_path.to_string_lossy().to_string()],
                pipeline_def,
                before_root_jobs,
                Some(DagExecutionMetadata {
                    segment_index: Some(segment_index),
                    segment_source: Some(source.as_segment_source().to_string()),
                }),
            )
            .await
        {
            tracing::error!(
                "Failed to create {:?} segment pipeline for session {} segment {}: {}",
                source,
                session_id,
                segment_index,
                e
            );
            return self
                .pipeline_coordinator
                .apply_event(PipelineCoordinationEvent::SegmentDagFailed {
                    session_id,
                    segment_index,
                    source,
                })
                .await;
        }

        Vec::new()
    }

    pub(super) async fn create_dag_pipeline_internal(
        &self,
        session_id: &str,
        streamer_id: &str,
        input_paths: Vec<String>,
        dag_definition: DagPipelineDefinition,
        before_root_jobs: Option<BeforeRootJobsHook>,
        metadata: Option<DagExecutionMetadata>,
    ) -> Result<DagCreationResult> {
        let dag_scheduler = self.dag_scheduler.as_ref().ok_or_else(|| {
            crate::Error::Validation(
                "DAG scheduler not configured. Call with_dag_repository() first.".to_string(),
            )
        })?;

        // First, expand any workflow steps in the DAG
        let expanded_dag = self.expand_workflows_in_dag(dag_definition).await?;

        // Resolve all steps in the DAG before creation (Presets -> Inline)
        let mut resolved_dag = expanded_dag;
        for dag_step in &mut resolved_dag.steps {
            let resolved = self.resolve_dag_step(&dag_step.step).await?;
            dag_step.step = resolved;
        }

        // Look up metadata for placeholder support
        let streamer_name = self.lookup_streamer_name(streamer_id).await;
        let (session_title, session_start) = self.lookup_session_meta(session_id).await;
        let platform = self.lookup_platform_name(streamer_id).await;

        // Delegate to DAG scheduler
        let result = dag_scheduler
            .create_dag_pipeline_with_hook(
                resolved_dag,
                &input_paths,
                DagRunContext {
                    streamer_id: Some(streamer_id.to_string()),
                    session_id: Some(session_id.to_string()),
                    streamer_name: streamer_name.clone(),
                    session_title: session_title.clone(),
                    platform: platform.clone(),
                    session_start,
                },
                metadata,
                before_root_jobs,
            )
            .await?;

        info!(
            "Created DAG pipeline {} with {} steps ({} root jobs) for session {}, streamer {}, streamer name {}, session title {}",
            result.dag_id,
            result.total_steps,
            result.root_job_ids.len(),
            session_id,
            streamer_id,
            streamer_name.unwrap_or_default(),
            session_title.unwrap_or_default(),
        );

        // Emit events for root jobs
        for job_id in &result.root_job_ids {
            let _ = self.event_tx.send(PipelineEvent::JobEnqueued {
                job_id: job_id.clone(),
                job_type: "dag_step".to_string(),
                streamer_id: streamer_id.to_string(),
            });
        }

        // Check queue depth
        self.check_queue_depth();

        Ok(result)
    }

    /// Expand workflow steps in a DAG definition.
    ///
    /// For each step that is a `Workflow`, this method:
    /// 1. Looks up the workflow by name from the pipeline preset repository
    /// 2. Gets the workflow's `dag_definition` (its internal DAG structure)
    /// 3. Expands the workflow's steps into the parent DAG with prefixed IDs
    /// 4. Wires up dependencies correctly:
    ///    - Workflow's root steps inherit the original workflow step's `depends_on`
    ///    - Steps that depended on the workflow step now depend on the workflow's leaf steps
    ///
    /// This process is applied until no workflow steps remain (handles nested workflows).
    pub(super) async fn expand_workflows_in_dag(
        &self,
        mut dag: DagPipelineDefinition,
    ) -> Result<DagPipelineDefinition> {
        use std::collections::HashSet;

        // Keep expanding until no workflow steps remain (handles nested workflows)
        let mut iteration = 0;
        const MAX_ITERATIONS: usize = 10; // Prevent infinite loops from circular workflow references

        loop {
            iteration += 1;
            if iteration > MAX_ITERATIONS {
                return Err(crate::Error::Validation(
                    "Maximum workflow expansion depth exceeded. Check for circular workflow references.".to_string(),
                ));
            }

            // Find workflow steps that need expansion
            let workflow_steps: Vec<(usize, String)> = dag
                .steps
                .iter()
                .enumerate()
                .filter_map(|(idx, step)| {
                    if let PipelineStep::Workflow { name } = &step.step {
                        Some((idx, name.clone()))
                    } else {
                        None
                    }
                })
                .collect();

            if workflow_steps.is_empty() {
                break; // No more workflows to expand
            }

            // Process each workflow step
            for (workflow_step_idx, workflow_name) in workflow_steps.into_iter().rev() {
                // Process in reverse to maintain index validity.
                let workflow_step = &dag.steps[workflow_step_idx];
                let workflow_step_id = workflow_step.id.clone();
                let workflow_step_deps = workflow_step.depends_on.clone();

                // Look up the workflow
                let workflow_dag = self.lookup_workflow(&workflow_name).await?;

                // Find workflow's root and leaf steps
                let root_step_ids: HashSet<String> = workflow_dag
                    .root_steps()
                    .iter()
                    .map(|s| s.id.clone())
                    .collect();
                let leaf_step_ids: HashSet<String> = workflow_dag
                    .leaf_steps()
                    .iter()
                    .map(|s| s.id.clone())
                    .collect();

                // Create a prefix to avoid ID collisions
                let prefix = format!("{}__", workflow_step_id);

                // Build expanded steps with prefixed IDs
                let expanded_steps: Vec<DagStep> = workflow_dag
                    .steps
                    .iter()
                    .map(|s| {
                        let new_id = format!("{}{}", prefix, s.id);
                        let new_deps: Vec<String> = if root_step_ids.contains(&s.id) {
                            // Root steps inherit the original workflow step's dependencies
                            workflow_step_deps.clone()
                        } else {
                            // Internal steps get prefixed dependencies
                            s.depends_on
                                .iter()
                                .map(|d| format!("{}{}", prefix, d))
                                .collect()
                        };
                        DagStep {
                            id: new_id,
                            step: s.step.clone(),
                            depends_on: new_deps,
                        }
                    })
                    .collect();

                // Find steps that depend on the workflow step and update their dependencies
                let prefixed_leaf_ids: Vec<String> = leaf_step_ids
                    .iter()
                    .map(|id| format!("{}{}", prefix, id))
                    .collect();

                for step in &mut dag.steps {
                    if step.depends_on.contains(&workflow_step_id) {
                        // Remove the workflow step ID and add the workflow's leaf step IDs
                        step.depends_on.retain(|d| d != &workflow_step_id);
                        step.depends_on.extend(prefixed_leaf_ids.clone());
                    }
                }

                // Remove the workflow step and insert the expanded steps
                dag.steps.remove(workflow_step_idx);
                dag.steps
                    .splice(workflow_step_idx..workflow_step_idx, expanded_steps);
            }
        }

        Ok(dag)
    }

    /// Look up a workflow by name and return its DAG definition.
    pub(super) async fn lookup_workflow(&self, name: &str) -> Result<DagPipelineDefinition> {
        let repo = self.pipeline_preset_repo.as_ref().ok_or_else(|| {
            crate::Error::Validation(format!(
                "No pipeline preset repository, cannot expand workflow '{}'",
                name
            ))
        })?;

        let workflow = repo
            .get_pipeline_preset_by_name(name)
            .await
            .map_err(|e| crate::Error::Database(e.to_string()))?
            .ok_or_else(|| crate::Error::Validation(format!("Workflow '{}' not found", name)))?;

        // Get the DAG definition from the workflow
        let dag_def = workflow.get_dag_definition().ok_or_else(|| {
            crate::Error::Validation(format!(
                "Workflow '{}' does not have a DAG definition. Only DAG-based workflows can be embedded.",
                name
            ))
        })?;

        Ok(dag_def)
    }

    /// Resolve a DAG step's PipelineStep to an Inline step.
    pub(super) async fn resolve_dag_step(&self, step: &PipelineStep) -> Result<PipelineStep> {
        match step {
            PipelineStep::Preset { name } => {
                if let Some(repo) = &self.preset_repo {
                    match repo.get_preset_by_name(name).await {
                        Ok(Some(preset)) => {
                            let config = if !preset.config.is_empty() {
                                serde_json::from_str(&preset.config)
                                    .unwrap_or(serde_json::Value::Null)
                            } else {
                                serde_json::Value::Null
                            };
                            Ok(PipelineStep::Inline {
                                processor: preset.processor,
                                config,
                            })
                        }
                        Ok(None) => {
                            // Fallback: assume name is processor
                            Ok(PipelineStep::Inline {
                                processor: name.clone(),
                                config: serde_json::Value::Null,
                            })
                        }
                        Err(e) => Err(crate::Error::Database(e.to_string())),
                    }
                } else {
                    // No repo, fallback
                    Ok(PipelineStep::Inline {
                        processor: name.clone(),
                        config: serde_json::Value::Null,
                    })
                }
            }
            PipelineStep::Workflow { name } => {
                // Workflows should be expanded before DAG creation
                Err(crate::Error::Validation(format!(
                    "Workflow '{}' should be resolved before DAG creation. \
                     Expand workflows into individual DAG steps.",
                    name
                )))
            }
            PipelineStep::Inline { .. } => Ok(step.clone()),
        }
    }
}
