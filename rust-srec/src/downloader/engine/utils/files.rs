//! File utility functions for download engines.

use std::path::Path;
use tokio::fs;

/// Ensure the output directory exists, creating it if necessary.
///
/// # Arguments
/// * `path` - The directory path to ensure exists
///
/// # Returns
/// * `Ok(())` - If the directory exists or was created successfully
/// * `Err(String)` - If the directory could not be created, with a descriptive error message
///
/// # Requirements
/// This function satisfies Requirements 2.1, 2.2, 2.3:
/// - Creates the output directory if it doesn't exist (2.1)
/// - Returns a descriptive error message if creation fails (2.2)
/// - Succeeds without error if the directory already exists (2.3)
pub async fn ensure_output_dir(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path)
        .await
        .map_err(|e| format!("Failed to create output directory {:?}: {}", path, e))
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
