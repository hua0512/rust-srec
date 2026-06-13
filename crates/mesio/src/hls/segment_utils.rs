use url::Url;

/// Determine if a segment URL represents an M4S (fMP4) segment
pub fn is_m4s_segment(url: &Url) -> bool {
    let path = url.path().to_lowercase();
    let query = url.query().unwrap_or("").to_lowercase();

    path.ends_with(".m4s")
        || path.ends_with(".mp4")
        || path.ends_with(".cmfv")
        || query.contains("format=mp4")
        || query.contains("fmt=mp4")
}
