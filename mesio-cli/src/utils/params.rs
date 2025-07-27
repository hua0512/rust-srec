use crate::error::AppError;
use tracing::{debug, error, info};

/// Parses a list of parameter strings into key-value pairs.
///
/// Each parameter string should be in the format "key=value". The function
/// splits each parameter at the first '=' character and returns a vector
/// of tuples containing the key and value as strings.
///
/// # Arguments
///
/// * `params` - A slice of strings, each representing a parameter in "key=value" format
///
/// # Returns
///
/// Returns `Ok(Vec<(String, String)>)` containing the parsed key-value pairs,
/// or `Err(AppError)` if any parameter is not in the correct format.
///
/// # Errors
///
/// Returns `AppError::InvalidInput` if any parameter string does not contain
/// an '=' character to separate the key from the value.
///
/// # Examples
///
/// ```
/// use mesio::utils::parse_params;
///
/// let params = vec![
///     "key1=value1".to_string(),
///     "key2=value2".to_string(),
/// ];
/// let result = parse_params(&params).unwrap();
/// assert_eq!(result, vec![
///     ("key1".to_string(), "value1".to_string()),
///     ("key2".to_string(), "value2".to_string()),
/// ]);
/// ```
pub fn parse_params(params: &[String]) -> Result<Vec<(String, String)>, AppError> {
    debug!("Parsing {} parameters", params.len());

    let result: Result<Vec<(String, String)>, AppError> = params
        .iter()
        .map(|param| {
            debug!("Parsing parameter: {param}");
            param
                .split_once('=')
                .map(|(key, value)| {
                    info!("Added parameter: key='{key}', value='{value}'");
                    (key.to_string(), value.to_string())
                })
                .ok_or_else(|| {
                    error!("Invalid param format: {param}");
                    AppError::InvalidInput(format!("Invalid param format: {param}"))
                })
        })
        .collect();

    match &result {
        Ok(params) => debug!("Successfully parsed {} parameters", params.len()),
        Err(e) => error!("Failed to parse parameters: {e}"),
    }

    result
}
