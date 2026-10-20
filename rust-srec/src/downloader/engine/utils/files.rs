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
