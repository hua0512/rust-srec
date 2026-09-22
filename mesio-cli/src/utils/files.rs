use crate::error::AppError;

/// Extracts a filename from a URL, removing the file extension and truncating if too long.
///
/// # Arguments
///
/// * `url_str` - The URL string to extract the filename from
///
/// # Returns
///
/// * `Ok(String)` containing the extracted filename (without extension, max 30 chars)
/// * `Err(AppError::InvalidInput)` if the URL is malformed
///
/// # Examples
///
/// ```
/// let filename = extract_filename_from_url("https://example.com/video.mp4")?;
/// assert_eq!(filename, "video");
/// ```
pub fn extract_filename_from_url(url_str: &str) -> Result<String, AppError> {
    let url = url_str
        .parse::<reqwest::Url>()
        .map_err(|e| AppError::InvalidInput(e.to_string()))?;

    let file_name = url
        .path_segments()
        .and_then(|mut s| s.next_back())
        .unwrap_or("stream");

    let url_name = match file_name.rfind('.') {
        Some(pos) => &file_name[..pos],
        None => file_name,
    };

    // we dont want large filenames
    let filename = if url_name.len() > 30 {
        format!("{}...", &url_name[..27])
    } else {
        url_name.to_string()
    };

    Ok(filename)
}

/// Expands a name template by replacing the `%u` placeholder with the filename extracted from a URL.
///
/// # Arguments
///
/// * `name_template` - The template string containing `%u` placeholder
/// * `url_str` - The URL string to extract the filename from
///
/// # Returns
///
/// * `Ok(String)` containing the expanded name with URL filename substituted
/// * `Err(AppError)` if the URL is malformed or filename extraction fails
///
/// # Examples
///
/// ```
/// let expanded = expand_name_url("episode_%u", "https://example.com/video.mp4")?;
/// assert_eq!(expanded, "episode_video");
/// ```
pub fn expand_name_url(name_template: &str, url_str: &str) -> Result<String, AppError> {
    let url_name = extract_filename_from_url(url_str)?;
    let base_name = name_template.replace("%u", &url_name);
    Ok(base_name)
}
