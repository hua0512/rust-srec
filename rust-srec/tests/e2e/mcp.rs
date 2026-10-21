//! The MCP endpoint: tool listing and per-key access.
//!
//! The endpoint is stateless: every JSON-RPC request is a POST on its own,
//! authenticated by its bearer API key.

use std::collections::HashSet;

use reqwest::{StatusCode, header};
use serde_json::{Value, json};

use crate::support::TestApp;

/// Send one JSON-RPC request to `/api/mcp` with `key` and return the reply.
async fn mcp(app: &TestApp, key: &str, method: &str, params: Value) -> Value {
    let response = app
        .client()
        .post(app.url("/api/mcp"))
        .bearer_auth(key)
        .header(header::ACCEPT, "application/json, text/event-stream")
        .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }))
        .send()
        .await
        .expect("send the MCP request");
    TestApp::read(response)
        .await
        .expect_status(StatusCode::OK)
        .body
}

#[tokio::test]
async fn tools_are_listed_for_any_key_but_write_tools_need_a_full_key() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let reader = app.create_api_key(&token, "mcp reader", "read_only").await;
    let writer = app.create_api_key(&token, "mcp writer", "full").await;

    // Every group is listed once, each tool with a generated input schema.
    let listed = mcp(&app, &reader.raw, "tools/list", json!({})).await;
    let tools = listed["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("no tools: {listed}"));
    let names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().expect("tool name"))
        .collect();
    for expected in [
        "config_get_global",
        "config_update_global",
        "config_list_platforms",
        "template_list",
        "engine_list",
        "streamer_list",
        "streamer_create",
        "filter_list",
        "session_list",
        "session_danmu_statistics",
        "session_list_danmu_files",
        "session_read_danmu",
        "pipeline_stats",
        "pipeline_list_jobs",
        "pipeline_list_dags",
        "notification_list_channels",
        "system_health",
        "parse_url",
    ] {
        assert!(
            names.contains(&expected),
            "missing tool {expected}: {names:?}"
        );
    }
    let unique: HashSet<&str> = names.iter().copied().collect();
    assert_eq!(unique.len(), names.len(), "duplicate tool names: {names:?}");
    for tool in tools {
        assert_eq!(tool["inputSchema"]["type"], "object", "{tool}");
    }

    let update = json!({
        "name": "config_update_global",
        "arguments": { "updates": { "max_concurrent_downloads": 4 } },
    });
    let downloads = || async {
        app.get("/api/config/global", &token)
            .await
            .expect_status(StatusCode::OK)
            .body["max_concurrent_downloads"]
            .clone()
    };
    let before = downloads().await;
    assert_ne!(before, 4);

    let refused = mcp(&app, &reader.raw, "tools/call", update.clone()).await;
    assert!(refused["result"].is_null(), "{refused}");
    // A JSON-RPC "invalid request" error.
    assert_eq!(refused["error"]["code"], -32600, "{refused}");
    assert!(
        refused["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("full-access")),
        "{refused}"
    );
    assert_eq!(downloads().await, before);

    // A read-only key still runs query tools.
    let sessions = mcp(
        &app,
        &reader.raw,
        "tools/call",
        json!({ "name": "session_list", "arguments": {} }),
    )
    .await;
    assert_eq!(sessions["result"]["isError"], false, "{sessions}");

    let updated = mcp(&app, &writer.raw, "tools/call", update).await;
    assert_eq!(updated["result"]["isError"], false, "{updated}");
    assert_eq!(downloads().await, 4);

    app.shutdown().await;
}
