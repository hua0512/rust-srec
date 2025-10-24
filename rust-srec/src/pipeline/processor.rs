use super::{
    action::{Action, ActionType, FinalOutput, Input, Output},
    actions::{
        CopyAction, ExecuteCommandAction, FileDeletionAction, RemuxAction, ThumbnailGenerationAction,
        UploadAction,
    },
};
use crate::metrics::PIPELINE_THROUGHPUT;
use async_trait::async_trait;
use std::path::PathBuf;
use tracing::error;

#[async_trait]
pub trait PipelineProcessor {
    async fn run(&self, pipeline: Vec<ActionType>, initial_input: PathBuf) -> Option<FinalOutput>;
}

pub struct PipelineProcessorImpl;

#[async_trait]
impl PipelineProcessor for PipelineProcessorImpl {
    async fn run(&self, pipeline: Vec<ActionType>, initial_input: PathBuf) -> Option<FinalOutput> {
        let mut current_input = Input::File(initial_input);
        let mut final_output: Option<FinalOutput> = None;

        for action_type in pipeline {
            let action: Box<dyn Action> = match action_type {
                ActionType::Copy(config) => Box::new(CopyAction(config)),
                ActionType::ExecuteCommand(config) => Box::new(ExecuteCommandAction(config)),
                ActionType::Remux(config) => Box::new(RemuxAction(config)),
                ActionType::Upload(config) => Box::new(UploadAction(config)),
                ActionType::ThumbnailGeneration(config) => {
                    Box::new(ThumbnailGenerationAction(config))
                }
                ActionType::FileDeletion(config) => Box::new(FileDeletionAction(config)),
            };

            match action.run(current_input).await {
                Ok(output) => {
                    PIPELINE_THROUGHPUT.inc();
                    match output {
                        Output::File(path) => {
                            current_input = Input::File(path);
                        }
                        Output::Final(output) => {
                            final_output = Some(output);
                            break;
                        }
                    }
                }
                Err(e) => {
                    error!("Error executing action: {:?}", e);
                    return None;
                }
            }
        }
        final_output
    }
}