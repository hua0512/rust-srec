use std::path::Path;

use crate::error::AppError;

#[inline]
pub async fn create_dirs(path: &Path) -> Result<(), AppError> {
    tokio::fs::create_dir_all(path)
        .await
        .map_err(AppError::Io)?;
    Ok(())
}
