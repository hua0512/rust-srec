use std::sync::Arc;

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use dashmap::DashMap;
use tempfile::TempDir;
use tower::ServiceExt;

use super::*;

fn archive_router(dir: &TempDir) -> (Router, Arc<DashMap<String, LoggingArchiveGrant>>) {
    let tokens = Arc::new(DashMap::new());
    let state = ArchiveRouteState {
        log_dir: dir.path().to_owned(),
        tokens: tokens.clone(),
        auth_service: None,
        archives: Arc::new(LogArchiveService::new()),
    };
    let router = Router::new()
        .route("/archive", get(download_logs_archive))
        .route("/download", get(download_logs_archive))
        .with_state(state);
    (router, tokens)
}

#[tokio::test]
async fn archive_grants_revalidate_the_issuing_session_or_key_and_remain_single_use() {
    use crate::api::auth_request::{AccessPolicy, authorize_request, tests::fixture};
    use crate::database::models::ApiKeyAccessLevel;
    let fixture = fixture().await;
    let tokens = DashMap::new();
    let headers = HeaderMap::new();
    let principal = authorize_request(
        Some(&fixture.service),
        &headers,
        Some(&fixture.access_token),
        AccessPolicy::Full,
    )
    .await
    .unwrap();
    let valid = issue_download_token(&tokens, principal.clone())
        .unwrap()
        .token;
    consume_download_token(&tokens, &valid, Some(&fixture.service))
        .await
        .unwrap();
    assert!(
        consume_download_token(&tokens, &valid, Some(&fixture.service))
            .await
            .is_err()
    );
    let revoked = issue_download_token(&tokens, principal).unwrap().token;
    fixture
        .service
        .logout(&fixture.refresh_token)
        .await
        .unwrap();
    assert!(
        consume_download_token(&tokens, &revoked, Some(&fixture.service))
            .await
            .is_err()
    );
    assert!(!tokens.contains_key(&revoked));
    let (key, raw) = fixture
        .service
        .create_api_key(
            &fixture.user_id,
            "archive-full",
            ApiKeyAccessLevel::Full,
            None,
        )
        .await
        .unwrap();
    let principal = authorize_request(
        Some(&fixture.service),
        &headers,
        Some(&raw),
        AccessPolicy::Full,
    )
    .await
    .unwrap();
    let revoked = issue_download_token(&tokens, principal).unwrap().token;
    fixture
        .service
        .revoke_api_key(&fixture.user_id, &key.id)
        .await
        .unwrap();
    assert!(
        consume_download_token(&tokens, &revoked, Some(&fixture.service))
            .await
            .is_err()
    );
    fixture.pool.close().await;
}

#[tokio::test]
async fn archive_route_preserves_headers_dates_names_and_single_use_tokens() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("rust-srec.log.2026-09-06"), b"excluded").unwrap();
    std::fs::write(dir.path().join("rust-srec.log.2026-09-07"), b"included").unwrap();
    std::fs::write(dir.path().join("unrelated.txt"), b"unrelated").unwrap();
    let (router, tokens) = archive_router(&dir);
    let token = issue_download_token(&tokens, None).unwrap().token;
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
        LoggingArchiveGrant {
            expires_at: chrono::Utc::now() - chrono::Duration::seconds(1),
            principal: None,
        },
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
    let token = issue_download_token(&tokens, None).unwrap().token;
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
        let token = issue_download_token(&tokens, None).unwrap().token;
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

#[test]
fn numbered_segments_use_filename_dates_and_preserve_file_selection() {
    let dir = TempDir::new().unwrap();
    let names = [
        "rust-srec.log.2001-01-01",
        "rust-srec.log.2001-01-01.00000000000000000002",
        "rust-srec.log.2001-01-01.00000000000000000003",
    ];
    for name in names {
        std::fs::write(dir.path().join(name), b"line\n").unwrap();
    }
    let date = chrono::NaiveDate::from_ymd_opt(2001, 1, 1).unwrap();
    let files = filter_by_range(scan_log_files(dir.path()).unwrap(), Some(date), Some(date));
    assert_eq!(
        files
            .iter()
            .map(|file| file.filename.as_str())
            .collect::<Vec<_>>(),
        names
    );
    assert!(files.iter().all(|file| file.date == date));
    let selected = filter_by_file_name(files, names[1]);
    let lines = list_log_lines(selected, 0, 10, None).unwrap();
    assert_eq!(lines.items[0].filename, names[1]);
    assert_eq!(lines.items[0].text, "line");
}
