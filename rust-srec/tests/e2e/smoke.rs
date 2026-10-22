use reqwest::StatusCode;

use crate::support::TestApp;

#[tokio::test]
async fn backend_starts_and_the_admin_signs_in() {
    let app = TestApp::start().await;

    let token = app.admin_token().await;
    let me = app.get("/api/streamers", &token).await;
    me.expect_status(StatusCode::OK);

    app.shutdown().await;
}
