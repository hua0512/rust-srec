use async_trait::async_trait;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ActionType {
    Copy(CopyActionConfig),
    ExecuteCommand(ExecuteCommandActionConfig),
    Remux(RemuxActionConfig),
    Upload(UploadActionConfig),
    ThumbnailGeneration(ThumbnailGenerationActionConfig),
    FileDeletion(FileDeletionActionConfig),
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CopyActionConfig {
    pub to: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ExecuteCommandActionConfig {
    pub command: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RemuxActionConfig {
    pub format: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct UploadActionConfig {
    pub remote: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ThumbnailGenerationActionConfig {
    pub count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FileDeletionActionConfig {
    pub target: PathBuf,
}

pub enum Input {
    File(Box<Path>),
}

pub enum Output {
    File(PathBuf),
    Final(FinalOutput),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FinalOutput {
    pub url: String,
    pub metadata: HashMap<String, String>,
}

#[async_trait]
pub trait Action: Send + Sync {
    async fn run(&self, input: Input) -> anyhow::Result<Output>;
}