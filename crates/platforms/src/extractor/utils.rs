use serde_json::Value;

pub fn parse_bool_from_extras(extras: Option<&Value>, key: &str, default: bool) -> bool {
    extras
        .and_then(|e| e.get(key))
        .and_then(|v| {
            if let Some(b) = v.as_bool() {
                Some(b)
            } else if let Some(s) = v.as_str() {
                s.parse::<bool>().ok()
            } else {
                None
            }
        })
        .unwrap_or(default)
}
