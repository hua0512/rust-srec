use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use tracing::info;

/// Parse a header string in format "Name: Value" and add it to the HeaderMap
pub fn parse_and_add_header(headers: &mut HeaderMap, header_str: &str) {
    // Find the first colon which separates name and value
    if let Some(colon_pos) = header_str.find(':') {
        let name = header_str[..colon_pos].trim();
        let value = header_str[colon_pos + 1..].trim();

        // Try to create a header name and value
        if let Ok(header_name) = HeaderName::from_bytes(name.as_bytes()) {
            if let Ok(header_value) = HeaderValue::from_str(value) {
                info!("Adding header: {}: {}", name, value);
                headers.insert(header_name, header_value);
                return;
            }
        } else {
            tracing::warn!("Invalid header name: '{}'", name);
        }
    }

    // Log error if header format is invalid
    tracing::warn!(
        "Invalid header format: '{}'. Expected 'Name: Value'",
        header_str
    );
}

/// Parse a collection of header strings and return a HeaderMap
pub fn parse_headers(header_strings: &[String]) -> HeaderMap {
    let mut headers = HeaderMap::new();

    for header_str in header_strings {
        parse_and_add_header(&mut headers, header_str);
    }

    headers
}
