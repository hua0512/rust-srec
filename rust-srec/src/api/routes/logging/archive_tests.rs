use std::sync::Arc;

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use dashmap::DashMap;
use tempfile::TempDir;
use tower::ServiceExt;

use super::*;

fn archive_router(dir: &TempDir) -> (Router, Arc<DashMap<String, chrono::DateTime<chrono::Utc>>>) {
    let tokens = Arc::new(DashMap::new());
    let state = ArchiveRouteState {
        log_dir: dir.path().to_owned(),
        tokens: tokens.clone(),
        archives: Arc::new(LogArchiveService::new()),
    };
    let router = Router::new()
        .route("/archive", get(download_logs_archive))
        .route("/download", get(download_logs_archive))
        .with_state(state);
    (router, tokens)
}

#[tokio::test]
async fn archive_route_preserves_headers_dates_names_and_single_use_tokens() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("rust-srec.log.2026-09-06"), b"excluded").unwrap();
    std::fs::write(dir.path().join("rust-srec.log.2026-09-07"), b"included").unwrap();
    std::fs::write(dir.path().join("unrelated.txt"), b"unrelated").unwrap();
    let (router, tokens) = archive_router(&dir);
    let token = issue_download_token(&tokens).unwrap().token;
    let uri = format!("/archive?token={token}&from=2026-09-07&to=2026-09-07");
    let response = router
        .clone()
        .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/zip");
    assert_eq!(
        response.headers()[header::CONTENT_DISPOSITION],
        "attachment; filename=\"rust-srec-logs-2026-09-07.zip\""
    );
    let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    assert_eq!(zip.len(), 1);
    assert_eq!(zip.by_index(0).unwrap().name(), "rust-srec.log.2026-09-07");
    let reused = router
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(reused.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn archive_route_rejects_expired_tokens_and_invalid_ranges() {
    let dir = TempDir::new().unwrap();
    let (router, tokens) = archive_router(&dir);
    tokens.insert(
        "expired".into(),
        chrono::Utc::now() - chrono::Duration::seconds(1),
    );
    for token in ["expired", "unknown"] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/archive?token={token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    let token = issue_download_token(&tokens).unwrap().token;
    let response = router
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/archive?token={token}&from=2026-09-07&to=2026-09-06"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(!tokens.contains_key(&token));
}

#[tokio::test]
async fn download_alias_shares_capacity_and_returns_retry_after() {
    let dir = TempDir::new().unwrap();
    let (router, tokens) = archive_router(&dir);
    let mut responses = Vec::new();
    for index in 0..3 {
        let token = issue_download_token(&tokens).unwrap().token;
        let path = if index == 0 { "archive" } else { "download" };
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/{path}?token={token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        if index < 2 {
            assert_eq!(response.status(), StatusCode::OK);
            responses.push(response);
        } else {
            assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
            assert_eq!(response.headers()[header::RETRY_AFTER], "5");
        }
        assert!(!tokens.contains_key(&token));
    }
    drop(responses);
}

#[test]
fn scan_entry_limit_is_checked_after_date_filtering() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("rust-srec.log.2026-09-06"), b"first").unwrap();
    std::fs::write(dir.path().join("rust-srec.log.2026-09-07"), b"second").unwrap();
    assert!(scan_log_files_matching(dir.path(), |_| true, 1).is_err());
    let selected =
        scan_log_files_matching(dir.path(), |file| file.date.to_string() == "2026-09-07", 1)
            .unwrap();
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].filename, "rust-srec.log.2026-09-07");
}
