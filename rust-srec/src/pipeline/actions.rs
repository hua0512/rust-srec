use super::action::{
    Action, CopyActionConfig, ExecuteCommandActionConfig, FileDeletionActionConfig, FinalOutput,
    Input, Output, RemuxActionConfig, ThumbnailGenerationActionConfig, UploadActionConfig,
};
use async_trait::async_trait;
use std::path::Path;
use tokio::process::Command;
use tracing::info;

pub struct CopyAction(pub CopyActionConfig);

#[async_trait]
impl Action for CopyAction {
    async fn run(&self, input: Input) -> Result<Output, anyhow::Error> {
        let Input::File(path) = input;
        let to_path = Path::new(&self.0.to);
        info!("Copying file from {:?} to {:?}", path, to_path);
        tokio::fs::copy(&path, to_path).await?;
        Ok(Output::File(to_path.to_path_buf()))
    }
}

pub struct ExecuteCommandAction(pub ExecuteCommandActionConfig);

#[async_trait]
impl Action for ExecuteCommandAction {
    async fn run(&self, input: Input) -> Result<Output, anyhow::Error> {
        let Input::File(path) = input;
        info!(
            "Executing command: {} with args: {:?}",
            self.0.command, self.0.args
        );
        let status = Command::new(&self.0.command)
            .args(&self.0.args)
            .arg(&path)
            .status()
            .await?;
        if status.success() {
            Ok(Output::File(path))
        } else {
            Err(anyhow::anyhow!(
                "Command failed with status: {}",
                status
            ))
        }
    }
}

pub struct RemuxAction(pub RemuxActionConfig);

#[async_trait]
impl Action for RemuxAction {
    async fn run(&self, input: Input) -> Result<Output, anyhow::Error> {
        let Input::File(path) = input;
        let output_path = path.with_extension(&self.0.format);
        info!(
            "Remuxing file: {:?} to format: {}",
            path, self.0.format
        );
        let status = Command::new("ffmpeg")
            .arg("-i")
            .arg(&path)
            .arg("-c")
            .arg("copy")
            .arg(&output_path)
            .status()
            .await?;
        if status.success() {
            Ok(Output::File(output_path))
        } else {
            Err(anyhow::anyhow!(
                "FFmpeg remuxing failed with status: {}",
                status
            ))
        }
    }
}

pub struct UploadAction(pub UploadActionConfig);

#[async_trait]
impl Action for UploadAction {
    async fn run(&self, input: Input) -> Result<Output, anyhow::Error> {
        let Input::File(path) = input;
        info!("Uploading file: {:?} to {}", path, self.0.remote);
        // Simulate upload with rclone
        let status = Command::new("rclone")
            .arg("copy")
            .arg(&path)
            .arg(&self.0.remote)
            .status()
            .await?;
        if status.success() {
            let final_output = FinalOutput {
                url: format!(
                    "{}/{}",
                    self.0.remote,
                    path.file_name().unwrap().to_str().unwrap()
                ),
                metadata: Default::default(),
            };
            Ok(Output::Final(final_output))
        } else {
            Err(anyhow::anyhow!(
                "Rclone upload failed with status: {}",
                status
            ))
        }
    }
}

pub struct ThumbnailGenerationAction(pub ThumbnailGenerationActionConfig);

#[async_trait]
impl Action for ThumbnailGenerationAction {
    async fn run(&self, input: Input) -> Result<Output, anyhow::Error> {
        let Input::File(path) = input;
        info!("Generating {} thumbnails for {:?}", self.0.count, path);
        let output_pattern = path.with_extension("").join("thumbnail_%d.jpg");
        let status = Command::new("ffmpeg")
            .arg("-i")
            .arg(&path)
            .arg("-vf")
            .arg("fps=1/60,scale=320:-1")
            .arg("-vframes")
            .arg(self.0.count.to_string())
            .arg(output_pattern)
            .status()
            .await?;
        if status.success() {
            Ok(Output::File(path))
        } else {
            Err(anyhow::anyhow!(
                "Thumbnail generation failed with status: {}",
                status
            ))
        }
    }
}

pub struct FileDeletionAction(pub FileDeletionActionConfig);

#[async_trait]
impl Action for FileDeletionAction {
    async fn run(&self, input: Input) -> Result<Output, anyhow::Error> {
        info!("Deleting file: {:?}", self.0.target);
        tokio::fs::remove_file(&self.0.target).await?;
        let Input::File(path) = input;
        Ok(Output::File(path))
    }
}