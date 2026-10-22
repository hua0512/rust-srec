//! Requests and request bodies several scenario modules share: reads of
//! streamers, sessions, check history and workflows, and workflow steps.

use reqwest::Method;
use serde_json::{Value, json};

use super::fake_platform::FAKE_PLATFORM_ID;
use super::{FakeRoom, Response, TestApp, WAIT, eventually, id_of};

/// The statuses a workflow ends in.
pub const TERMINAL_DAG_STATUSES: [&str; 3] = ["COMPLETED", "FAILED", "CANCELLED"];

impl TestApp {
    /// GET `path`, panicking unless it answers 200, and return the body.
    pub async fn get_ok(&self, path: &str, token: &str) -> Value {
        self.get(path, token).await.ok()
    }

    /// The `items` of a list route.
    pub async fn get_items(&self, path: &str, token: &str) -> Vec<Value> {
        self.get_ok(path, token).await["items"]
            .as_array()
            .expect("list items")
            .clone()
    }

    pub async fn streamer(&self, token: &str, id: &str) -> Value {
        self.get_ok(&format!("/api/streamers/{id}"), token).await
    }

    /// Create a disabled streamer and return its id.
    pub async fn disabled_streamer(&self, token: &str, name: &str, url: &str) -> String {
        let streamer = json!({ "name": name, "url": url });
        id_of(&self.create_disabled_streamer(token, streamer).await)
    }

    /// Create an enabled streamer watching `room`, with `config` as its
    /// streamer-specific config, and return its id.
    pub async fn watch(&self, token: &str, name: &str, room: &FakeRoom, config: Value) -> String {
        let created = self
            .create_enabled_streamer(
                token,
                json!({ "name": name, "url": room.url(), "streamer_specific_config": config }),
            )
            .await;
        assert_eq!(created["platform_config_id"], FAKE_PLATFORM_ID, "{created}");
        id_of(&created)
    }

    /// The streamer's check history, oldest first.
    pub async fn checks(&self, token: &str, id: &str) -> Vec<Value> {
        self.get_items(&format!("/api/streamers/{id}/check-history"), token)
            .await
    }

    /// Wait until the streamer has `count` more checks recorded than it has
    /// now.
    pub async fn wait_for_more_checks(&self, token: &str, id: &str, count: usize) {
        let target = self.checks(token, id).await.len() + count;
        eventually(WAIT, &format!("{count} more checks of {id}"), || async {
            (self.checks(token, id).await.len() >= target).then_some(())
        })
        .await;
    }

    /// The streamer's sessions, newest first.
    pub async fn sessions(&self, token: &str, streamer_id: &str) -> Vec<Value> {
        let path = format!("/api/sessions?streamer_id={streamer_id}&include_empty=true");
        self.get_items(&path, token).await
    }

    pub async fn session(&self, token: &str, id: &str) -> Value {
        self.get_ok(&format!("/api/sessions/{id}"), token).await
    }

    /// Wait for the streamer's only session to end and return its id.
    pub async fn ended_session_id(&self, token: &str, streamer_id: &str) -> String {
        eventually(WAIT, "the session to end", || async {
            let sessions = self.sessions(token, streamer_id).await;
            assert!(sessions.len() <= 1, "more than one session: {sessions:?}");
            sessions
                .first()
                .filter(|session| session["is_live"] == false)
                .map(id_of)
        })
        .await
    }

    /// Wait for the streamer's only session to end and return its detail.
    pub async fn ended_session(&self, token: &str, streamer_id: &str) -> Value {
        let id = self.ended_session_id(token, streamer_id).await;
        self.session(token, &id).await
    }

    /// The session's segments, oldest first.
    pub async fn segments(&self, token: &str, session_id: &str) -> Vec<Value> {
        let path = format!("/api/sessions/{session_id}/segments");
        let mut segments = self.get_items(&path, token).await;
        segments.sort_by_key(|segment| segment["segment_index"].as_i64());
        segments
    }

    /// The session's workflows.
    pub async fn dags(&self, token: &str, session_id: &str) -> Vec<Value> {
        let path = format!("/api/pipeline/dags?session_id={session_id}");
        self.get_ok(&path, token).await["dags"]
            .as_array()
            .expect("dags")
            .clone()
    }

    pub async fn dag(&self, token: &str, dag_id: &str) -> Value {
        self.get_ok(&format!("/api/pipeline/dag/{dag_id}"), token)
            .await
    }

    /// Send a notification channel's test message.
    pub async fn test_send(&self, token: &str, channel_id: &str) -> Response {
        let path = format!("/api/notifications/channels/{channel_id}/test");
        self.send(Method::POST, &path, Some(token), None).await
    }

    /// Wait until the workflow reaches a terminal status and return its
    /// detail.
    pub async fn settled_dag(&self, token: &str, dag_id: &str) -> Value {
        eventually(WAIT, &format!("workflow {dag_id} to settle"), || async {
            let body = self.dag(token, dag_id).await;
            let status = body["status"].as_str().unwrap_or_default();
            TERMINAL_DAG_STATUSES.contains(&status).then_some(body)
        })
        .await
    }
}

/// A workflow of one `execute` step that succeeds on every OS.
pub fn succeed(name: &str) -> Value {
    json!({
        "name": name,
        "steps": [inline("succeed", "execute", json!({ "command": "exit 0" }), &[])],
    })
}

/// A workflow step running `processor` with `config` after `depends_on`.
pub fn inline(id: &str, processor: &str, config: Value, depends_on: &[&str]) -> Value {
    json!({
        "id": id,
        "step": { "type": "inline", "processor": processor, "config": config },
        "depends_on": depends_on,
    })
}

/// The step `step_id` in a workflow's detail.
#[track_caller]
pub fn step<'a>(dag: &'a Value, step_id: &str) -> &'a Value {
    dag["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .find(|step| step["step_id"] == step_id)
        .unwrap_or_else(|| panic!("step {step_id} in {dag}"))
}
