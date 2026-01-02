//! File utility functions for download engines.

use std::path::Path;

use crate::Result;
use crate::utils::fs;

/// Ensure the output directory exists, creating it if necessary.
///
/// # Arguments
/// * `path` - The directory path to ensure exists
///
/// # Returns
/// * `Ok(())` - If the directory exists or was created successfully
/// * `Err(crate::Error)` - If the directory could not be created
///
pub async fn ensure_output_dir(path: &Path) -> Result<()> {
    fs::ensure_dir_all_with_op("creating output directory", path).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_ensure_output_dir_creates_new_directory() {
        let temp = TempDir::new().unwrap();
        let new_dir = temp.path().join("new_subdir");

        assert!(!new_dir.exists());
        let result = ensure_output_dir(&new_dir).await;
        assert!(result.is_ok());
        assert!(new_dir.exists());
    }

    #[tokio::test]
    async fn test_ensure_output_dir_succeeds_for_existing_directory() {
        let temp = TempDir::new().unwrap();
        let existing_dir = temp.path();

        assert!(existing_dir.exists());
        let result = ensure_output_dir(existing_dir).await;
        assert!(result.is_ok());
        assert!(existing_dir.exists());
    }

    #[tokio::test]
    async fn test_ensure_output_dir_creates_nested_directories() {
        let temp = TempDir::new().unwrap();
        let nested_dir = temp.path().join("level1").join("level2").join("level3");

        assert!(!nested_dir.exists());
        let result = ensure_output_dir(&nested_dir).await;
        assert!(result.is_ok());
        assert!(nested_dir.exists());
    }
}
