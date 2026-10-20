//! Pipeline presets, DAG validation, jobs and workflows over HTTP.
//!
//! Workflows that actually run use only processors that work offline:
//! `execute` with `exit 0` / `exit 1` (valid under both `sh -c` and
//! `cmd /C`), `copy_move`, `delete` and `compression`, on files under the
//! app's output directory. Scenarios that need a job to stay running use
//! `sleep 30` and are therefore `#[cfg(unix)]`.
//!
//! A workflow that must stay queued without running is seeded through SQL with
//! a job type no worker pool claims, the state a workflow is in while every
//! worker is busy.

use std::collections::BTreeSet;
use std::path::Path;

use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::support::{
    Response, TestApp, WAIT, assert_recent, eventually, id_of, millis, now_ms, sorted_keys,
};

/// A job type no worker pool claims, so a seeded job keeps its status.
const UNCLAIMED_JOB_TYPE: &str = "e2e_unclaimed";

const TERMINAL_DAG_STATUSES: [&str; 3] = ["COMPLETED", "FAILED", "CANCELLED"];

fn inline(id: &str, processor: &str, config: Value, depends_on: &[&str]) -> Value {
    json!({
        "id": id,
        "step": { "type": "inline", "processor": processor, "config": config },
        "depends_on": depends_on,
    })
}

fn execute(id: &str, command: &str, depends_on: &[&str]) -> Value {
    inline(id, "execute", json!({ "command": command }), depends_on)
}

fn definition(name: &str, steps: Vec<Value>) -> Value {
    json!({ "name": name, "steps": steps })
}

fn message(response: &Response) -> &str {
    response.body["message"].as_str().unwrap_or_default()
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .unwrap_or_else(|| panic!("expected an array, got {value}"))
        .iter()
        .map(|item| item.as_str().expect("string item").to_string())
        .collect()
}

async fn validate(app: &TestApp, token: &str, dag: Value) -> Value {
    app.post("/api/pipeline/validate", token, &json!({ "dag": dag }))
        .await
        .expect_status(StatusCode::OK)
        .body
}

async fn create_pipeline(app: &TestApp, token: &str, inputs: &[String], dag: Value) -> Response {
    app.post(
        "/api/pipeline/create",
        token,
        &json!({
            "session_id": "session-e2e",
            "streamer_id": "streamer-e2e",
            "input_paths": inputs,
            "dag": dag,
        }),
    )
    .await
}

/// Create a workflow and return its DAG id.
async fn start_pipeline(app: &TestApp, token: &str, inputs: &[String], dag: Value) -> String {
    let created = create_pipeline(app, token, inputs, dag)
        .await
        .expect_status(StatusCode::CREATED);
    created.body["pipeline_id"]
        .as_str()
        .expect("pipeline_id")
        .to_string()
}

async fn dag(app: &TestApp, token: &str, dag_id: &str) -> Value {
    app.get(&format!("/api/pipeline/dag/{dag_id}"), token)
        .await
        .expect_status(StatusCode::OK)
        .body
}

/// Wait until the workflow reaches a terminal status and return its detail.
async fn settled_dag(app: &TestApp, token: &str, dag_id: &str) -> Value {
    eventually(WAIT, &format!("DAG {dag_id} to settle"), || async {
        let body = dag(app, token, dag_id).await;
        let status = body["status"].as_str().unwrap_or_default();
        TERMINAL_DAG_STATUSES.contains(&status).then_some(body)
    })
    .await
}

/// Run a workflow to its end and return its id with its settled detail.
async fn run_pipeline(
    app: &TestApp,
    token: &str,
    inputs: &[String],
    dag: Value,
) -> (String, Value) {
    let dag_id = start_pipeline(app, token, inputs, dag).await;
    let settled = settled_dag(app, token, &dag_id).await;
    (dag_id, settled)
}

fn step<'a>(dag: &'a Value, step_id: &str) -> &'a Value {
    dag["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .find(|step| step["step_id"] == step_id)
        .unwrap_or_else(|| panic!("step {step_id} in {dag}"))
}

fn step_job_id(dag: &Value, step_id: &str) -> String {
    step(dag, step_id)["job_id"]
        .as_str()
        .unwrap_or_else(|| panic!("step {step_id} has a job: {dag}"))
        .to_string()
}

async fn job(app: &TestApp, token: &str, job_id: &str) -> Value {
    app.get(&format!("/api/pipeline/jobs/{job_id}"), token)
        .await
        .expect_status(StatusCode::OK)
        .body
}

/// Seed a standalone job (one that belongs to no workflow) in `status`.
async fn seed_standalone_job(app: &TestApp, id: &str, status: &str, retry_count: i64) {
    let now = now_ms();
    let started = if status == "PENDING" {
        "NULL".to_string()
    } else {
        (now - 2_000).to_string()
    };
    let completed = if matches!(status, "COMPLETED" | "FAILED" | "CANCELLED") {
        (now - 1_000).to_string()
    } else {
        "NULL".to_string()
    };
    let error = match status {
        "FAILED" => "'seeded failure'",
        "CANCELLED" => "'Job cancelled'",
        _ => "NULL",
    };
    app.run_sql(&format!(
        "INSERT INTO job (id, job_type, status, config, state, created_at, updated_at, input, \
             outputs, priority, streamer_id, session_id, started_at, completed_at, error, \
             retry_count) VALUES ('{id}', '{UNCLAIMED_JOB_TYPE}', '{status}', '{{}}', '{{}}', \
             {created}, {now}, '[\"/recordings/{id}.flv\"]', '[]', 0, 'streamer-seeded', \
             'session-seeded', {started}, {completed}, {error}, {retry_count});",
        created = now - 3_000,
    ))
    .await;
}

/// Seed a workflow whose only step is queued behind busy workers: the DAG
/// and step are PROCESSING and the step's job is PENDING. Returns the job id.
async fn seed_queued_dag(app: &TestApp, dag_id: &str) -> String {
    let now = now_ms();
    let job_id = format!("{dag_id}-job");
    let step_row_id = format!("{dag_id}-step");
    let definition = json!({
        "name": format!("Queued {dag_id}"),
        "steps": [{
            "id": "queued",
            "step": { "type": "inline", "processor": UNCLAIMED_JOB_TYPE, "config": null },
            "depends_on": [],
        }],
    })
    .to_string();
    app.run_sql(&format!(
        "INSERT INTO dag_execution (id, dag_definition, status, streamer_id, session_id, \
             created_at, updated_at, total_steps, completed_steps, failed_steps) VALUES \
             ('{dag_id}', '{definition}', 'PROCESSING', 'streamer-seeded', 'session-seeded', \
             {now}, {now}, 1, 0, 0);
             INSERT INTO job (id, job_type, status, config, state, created_at, updated_at, input, \
             outputs, priority, streamer_id, session_id, retry_count, pipeline_id) VALUES \
             ('{job_id}', '{UNCLAIMED_JOB_TYPE}', 'PENDING', '{{}}', '{{}}', {now}, {now}, \
             '[\"/recordings/queued.flv\"]', '[]', 0, 'streamer-seeded', 'session-seeded', 0, \
             '{dag_id}');
             INSERT INTO dag_step_execution (id, dag_id, step_id, job_id, status, \
             depends_on_step_ids, created_at, updated_at) VALUES ('{step_row_id}', '{dag_id}', \
             'queued', '{job_id}', 'PROCESSING', '[]', {now}, {now});
             UPDATE job SET dag_step_execution_id = '{step_row_id}' WHERE id = '{job_id}';"
    ))
    .await;
    job_id
}

// ---------------------------------------------------------------------------
// POST /api/pipeline/validate
// ---------------------------------------------------------------------------

#[tokio::test]
async fn validate_rejects_malformed_graphs_with_the_messages_the_editor_shows() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;

    let empty = validate(&app, &token, definition("empty", vec![])).await;
    assert_eq!(
        empty,
        json!({
            "valid": false,
            "errors": ["DAG must have at least one step"],
            "warnings": [],
            "root_steps": [],
            "leaf_steps": [],
            "max_depth": 0,
        })
    );

    let duplicate = validate(
        &app,
        &token,
        definition(
            "duplicate",
            vec![execute("a", "exit 0", &[]), execute("a", "exit 0", &[])],
        ),
    )
    .await;
    assert_eq!(duplicate["valid"], false);
    assert_eq!(duplicate["errors"], json!(["Duplicate step ID: a"]));

    let self_dependency = validate(
        &app,
        &token,
        definition(
            "self",
            vec![
                execute("root", "exit 0", &[]),
                execute("a", "exit 0", &["a"]),
            ],
        ),
    )
    .await;
    assert_eq!(self_dependency["valid"], false);
    assert_eq!(
        self_dependency["errors"],
        json!(["Step 'a' depends on itself"])
    );

    let missing = validate(
        &app,
        &token,
        definition(
            "missing",
            vec![
                execute("a", "exit 0", &[]),
                execute("b", "exit 0", &["ghost"]),
            ],
        ),
    )
    .await;
    assert_eq!(missing["valid"], false);
    assert_eq!(
        missing["errors"],
        json!(["Step 'b' depends on non-existent step 'ghost'"])
    );

    let no_root = validate(
        &app,
        &token,
        definition(
            "no-root",
            vec![
                execute("a", "exit 0", &["b"]),
                execute("b", "exit 0", &["a"]),
            ],
        ),
    )
    .await;
    assert_eq!(no_root["valid"], false);
    assert_eq!(no_root["root_steps"], json!([]));
    assert_eq!(
        no_root["errors"],
        json!([
            "DAG has no root steps (all steps have dependencies)",
            "Cycle detected involving: a -> b",
        ])
    );

    // A cycle below a sound root. Structural errors suppress the creation
    // checks, so the unknown processor on the root is not reported yet.
    let cycle = validate(
        &app,
        &token,
        definition(
            "cycle",
            vec![
                inline("root", "transmogrify", json!({}), &[]),
                execute("a", "exit 0", &["root", "b"]),
                execute("b", "exit 0", &["a"]),
            ],
        ),
    )
    .await;
    assert_eq!(cycle["valid"], false);
    assert_eq!(cycle["root_steps"], json!(["root"]));
    assert_eq!(cycle["errors"], json!(["Cycle detected involving: a -> b"]));

    let oversized_steps: Vec<Value> = (0..1001)
        .map(|index| execute(&format!("s{index}"), "exit 0", &[]))
        .collect();
    let oversized = validate(&app, &token, definition("oversized", oversized_steps)).await;
    assert_eq!(
        oversized,
        json!({
            "valid": false,
            "errors": ["DAG has 1001 steps, maximum allowed is 1000"],
            "warnings": [],
            "root_steps": [],
            "leaf_steps": [],
            "max_depth": 0,
        })
    );

    app.shutdown().await;
}

#[tokio::test]
async fn validate_reports_roots_leaves_and_depth_for_chains_fan_out_and_fan_in() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;

    let chain = validate(
        &app,
        &token,
        definition(
            "chain",
            vec![
                execute("a", "exit 0", &[]),
                execute("b", "exit 0", &["a"]),
                execute("c", "exit 0", &["b"]),
            ],
        ),
    )
    .await;
    assert_eq!(
        chain,
        json!({
            "valid": true,
            "errors": [],
            "warnings": [],
            "root_steps": ["a"],
            "leaf_steps": ["c"],
            "max_depth": 3,
        })
    );

    let fan_out = validate(
        &app,
        &token,
        definition(
            "fan-out",
            vec![
                execute("a", "exit 0", &[]),
                execute("b", "exit 0", &["a"]),
                execute("c", "exit 0", &["a"]),
            ],
        ),
    )
    .await;
    assert_eq!(fan_out["valid"], true);
    assert_eq!(fan_out["root_steps"], json!(["a"]));
    assert_eq!(fan_out["leaf_steps"], json!(["b", "c"]));
    assert_eq!(fan_out["max_depth"], 2);

    let fan_in = validate(
        &app,
        &token,
        definition(
            "fan-in",
            vec![
                execute("a", "exit 0", &[]),
                execute("b", "exit 0", &[]),
                execute("c", "exit 0", &["a", "b"]),
            ],
        ),
    )
    .await;
    assert_eq!(fan_in["valid"], true);
    assert_eq!(fan_in["root_steps"], json!(["a", "b"]));
    assert_eq!(fan_in["leaf_steps"], json!(["c"]));
    assert_eq!(fan_in["max_depth"], 2);

    let single = validate(
        &app,
        &token,
        definition("single", vec![execute("only", "exit 0", &[])]),
    )
    .await;
    assert_eq!(
        single,
        json!({
            "valid": true,
            "errors": [],
            "warnings": ["DAG has only one step - consider if a pipeline is necessary"],
            "root_steps": ["only"],
            "leaf_steps": ["only"],
            "max_depth": 1,
        })
    );

    let deep_steps: Vec<Value> = (0..11)
        .map(|index| {
            let previous = format!("s{}", index.max(1) - 1);
            let depends_on: Vec<&str> = if index == 0 {
                vec![]
            } else {
                vec![previous.as_str()]
            };
            execute(&format!("s{index}"), "exit 0", &depends_on)
        })
        .collect();
    let deep = validate(&app, &token, definition("deep", deep_steps)).await;
    assert_eq!(deep["valid"], true);
    assert_eq!(deep["max_depth"], 11);
    assert_eq!(
        deep["warnings"],
        json!(["DAG has depth 11 - deep pipelines may be slow"])
    );

    app.shutdown().await;
}

#[tokio::test]
async fn validate_runs_the_creation_checks_once_the_graph_is_sound() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;

    let unknown = validate(
        &app,
        &token,
        definition(
            "unknown",
            vec![inline("odd", "transmogrify", json!({}), &[])],
        ),
    )
    .await;
    assert_eq!(unknown["valid"], false);
    let errors = strings(&unknown["errors"]);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].starts_with(
            "Pipeline references unknown processor(s): 'transmogrify' (step 'odd'). Available processors: "
        ),
        "{errors:?}"
    );

    // A preset name that matches no job preset is taken as a processor name.
    let missing_preset = validate(
        &app,
        &token,
        definition(
            "missing-preset",
            vec![json!({ "id": "a", "step": { "type": "preset", "name": "no_such_job_preset" } })],
        ),
    )
    .await;
    assert_eq!(missing_preset["valid"], false);
    assert!(
        strings(&missing_preset["errors"])[0]
            .contains("unknown processor(s): 'no_such_job_preset' (step 'a')"),
        "{missing_preset}"
    );

    let missing_workflow = validate(
        &app,
        &token,
        definition(
            "missing-workflow",
            vec![json!({ "id": "w", "step": { "type": "workflow", "name": "No Such Workflow" } })],
        ),
    )
    .await;
    assert_eq!(missing_workflow["valid"], false);
    assert_eq!(
        missing_workflow["errors"],
        json!(["Workflow 'No Such Workflow' not found"])
    );

    let delete_beside_reader = validate(
        &app,
        &token,
        definition(
            "racy-delete",
            vec![
                execute("probe", "exit 0", &[]),
                inline("tidy", "delete", json!({}), &[]),
            ],
        ),
    )
    .await;
    assert_eq!(delete_beside_reader["valid"], false);
    assert_eq!(
        delete_beside_reader["errors"],
        json!([
            "Step 'tidy' removes the pipeline's inputs while 'probe' still read them; make 'tidy' depend on that step so it runs after them"
        ])
    );

    let zero_retry = validate(
        &app,
        &token,
        definition(
            "zero-retry",
            vec![json!({
                "id": "a",
                "step": { "type": "inline", "processor": "execute", "config": { "command": "exit 0" } },
                "retry": { "max_attempts": 0 },
            })],
        ),
    )
    .await;
    assert_eq!(zero_retry["valid"], false);
    assert_eq!(
        zero_retry["errors"],
        json!(["Step 'a' has a retry policy with zero attempts"])
    );

    let zero_timeout = validate(
        &app,
        &token,
        definition(
            "zero-timeout",
            vec![json!({
                "id": "a",
                "step": { "type": "inline", "processor": "execute", "config": { "command": "exit 0" } },
                "timeout_secs": 0,
            })],
        ),
    )
    .await;
    assert_eq!(zero_timeout["valid"], false);
    assert_eq!(
        zero_timeout["errors"],
        json!(["Step 'a' has a zero timeout"])
    );

    // Removal through a processor option only warns: the files go after the
    // step succeeds, so a sibling that finishes first is unaffected.
    let option_removal = validate(
        &app,
        &token,
        definition(
            "option-removal",
            vec![
                execute("probe", "exit 0", &[]),
                inline(
                    "remux",
                    "remux",
                    json!({ "remove_input_on_success": true }),
                    &[],
                ),
            ],
        ),
    )
    .await;
    assert_eq!(option_removal["valid"], true);
    assert_eq!(option_removal["errors"], json!([]));
    let warnings = strings(&option_removal["warnings"]);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].starts_with(
            "Step 'remux' deletes the pipeline's inputs once it succeeds (remove_input_on_success) while 'probe' still read them"
        ),
        "{warnings:?}"
    );

    // Job presets and workflows that exist resolve.
    app.post(
        "/api/job/presets",
        &token,
        &json!({
            "id": "e2e-validate-preset",
            "name": "e2e validate copy",
            "processor": "copy_move",
            "config": "{\"destination\":\"/tmp/e2e\"}",
        }),
    )
    .await
    .expect_status(StatusCode::CREATED);
    app.post(
        "/api/pipeline/presets",
        &token,
        &json!({
            "name": "E2E Validate Flow",
            "dag": definition("E2E Validate Flow", vec![execute("inner", "exit 0", &[])]),
        }),
    )
    .await
    .expect_status(StatusCode::CREATED);
    let resolved = validate(
        &app,
        &token,
        definition(
            "resolved",
            vec![
                json!({ "id": "copy", "step": { "type": "preset", "name": "e2e validate copy" } }),
                json!({
                    "id": "flow",
                    "step": { "type": "workflow", "name": "E2E Validate Flow" },
                    "depends_on": ["copy"],
                }),
            ],
        ),
    )
    .await;
    assert_eq!(resolved["valid"], true, "{resolved}");
    assert_eq!(resolved["errors"], json!([]));

    app.shutdown().await;
}

// ---------------------------------------------------------------------------
// Pipeline presets (workflows)
// ---------------------------------------------------------------------------

fn archive_definition(name: &str) -> Value {
    definition(
        name,
        vec![
            inline(
                "copy",
                "copy_move",
                json!({ "destination": "/archive", "operation": "copy" }),
                &[],
            ),
            inline("tidy", "delete", json!({}), &["copy"]),
        ],
    )
}

#[tokio::test]
async fn pipeline_presets_round_trip_and_keep_created_at_across_updates() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;

    let created = app
        .post(
            "/api/pipeline/presets",
            &token,
            &json!({
                "name": "E2E Nightly Archive",
                "description": "copy then tidy",
                "dag": archive_definition("E2E Nightly Archive"),
            }),
        )
        .await
        .expect_status(StatusCode::CREATED)
        .body;
    let id = created["id"].as_str().expect("preset id").to_string();
    assert_eq!(
        sorted_keys(&created),
        [
            "created_at",
            "dag",
            "description",
            "id",
            "name",
            "updated_at"
        ]
    );
    assert_eq!(created["name"], "E2E Nightly Archive");
    assert_eq!(created["description"], "copy then tidy");
    assert_eq!(created["dag"], archive_definition("E2E Nightly Archive"));
    let created_at = millis(&created["created_at"]);
    let created_updated_at = millis(&created["updated_at"]);

    let fetched = app
        .get(&format!("/api/pipeline/presets/{id}"), &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(fetched["dag"], created["dag"]);
    assert_eq!(millis(&fetched["created_at"]), created_at);

    let renamed_definition = definition(
        "E2E Weekly Archive",
        vec![execute("announce", "exit 0", &[])],
    );
    // The update is stamped in whole milliseconds; within the creation's
    // millisecond it would not compare later.
    eventually(WAIT, "the clock to pass the preset's creation", || async {
        (now_ms() > created_updated_at).then_some(())
    })
    .await;
    let updated = app
        .put(
            &format!("/api/pipeline/presets/{id}"),
            &token,
            &json!({
                "name": "E2E Weekly Archive",
                "description": null,
                "dag": renamed_definition,
            }),
        )
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(updated["id"], id.as_str());
    assert_eq!(updated["name"], "E2E Weekly Archive");
    assert_eq!(updated["description"], Value::Null);
    assert_eq!(millis(&updated["created_at"]), created_at);
    assert!(millis(&updated["updated_at"]) > created_updated_at);

    let reread = app
        .get(&format!("/api/pipeline/presets/{id}"), &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(reread["name"], "E2E Weekly Archive");
    assert_eq!(reread["dag"], renamed_definition);
    assert_eq!(millis(&reread["created_at"]), created_at);

    let missing = app
        .put(
            "/api/pipeline/presets/no-such-preset",
            &token,
            &json!({ "name": "Ghost", "dag": renamed_definition }),
        )
        .await;
    missing.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");

    let listed = app
        .get(
            "/api/pipeline/presets?search=e2e%20weekly&limit=500",
            &token,
        )
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(listed["total"], 1);
    assert_eq!(listed["limit"], 100);
    assert_eq!(listed["offset"], 0);
    assert_eq!(listed["presets"][0]["id"], id.as_str());

    app.delete(&format!("/api/pipeline/presets/{id}"), &token)
        .await
        .expect_status(StatusCode::OK);
    let gone = app
        .get(&format!("/api/pipeline/presets/{id}"), &token)
        .await;
    gone.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");

    app.shutdown().await;
}

async fn save_preset(app: &TestApp, token: &str, name: &str, dag: Value) -> Response {
    app.post(
        "/api/pipeline/presets",
        token,
        &json!({ "name": name, "dag": dag }),
    )
    .await
}

#[tokio::test]
async fn pipeline_presets_refuse_a_workflow_that_cannot_run_and_store_nothing() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;

    let empty = save_preset(&app, &token, "E2E Rejected Empty", definition("x", vec![])).await;
    empty.assert_error(StatusCode::BAD_REQUEST, "BAD_REQUEST");

    let unknown = save_preset(
        &app,
        &token,
        "E2E Rejected Unknown",
        definition("x", vec![inline("odd", "transmogrify", json!({}), &[])]),
    )
    .await;
    unknown.assert_error(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    assert!(
        message(&unknown).contains("unknown processor(s): 'transmogrify' (step 'odd')"),
        "{}",
        unknown.text
    );

    let missing_preset = save_preset(
        &app,
        &token,
        "E2E Rejected Preset",
        definition(
            "x",
            vec![json!({ "id": "a", "step": { "type": "preset", "name": "no_such_job_preset" } })],
        ),
    )
    .await;
    missing_preset.assert_error(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    assert!(
        message(&missing_preset).contains("'no_such_job_preset' (step 'a')"),
        "{}",
        missing_preset.text
    );

    let racy_delete = save_preset(
        &app,
        &token,
        "E2E Rejected Delete",
        definition(
            "x",
            vec![
                execute("probe", "exit 0", &[]),
                inline("tidy", "delete", json!({}), &[]),
            ],
        ),
    )
    .await;
    racy_delete.assert_error(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    assert!(
        message(&racy_delete).starts_with("Step 'tidy' removes the pipeline's inputs"),
        "{}",
        racy_delete.text
    );

    // Every error creation would report is returned, joined by "; ".
    let two_faults = save_preset(
        &app,
        &token,
        "E2E Rejected Twice",
        definition(
            "x",
            vec![
                json!({
                    "id": "slow",
                    "step": { "type": "inline", "processor": "execute", "config": { "command": "exit 0" } },
                    "timeout_secs": 0,
                }),
                inline("odd", "transmogrify", json!({}), &[]),
            ],
        ),
    )
    .await;
    two_faults.assert_error(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    assert!(
        message(&two_faults).starts_with(
            "Step 'slow' has a zero timeout; Pipeline references unknown processor(s): 'transmogrify' (step 'odd')"
        ),
        "{}",
        two_faults.text
    );

    let stored = app
        .get("/api/pipeline/presets?search=E2E%20Rejected", &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(stored["total"], 0, "{stored}");

    // An update is checked the same way and leaves the stored workflow as it was.
    let kept = app
        .post(
            "/api/pipeline/presets",
            &token,
            &json!({ "name": "E2E Kept", "dag": archive_definition("E2E Kept") }),
        )
        .await
        .expect_status(StatusCode::CREATED)
        .body;
    let kept_id = kept["id"].as_str().expect("id");
    let refused = app
        .put(
            &format!("/api/pipeline/presets/{kept_id}"),
            &token,
            &json!({
                "name": "E2E Kept Broken",
                "dag": definition("x", vec![inline("odd", "transmogrify", json!({}), &[])]),
            }),
        )
        .await;
    refused.assert_error(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    let empty_update = app
        .put(
            &format!("/api/pipeline/presets/{kept_id}"),
            &token,
            &json!({ "name": "E2E Kept Empty", "dag": definition("x", vec![]) }),
        )
        .await;
    empty_update.assert_error(StatusCode::BAD_REQUEST, "BAD_REQUEST");
    let unchanged = app
        .get(&format!("/api/pipeline/presets/{kept_id}"), &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(unchanged["name"], "E2E Kept");
    assert_eq!(unchanged["dag"], archive_definition("E2E Kept"));

    app.shutdown().await;
}

#[tokio::test]
async fn pipeline_preset_preview_lists_each_job_and_a_topological_order() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;

    app.post(
        "/api/job/presets",
        &token,
        &json!({
            "id": "e2e-preview-preset",
            "name": "e2e preview copy",
            "processor": "copy_move",
            "config": "{\"destination\":\"/archive\"}",
        }),
    )
    .await
    .expect_status(StatusCode::CREATED);
    app.post(
        "/api/pipeline/presets",
        &token,
        &json!({
            "name": "E2E Preview Inner",
            "dag": definition("E2E Preview Inner", vec![execute("inner", "exit 0", &[])]),
        }),
    )
    .await
    .expect_status(StatusCode::CREATED);

    let preset = app
        .post(
            "/api/pipeline/presets",
            &token,
            &json!({
                "name": "E2E Preview",
                "dag": definition(
                    "E2E Preview",
                    vec![
                        json!({ "id": "finish", "step": { "type": "inline", "processor": "delete", "config": {} }, "depends_on": ["flow", "notify"] }),
                        json!({ "id": "flow", "step": { "type": "workflow", "name": "E2E Preview Inner" }, "depends_on": ["prep"] }),
                        execute("notify", "exit 0", &["prep"]),
                        json!({ "id": "prep", "step": { "type": "preset", "name": "e2e preview copy" } }),
                    ],
                ),
            }),
        )
        .await
        .expect_status(StatusCode::CREATED)
        .body;
    let id = preset["id"].as_str().expect("id");

    let preview = app
        .get(&format!("/api/pipeline/presets/{id}/preview"), &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(preview["preset_id"], id);
    assert_eq!(preview["preset_name"], "E2E Preview");
    assert_eq!(
        preview["jobs"],
        json!([
            { "step_id": "finish", "processor": "delete", "depends_on": ["flow", "notify"], "is_root": false, "is_leaf": true },
            { "step_id": "flow", "processor": "workflow:E2E Preview Inner", "depends_on": ["prep"], "is_root": false, "is_leaf": false },
            { "step_id": "notify", "processor": "execute", "depends_on": ["prep"], "is_root": false, "is_leaf": false },
            { "step_id": "prep", "processor": "e2e preview copy", "depends_on": [], "is_root": true, "is_leaf": false },
        ])
    );

    let order = strings(&preview["execution_order"]);
    assert_eq!(order.len(), 4, "{order:?}");
    let position = |step: &str| {
        order
            .iter()
            .position(|id| id == step)
            .expect("step in order")
    };
    assert_eq!(position("prep"), 0);
    assert!(position("flow") < position("finish"));
    assert!(position("notify") < position("finish"));
    assert_eq!(position("finish"), 3);

    let missing = app
        .get("/api/pipeline/presets/no-such-preset/preview", &token)
        .await;
    missing.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");

    app.shutdown().await;
}

// ---------------------------------------------------------------------------
// Job presets
// ---------------------------------------------------------------------------

fn job_preset(id: &str, name: &str) -> Value {
    json!({
        "id": id,
        "name": name,
        "description": "Copy to the archive",
        "category": "file_ops",
        "processor": "copy_move",
        "config": "{\"destination\":\"/archive\",\"operation\":\"copy\"}",
    })
}

#[tokio::test]
async fn job_presets_resolve_by_exact_name_and_validate_their_writes() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;

    // Name order puts "e2e a ..." ahead of "e2e z ...".
    let target = app
        .post(
            "/api/job/presets",
            &token,
            &job_preset("e2e-target", "e2e z target"),
        )
        .await
        .expect_status(StatusCode::CREATED)
        .body;
    assert_eq!(target["id"], "e2e-target");
    assert_eq!(
        target["config"],
        "{\"destination\":\"/archive\",\"operation\":\"copy\"}"
    );
    millis(&target["created_at"]);
    app.post(
        "/api/job/presets",
        &token,
        &job_preset("e2e-decoy", "e2e a target decoy"),
    )
    .await
    .expect_status(StatusCode::CREATED);

    let searched = app
        .get("/api/job/presets?search=e2e&limit=1", &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(searched["presets"][0]["id"], "e2e-decoy");
    assert_eq!(searched["total"], 2);

    let by_name = app
        .get("/api/job/presets?name=e2e%20z%20target&limit=1", &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(by_name["total"], 1);
    assert_eq!(by_name["limit"], 1);
    assert_eq!(by_name["offset"], 0);
    assert_eq!(by_name["presets"].as_array().map(Vec::len), Some(1));
    assert_eq!(by_name["presets"][0]["id"], "e2e-target");
    assert!(by_name["presets"][0]["config"].is_string());
    assert!(
        strings(&by_name["categories"]).contains(&"file_ops".to_string()),
        "{by_name}"
    );

    let capped = app
        .get("/api/job/presets?limit=500", &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(capped["limit"], 100);

    let invalid = [
        ("bad processor", json!({ "processor": "transmogrify" })),
        ("bad category", json!({ "category": "misc" })),
        ("bad config", json!({ "config": "{not json" })),
        ("blank name", json!({ "name": "   " })),
    ];
    for (index, (label, change)) in invalid.into_iter().enumerate() {
        let mut body = job_preset(
            &format!("e2e-invalid-{index}"),
            &format!("e2e invalid {index}"),
        );
        for (key, value) in change.as_object().expect("object") {
            body[key] = value.clone();
        }
        let rejected = app.post("/api/job/presets", &token, &body).await;
        assert_eq!(
            rejected.status,
            StatusCode::BAD_REQUEST,
            "{label}: {}",
            rejected.text
        );
        assert_eq!(rejected.error_code(), Some("BAD_REQUEST"), "{label}");
        let stored = app
            .get(&format!("/api/job/presets/e2e-invalid-{index}"), &token)
            .await;
        assert_eq!(stored.status, StatusCode::NOT_FOUND, "{label} was stored");
    }

    let taken = app
        .post(
            "/api/job/presets",
            &token,
            &job_preset("e2e-duplicate", "e2e z target"),
        )
        .await;
    taken.assert_error(StatusCode::CONFLICT, "CONFLICT");

    // An update may keep its own name but not take another preset's.
    let mut keep_name = job_preset("ignored", "e2e z target");
    keep_name["config"] = json!("{\"destination\":\"/elsewhere\"}");
    keep_name.as_object_mut().expect("object").remove("id");
    let kept = app
        .put("/api/job/presets/e2e-target", &token, &keep_name)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(kept["config"], "{\"destination\":\"/elsewhere\"}");
    assert_eq!(millis(&kept["created_at"]), millis(&target["created_at"]));
    let mut steal_name = keep_name.clone();
    steal_name["name"] = json!("e2e a target decoy");
    let stolen = app
        .put("/api/job/presets/e2e-target", &token, &steal_name)
        .await;
    stolen.assert_error(StatusCode::CONFLICT, "CONFLICT");
    let missing = app
        .put("/api/job/presets/no-such-preset", &token, &keep_name)
        .await;
    missing.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");

    let reread = app
        .get("/api/job/presets/e2e-target", &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(reread["name"], "e2e z target");
    assert_eq!(reread["config"], "{\"destination\":\"/elsewhere\"}");

    app.delete("/api/job/presets/e2e-decoy", &token)
        .await
        .expect_status(StatusCode::OK);
    let deleted = app.get("/api/job/presets/e2e-decoy", &token).await;
    deleted.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");

    app.shutdown().await;
}

#[tokio::test]
async fn cloning_a_job_preset_copies_it_under_a_new_id_and_refuses_a_taken_name() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;

    app.post(
        "/api/job/presets",
        &token,
        &job_preset("e2e-source", "e2e source"),
    )
    .await
    .expect_status(StatusCode::CREATED);

    let clone = app
        .post(
            "/api/job/presets/e2e-source/clone",
            &token,
            &json!({ "new_name": "e2e source copy" }),
        )
        .await
        .expect_status(StatusCode::CREATED)
        .body;
    let clone_id = clone["id"].as_str().expect("clone id");
    assert_ne!(clone_id, "e2e-source");
    assert_eq!(clone["name"], "e2e source copy");
    assert_eq!(clone["description"], "Copy of: Copy to the archive");
    assert_eq!(clone["category"], "file_ops");
    assert_eq!(clone["processor"], "copy_move");
    assert_eq!(
        clone["config"],
        "{\"destination\":\"/archive\",\"operation\":\"copy\"}"
    );
    let stored = app
        .get(&format!("/api/job/presets/{clone_id}"), &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(stored["name"], "e2e source copy");

    // Creating under a taken name is a 409; cloning to one is a 422.
    let taken = app
        .post(
            "/api/job/presets/e2e-source/clone",
            &token,
            &json!({ "new_name": "e2e source copy" }),
        )
        .await;
    taken.assert_error(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");

    let missing = app
        .post(
            "/api/job/presets/no-such-preset/clone",
            &token,
            &json!({ "new_name": "e2e orphan" }),
        )
        .await;
    missing.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");

    let listed = app
        .get("/api/job/presets?search=e2e%20source", &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(listed["total"], 2, "{listed}");

    app.shutdown().await;
}

// ---------------------------------------------------------------------------
// POST /api/pipeline/create and the offline processors
// ---------------------------------------------------------------------------

#[tokio::test]
async fn creating_a_pipeline_returns_its_first_job_and_an_invalid_definition_creates_nothing() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let input = app.write_output_file("create/clip.flv", "clip");

    let created = create_pipeline(
        &app,
        &token,
        std::slice::from_ref(&input),
        definition(
            "two steps",
            vec![
                execute("first", "exit 0", &[]),
                execute("second", "exit 0", &["first"]),
            ],
        ),
    )
    .await
    .expect_status(StatusCode::CREATED)
    .body;
    let dag_id = created["pipeline_id"].as_str().expect("pipeline_id");
    let first_job = &created["first_job"];
    assert_eq!(first_job["pipeline_id"], dag_id);
    assert_eq!(first_job["belongs_to_workflow"], true);
    assert_eq!(first_job["processor_type"], "execute");
    assert_eq!(first_job["input_path"], json!([input]));
    assert_eq!(first_job["session_id"], "session-e2e");
    assert_eq!(first_job["streamer_id"], "streamer-e2e");
    assert_eq!(first_job["retry_count"], 0);
    // A worker may claim, or even finish, the job before the response is built.
    let status = first_job["status"].as_str().unwrap_or_default();
    assert!(
        ["PENDING", "PROCESSING", "COMPLETED"].contains(&status),
        "{first_job}"
    );
    let settled = settled_dag(&app, &token, dag_id).await;
    assert_eq!(settled["status"], "COMPLETED");
    assert_eq!(
        step_job_id(&settled, "first"),
        first_job["id"].as_str().expect("job id")
    );

    let empty = create_pipeline(
        &app,
        &token,
        std::slice::from_ref(&input),
        definition("x", vec![]),
    )
    .await;
    empty.assert_error(StatusCode::BAD_REQUEST, "BAD_REQUEST");

    let invalid = [
        (
            "cycle",
            definition(
                "x",
                vec![
                    execute("root", "exit 0", &[]),
                    execute("a", "exit 0", &["root", "b"]),
                    execute("b", "exit 0", &["a"]),
                ],
            ),
        ),
        (
            "duplicate id",
            definition(
                "x",
                vec![execute("a", "exit 0", &[]), execute("a", "exit 0", &[])],
            ),
        ),
        (
            "unknown processor",
            definition("x", vec![inline("odd", "transmogrify", json!({}), &[])]),
        ),
        (
            "missing dependency",
            definition("x", vec![execute("a", "exit 0", &["ghost"])]),
        ),
        (
            "zero retry",
            definition(
                "x",
                vec![json!({
                    "id": "a",
                    "step": { "type": "inline", "processor": "execute", "config": { "command": "exit 0" } },
                    "retry": { "max_attempts": 0 },
                })],
            ),
        ),
    ];
    for (label, dag) in invalid {
        let rejected = create_pipeline(&app, &token, std::slice::from_ref(&input), dag).await;
        assert_eq!(
            rejected.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{label}: {}",
            rejected.text
        );
        assert_eq!(rejected.error_code(), Some("VALIDATION_ERROR"), "{label}");
    }

    let dags = app
        .get("/api/pipeline/dags", &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(dags["total"], 1, "{dags}");
    let jobs = app
        .get("/api/pipeline/jobs", &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(jobs["total"], 2, "{jobs}");

    app.shutdown().await;
}

#[tokio::test]
async fn execute_steps_settle_their_workflow_as_completed_or_failed() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let input = app.write_output_file("execute/clip.flv", "clip");

    let (_, completed) = run_pipeline(
        &app,
        &token,
        std::slice::from_ref(&input),
        definition("succeeds", vec![execute("run", "exit 0", &[])]),
    )
    .await;
    assert_eq!(completed["status"], "COMPLETED");
    assert_eq!(completed["error"], Value::Null);
    assert_eq!(step(&completed, "run")["status"], "COMPLETED");
    // Without scan_output_dir an execute step passes its inputs on.
    assert_eq!(step(&completed, "run")["outputs"], json!([input]));
    let succeeded = job(&app, &token, &step_job_id(&completed, "run")).await;
    assert_eq!(succeeded["status"], "COMPLETED");
    assert_eq!(succeeded["error_message"], Value::Null);
    assert!(succeeded["completed_at"].is_string());

    let (_, failed) = run_pipeline(
        &app,
        &token,
        std::slice::from_ref(&input),
        definition("fails", vec![execute("run", "exit 1", &[])]),
    )
    .await;
    assert_eq!(failed["status"], "FAILED");
    assert_eq!(failed["failed_steps"], 1);
    assert!(failed["error"].is_string(), "{failed}");
    assert_eq!(step(&failed, "run")["status"], "FAILED");
    let failure = job(&app, &token, &step_job_id(&failed, "run")).await;
    assert_eq!(failure["status"], "FAILED");
    let error = failure["error_message"].as_str().expect("error message");
    assert!(error.contains("exit code: 1"), "{error}");

    app.shutdown().await;
}

#[tokio::test]
async fn copy_move_refuses_to_overwrite_an_existing_destination() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let source = app.write_output_file("recordings/clip.flv", "new recording");
    let existing = app.write_output_file("archive/clip.flv", "earlier recording");
    let archive = app.output_path("archive");

    let (_, refused) = run_pipeline(
        &app,
        &token,
        std::slice::from_ref(&source),
        definition(
            "copy",
            vec![inline(
                "copy",
                "copy_move",
                json!({ "destination": archive, "operation": "copy" }),
                &[],
            )],
        ),
    )
    .await;
    assert_eq!(refused["status"], "FAILED");
    let failure = job(&app, &token, &step_job_id(&refused, "copy")).await;
    assert_eq!(failure["status"], "FAILED");
    let error = failure["error_message"].as_str().expect("error message");
    assert!(
        error.contains("All 1 input files failed to copy/move"),
        "{error}"
    );
    assert_eq!(
        std::fs::read_to_string(&existing).expect("destination"),
        "earlier recording"
    );
    assert_eq!(
        std::fs::read_to_string(&source).expect("source"),
        "new recording"
    );

    let (_, replaced) = run_pipeline(
        &app,
        &token,
        std::slice::from_ref(&source),
        definition(
            "copy over",
            vec![inline(
                "copy",
                "copy_move",
                json!({ "destination": archive, "operation": "copy", "overwrite": true }),
                &[],
            )],
        ),
    )
    .await;
    assert_eq!(replaced["status"], "COMPLETED");
    assert_eq!(
        std::fs::read_to_string(&existing).expect("destination"),
        "new recording"
    );

    app.shutdown().await;
}

#[tokio::test]
async fn a_delete_step_removes_the_previous_steps_outputs_and_keeps_the_source() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let source = app.write_output_file("recordings/part.flv", "recording");
    let copies = app.output_path("copies");
    let copy = Path::new(&copies)
        .join("part.flv")
        .to_string_lossy()
        .into_owned();

    let (_, settled) = run_pipeline(
        &app,
        &token,
        std::slice::from_ref(&source),
        definition(
            "copy then delete",
            vec![
                inline(
                    "copy",
                    "copy_move",
                    json!({ "destination": copies, "operation": "copy" }),
                    &[],
                ),
                inline("remove", "delete", json!({}), &["copy"]),
            ],
        ),
    )
    .await;
    assert_eq!(settled["status"], "COMPLETED", "{settled}");
    assert_eq!(step(&settled, "copy")["outputs"], json!([copy]));
    assert_eq!(step(&settled, "remove")["outputs"], json!([]));
    assert!(!Path::new(&copy).exists(), "the copy should be deleted");
    assert_eq!(
        std::fs::read_to_string(&source).expect("source"),
        "recording"
    );

    app.shutdown().await;
}

#[tokio::test]
async fn compression_stores_entries_without_roots_drive_prefixes_or_parent_components() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    std::fs::create_dir_all(app.output_dir().join("bundle").join("sub")).expect("sub dir");
    app.write_output_file("bundle/a.txt", "alpha");
    let b = app.write_output_file("bundle/b.txt", "beta");
    let through_parent = app
        .output_dir()
        .join("bundle")
        .join("sub")
        .join("..")
        .join("a.txt")
        .to_string_lossy()
        .into_owned();
    let archive = app.output_path("bundle.zip");

    let (_, settled) = run_pipeline(
        &app,
        &token,
        &[through_parent, b],
        definition(
            "compress",
            vec![inline(
                "zip",
                "compression",
                json!({ "format": "zip", "preserve_paths": true, "output_path": archive }),
                &[],
            )],
        ),
    )
    .await;
    assert_eq!(settled["status"], "COMPLETED", "{settled}");
    assert_eq!(step(&settled, "zip")["outputs"], json!([archive]));

    let file = std::fs::File::open(&archive).expect("open the archive");
    let mut zip = zip::ZipArchive::new(file).expect("read the archive");
    let names: Vec<String> = zip
        .file_names()
        .map(|name| name.expect("entry name").into_owned())
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");
    for name in &names {
        assert!(!name.starts_with('/'), "{name} keeps the root");
        assert!(!name.contains(':'), "{name} keeps a drive prefix");
        assert!(!name.contains('\\'), "{name} uses a backslash");
        assert!(
            !name
                .split('/')
                .any(|part| part == ".." || part == "." || part.is_empty()),
            "{name} keeps a relative or empty component"
        );
    }
    let alpha = names
        .iter()
        .find(|name| name.ends_with("bundle/sub/a.txt"))
        .unwrap_or_else(|| panic!("the parent component is dropped, not resolved: {names:?}"));
    assert!(
        names.iter().any(|name| name.ends_with("bundle/b.txt")),
        "{names:?}"
    );
    let mut contents = String::new();
    std::io::Read::read_to_string(&mut zip.by_name(alpha).expect("entry"), &mut contents)
        .expect("read entry");
    assert_eq!(contents, "alpha");

    app.shutdown().await;
}

// ---------------------------------------------------------------------------
// Jobs
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pending_and_running_jobs_cannot_be_deleted() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    for (id, status) in [
        ("job-pending", "PENDING"),
        ("job-processing", "PROCESSING"),
        ("job-completed", "COMPLETED"),
        ("job-failed", "FAILED"),
        ("job-cancelled", "CANCELLED"),
    ] {
        seed_standalone_job(&app, id, status, 0).await;
    }

    for (id, status) in [("job-pending", "PENDING"), ("job-processing", "PROCESSING")] {
        let refused = app
            .delete(&format!("/api/pipeline/jobs/{id}"), &token)
            .await;
        refused.assert_error(StatusCode::CONFLICT, "CONFLICT");
        assert_eq!(job(&app, &token, id).await["status"], status);
    }

    for id in ["job-completed", "job-failed", "job-cancelled"] {
        let deleted = app
            .delete(&format!("/api/pipeline/jobs/{id}"), &token)
            .await
            .expect_status(StatusCode::OK)
            .body;
        assert_eq!(deleted["success"], true);
        let gone = app.get(&format!("/api/pipeline/jobs/{id}"), &token).await;
        gone.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");
    }

    // Deleting a job that does not exist succeeds as a no-op.
    app.delete("/api/pipeline/jobs/no-such-job", &token)
        .await
        .expect_status(StatusCode::OK);

    app.shutdown().await;
}

/// Run a workflow whose only step fails and return its job id.
async fn failed_execute_job(app: &TestApp, token: &str) -> String {
    let input = app.write_output_file("logs/clip.flv", "clip");
    let (_, failed) = run_pipeline(
        app,
        token,
        &[input],
        definition("fails", vec![execute("run", "exit 1", &[])]),
    )
    .await;
    assert_eq!(failed["status"], "FAILED");
    step_job_id(&failed, "run")
}

#[tokio::test]
async fn job_logs_are_paged_and_an_unknown_job_has_none() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let job_id = failed_execute_job(&app, &token).await;

    let all = app
        .get(&format!("/api/pipeline/jobs/{job_id}/logs"), &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(all["limit"], 20);
    assert_eq!(all["offset"], 0);
    let items = all["items"].as_array().expect("items");
    let total = all["total"].as_u64().expect("total");
    assert_eq!(total as usize, items.len());
    assert!(total >= 2, "{all}");
    for item in items {
        millis(&item["timestamp"]);
        assert!(item["level"].is_string(), "{item}");
        assert!(item["message"].is_string(), "{item}");
    }
    let messages: Vec<&str> = items
        .iter()
        .filter_map(|item| item["message"].as_str())
        .collect();
    assert!(
        messages
            .iter()
            .any(|m| m.contains("Executing command: exit 1")),
        "{messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|m| m.contains("Command failed with status")),
        "{messages:?}"
    );

    let second = app
        .get(
            &format!("/api/pipeline/jobs/{job_id}/logs?limit=1&offset=1"),
            &token,
        )
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(second["total"], total);
    assert_eq!(second["limit"], 1);
    assert_eq!(second["offset"], 1);
    assert_eq!(second["items"], json!([items[1]]));

    let capped = app
        .get(
            &format!("/api/pipeline/jobs/{job_id}/logs?limit=5000"),
            &token,
        )
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(capped["limit"], 1000);

    let unknown = app
        .get("/api/pipeline/jobs/no-such-job/logs", &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(unknown["items"], json!([]));
    assert_eq!(unknown["total"], 0);

    // A finished job has no live progress and, not being an upload, no
    // upload records.
    let progress = app
        .get(&format!("/api/pipeline/jobs/{job_id}/progress"), &token)
        .await;
    progress.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");
    let uploads = app
        .get(&format!("/api/pipeline/jobs/{job_id}/uploads"), &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(uploads, json!({ "items": [] }));

    app.shutdown().await;
}

#[tokio::test]
#[ignore = "bug: job log levels are serialized as \"Info\"/\"Error\", but the job page highlights 'ERROR' and 'WARN'"]
async fn known_bug_job_log_levels_use_the_uppercase_names_the_job_page_highlights() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let job_id = failed_execute_job(&app, &token).await;

    let logs = app
        .get(
            &format!("/api/pipeline/jobs/{job_id}/logs?limit=1000"),
            &token,
        )
        .await
        .expect_status(StatusCode::OK)
        .body;
    let level_of = |needle: &str| {
        logs["items"]
            .as_array()
            .expect("items")
            .iter()
            .find(|item| item["message"].as_str().is_some_and(|m| m.contains(needle)))
            .unwrap_or_else(|| panic!("a log line containing {needle}: {logs}"))["level"]
            .clone()
    };
    assert_eq!(level_of("Executing command"), "INFO");
    assert_eq!(level_of("Command failed with status"), "ERROR");

    app.shutdown().await;
}

#[tokio::test]
async fn retrying_a_standalone_job_resets_it_to_pending() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    seed_standalone_job(&app, "job-failed", "FAILED", 2).await;
    seed_standalone_job(&app, "job-cancelled", "CANCELLED", 0).await;
    seed_standalone_job(&app, "job-completed", "COMPLETED", 0).await;
    seed_standalone_job(&app, "job-pending", "PENDING", 0).await;

    let before = job(&app, &token, "job-failed").await;
    assert_eq!(before["error_message"], "seeded failure");
    assert_eq!(before["belongs_to_workflow"], false);

    let retried = app
        .post("/api/pipeline/jobs/job-failed/retry", &token, &json!({}))
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(retried["id"], "job-failed");
    assert_eq!(retried["status"], "PENDING");
    assert_eq!(retried["retry_count"], 3);
    assert_eq!(retried["error_message"], Value::Null);
    assert_eq!(retried["started_at"], Value::Null);
    assert_eq!(retried["completed_at"], Value::Null);
    let reread = job(&app, &token, "job-failed").await;
    assert_eq!(reread["status"], "PENDING");
    assert_eq!(reread["retry_count"], 3);

    let again = app
        .post("/api/pipeline/jobs/job-failed/retry", &token, &json!({}))
        .await;
    again.assert_error(StatusCode::CONFLICT, "CONFLICT");

    let cancelled = app
        .post("/api/pipeline/jobs/job-cancelled/retry", &token, &json!({}))
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(cancelled["status"], "PENDING");
    assert_eq!(cancelled["retry_count"], 1);
    assert_eq!(cancelled["error_message"], Value::Null);

    for id in ["job-completed", "job-pending"] {
        let refused = app
            .post(
                &format!("/api/pipeline/jobs/{id}/retry"),
                &token,
                &json!({}),
            )
            .await;
        refused.assert_error(StatusCode::CONFLICT, "CONFLICT");
    }
    assert_eq!(
        job(&app, &token, "job-completed").await["status"],
        "COMPLETED"
    );

    let unknown = app
        .post("/api/pipeline/jobs/no-such-job/retry", &token, &json!({}))
        .await;
    unknown.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");

    app.shutdown().await;
}

#[tokio::test]
async fn a_failed_workflows_step_job_must_be_retried_through_the_workflow() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let input = app.write_output_file("retry/clip.flv", "clip");
    let (dag_id, failed) = run_pipeline(
        &app,
        &token,
        &[input],
        definition("fails", vec![execute("run", "exit 1", &[])]),
    )
    .await;
    let job_id = step_job_id(&failed, "run");
    let before = job(&app, &token, &job_id).await;
    assert_eq!(before["belongs_to_workflow"], true);
    assert_eq!(before["pipeline_id"], dag_id.as_str());

    let refused = app
        .post(
            &format!("/api/pipeline/jobs/{job_id}/retry"),
            &token,
            &json!({}),
        )
        .await;
    refused.assert_error(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    assert!(
        message(&refused).contains("retry the workflow"),
        "{}",
        refused.text
    );

    let after = dag(&app, &token, &dag_id).await;
    assert_eq!(after["status"], "FAILED");
    assert_eq!(step(&after, "run")["status"], "FAILED");
    assert_eq!(step_job_id(&after, "run"), job_id);
    let unchanged = job(&app, &token, &job_id).await;
    assert_eq!(unchanged["status"], "FAILED");
    assert_eq!(unchanged["retry_count"], 0);

    app.shutdown().await;
}

#[tokio::test]
async fn cancelling_a_queued_job_sticks_and_cancels_its_workflow_but_a_finished_job_is_refused() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    seed_standalone_job(&app, "job-pending", "PENDING", 0).await;
    seed_standalone_job(&app, "job-completed", "COMPLETED", 0).await;
    seed_standalone_job(&app, "job-failed", "FAILED", 0).await;
    let step_job = seed_queued_dag(&app, "dag-queued").await;

    let cancelled = app
        .post("/api/pipeline/jobs/job-pending/cancel", &token, &json!({}))
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(
        cancelled,
        json!({ "success": true, "message": "Job 'job-pending' cancelled successfully" })
    );
    assert_eq!(
        job(&app, &token, "job-pending").await["status"],
        "CANCELLED"
    );
    // Cancelling again is accepted and changes nothing.
    app.post("/api/pipeline/jobs/job-pending/cancel", &token, &json!({}))
        .await
        .expect_status(StatusCode::OK);
    assert_eq!(
        job(&app, &token, "job-pending").await["status"],
        "CANCELLED"
    );

    // Cancelling a workflow's step job cancels the whole workflow.
    app.post(
        &format!("/api/pipeline/jobs/{step_job}/cancel"),
        &token,
        &json!({}),
    )
    .await
    .expect_status(StatusCode::OK);
    assert_eq!(job(&app, &token, &step_job).await["status"], "CANCELLED");
    let workflow = dag(&app, &token, "dag-queued").await;
    assert_eq!(workflow["status"], "CANCELLED");
    assert_eq!(step(&workflow, "queued")["status"], "CANCELLED");

    // A step job of a cancelled workflow is retried through the workflow too.
    let refused = app
        .post(
            &format!("/api/pipeline/jobs/{step_job}/retry"),
            &token,
            &json!({}),
        )
        .await;
    refused.assert_error(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    assert!(
        message(&refused).contains("retry the workflow"),
        "{}",
        refused.text
    );
    assert_eq!(job(&app, &token, &step_job).await["status"], "CANCELLED");
    assert_eq!(dag(&app, &token, "dag-queued").await["status"], "CANCELLED");

    for (id, status) in [("job-completed", "COMPLETED"), ("job-failed", "FAILED")] {
        let refused = app
            .post(
                &format!("/api/pipeline/jobs/{id}/cancel"),
                &token,
                &json!({}),
            )
            .await;
        refused.assert_error(StatusCode::CONFLICT, "CONFLICT");
        assert_eq!(job(&app, &token, id).await["status"], status);
    }

    let unknown = app
        .post("/api/pipeline/jobs/no-such-job/cancel", &token, &json!({}))
        .await;
    unknown.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");

    app.shutdown().await;
}

#[cfg(unix)]
const SLEEP: &str = "sleep 30";

#[cfg(unix)]
async fn job_reaches(app: &TestApp, token: &str, job_id: &str, status: &str) -> Value {
    eventually(WAIT, &format!("job {job_id} to be {status}"), || async {
        let body = job(app, token, job_id).await;
        (body["status"] == status).then_some(body)
    })
    .await
}

/// Runs only on Unix: it needs a job that keeps running (`sleep`).
#[cfg(unix)]
#[tokio::test]
async fn cancelling_a_running_step_job_cancels_its_workflow_and_outlasts_the_worker() {
    // One CPU worker, so a later job can only run once the cancelled one has
    // let its worker go.
    let app = TestApp::start_seeded("UPDATE global_config SET max_concurrent_cpu_jobs = 1;").await;
    let token = app.admin_token().await;
    let input = app.write_output_file("cancel/clip.flv", "clip");

    let dag_id = start_pipeline(
        &app,
        &token,
        std::slice::from_ref(&input),
        definition("sleeps", vec![execute("wait", SLEEP, &[])]),
    )
    .await;
    let job_id = step_job_id(&dag(&app, &token, &dag_id).await, "wait");
    job_reaches(&app, &token, &job_id, "PROCESSING").await;

    let cancelled = app
        .post(
            &format!("/api/pipeline/jobs/{job_id}/cancel"),
            &token,
            &json!({}),
        )
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(cancelled["success"], true);
    assert_eq!(job(&app, &token, &job_id).await["status"], "CANCELLED");
    let cancelled_dag = dag(&app, &token, &dag_id).await;
    assert_eq!(cancelled_dag["status"], "CANCELLED");
    assert_eq!(step(&cancelled_dag, "wait")["status"], "CANCELLED");

    let (_, completed) = run_pipeline(
        &app,
        &token,
        std::slice::from_ref(&input),
        definition("after", vec![execute("run", "exit 0", &[])]),
    )
    .await;
    assert_eq!(completed["status"], "COMPLETED");
    // The worker has exited; its late outcome did not overwrite the cancel.
    let settled = job(&app, &token, &job_id).await;
    assert_eq!(settled["status"], "CANCELLED", "{settled}");
    assert_eq!(dag(&app, &token, &dag_id).await["status"], "CANCELLED");

    app.shutdown().await;
}

#[tokio::test]
async fn job_lists_filter_by_uppercase_status_and_cap_their_page() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    seed_standalone_job(&app, "job-standalone-completed", "COMPLETED", 0).await;
    seed_standalone_job(&app, "job-standalone-failed", "FAILED", 0).await;
    let input = app.write_output_file("list/clip.flv", "clip");
    let (dag_id, completed) = run_pipeline(
        &app,
        &token,
        &[input],
        definition(
            "listed",
            vec![
                json!({
                    "id": "first",
                    "step": { "type": "inline", "processor": "execute", "config": { "command": "exit 0" } },
                    "retry": { "max_attempts": 3, "backoff_secs": 60 },
                }),
                execute("second", "exit 0", &["first"]),
            ],
        ),
    )
    .await;
    assert_eq!(completed["status"], "COMPLETED");

    let completed_jobs = app
        .get("/api/pipeline/jobs?status=COMPLETED&limit=500", &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(completed_jobs["total"], 3);
    assert_eq!(completed_jobs["limit"], 100);
    let ids: BTreeSet<String> = completed_jobs["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|item| {
            assert_eq!(item["status"], "COMPLETED");
            item["id"].as_str().expect("id").to_string()
        })
        .collect();
    assert_eq!(
        ids,
        BTreeSet::from([
            "job-standalone-completed".to_string(),
            step_job_id(&completed, "first"),
            step_job_id(&completed, "second"),
        ])
    );

    let lowercase = app.get("/api/pipeline/jobs?status=completed", &token).await;
    assert_eq!(
        lowercase.status,
        StatusCode::BAD_REQUEST,
        "{}",
        lowercase.text
    );

    let page = app
        .get("/api/pipeline/jobs/page?status=FAILED&limit=500", &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(sorted_keys(&page), ["items", "limit", "offset"]);
    assert_eq!(page["limit"], 100);
    assert_eq!(page["items"].as_array().map(Vec::len), Some(1));
    assert_eq!(page["items"][0]["id"], "job-standalone-failed");
    assert_eq!(page["items"][0]["belongs_to_workflow"], false);

    let workflow_jobs = app
        .get(&format!("/api/pipeline/jobs?pipeline_id={dag_id}"), &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(workflow_jobs["total"], 2);
    for item in workflow_jobs["items"].as_array().expect("items") {
        assert_eq!(item["pipeline_id"], dag_id.as_str());
        assert_eq!(item["belongs_to_workflow"], true);
        // Only the job detail fills the step's attempt budget.
        assert_eq!(item["max_attempts"], Value::Null);
    }

    let first = job(&app, &token, &step_job_id(&completed, "first")).await;
    assert_eq!(first["max_attempts"], 3);
    let second = job(&app, &token, &step_job_id(&completed, "second")).await;
    assert_eq!(second["max_attempts"], Value::Null);

    let stats = app
        .get("/api/pipeline/stats", &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(stats["pending_count"], 0);
    assert_eq!(stats["processing_count"], 0);
    assert_eq!(stats["completed_count"], 3);
    assert_eq!(stats["failed_count"], 1);
    assert_eq!(stats["cancelled_count"], 0);
    let stalled = app
        .get("/api/pipeline/stalled-operations", &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(stalled, json!({ "items": [] }));

    app.shutdown().await;
}

// ---------------------------------------------------------------------------
// DAG executions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn batch_dag_actions_report_a_result_per_item_and_omit_code_on_success() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let input = app.write_output_file("batch/clip.flv", "clip");
    let (completed, _) = run_pipeline(
        &app,
        &token,
        std::slice::from_ref(&input),
        definition("succeeds", vec![execute("run", "exit 0", &[])]),
    )
    .await;
    let (failed, _) = run_pipeline(
        &app,
        &token,
        std::slice::from_ref(&input),
        definition("fails", vec![execute("run", "exit 1", &[])]),
    )
    .await;
    seed_queued_dag(&app, "dag-queued").await;

    let cancel = app
        .post(
            "/api/pipeline/dags/batch",
            &token,
            &json!({ "ids": [completed, "dag-queued", "no-such-dag"], "action": { "type": "cancel" } }),
        )
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(cancel["requested"], 3);
    assert_eq!(cancel["succeeded"], 1);
    assert_eq!(cancel["failed"], 2);
    let results = cancel["results"].as_array().expect("results");
    assert_eq!(results[0]["id"], completed.as_str());
    assert_eq!(results[0]["success"], false);
    assert_eq!(results[0]["code"], "DAG_ALREADY_TERMINAL");
    assert!(results[0]["error"].is_string());
    // A successful item carries no `code` or `error` key at all.
    assert_eq!(results[1], json!({ "id": "dag-queued", "success": true }));
    assert_eq!(results[2]["id"], "no-such-dag");
    assert_eq!(results[2]["code"], "NOT_FOUND");
    assert_eq!(dag(&app, &token, "dag-queued").await["status"], "CANCELLED");
    assert_eq!(dag(&app, &token, &completed).await["status"], "COMPLETED");

    let retry = app
        .post(
            "/api/pipeline/dags/batch",
            &token,
            &json!({ "ids": [failed, completed], "action": { "type": "retry" } }),
        )
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(retry["succeeded"], 1);
    assert_eq!(
        retry["results"][0],
        json!({ "id": failed, "success": true })
    );
    assert_eq!(retry["results"][1]["success"], false);
    assert_eq!(retry["results"][1]["code"], "VALIDATION_ERROR");
    assert_eq!(settled_dag(&app, &token, &failed).await["status"], "FAILED");

    let delete = app
        .post(
            "/api/pipeline/dags/batch",
            &token,
            &json!({ "ids": [completed, "no-such-dag"], "action": { "type": "delete" } }),
        )
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(delete["requested"], 2);
    assert_eq!(delete["succeeded"], 1);
    assert_eq!(
        delete["results"][0],
        json!({ "id": completed, "success": true })
    );
    assert_eq!(delete["results"][1]["code"], "NOT_FOUND");
    let gone = app
        .get(&format!("/api/pipeline/dag/{completed}"), &token)
        .await;
    gone.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");

    app.shutdown().await;
}

#[tokio::test]
async fn batch_dag_requests_with_bad_ids_are_rejected_before_any_work() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    seed_queued_dag(&app, "dag-queued").await;

    let oversized: Vec<String> = std::iter::once("dag-queued".to_string())
        .chain((0..100).map(|index| format!("dag-{index}")))
        .collect();
    let bad_batches = [
        ("empty", json!([])),
        ("duplicate", json!(["dag-queued", "dag-queued"])),
        ("blank", json!(["dag-queued", "  "])),
        ("oversized", json!(oversized)),
    ];
    for (label, ids) in bad_batches {
        let rejected = app
            .post(
                "/api/pipeline/dags/batch",
                &token,
                &json!({ "ids": ids, "action": { "type": "cancel" } }),
            )
            .await;
        assert_eq!(
            rejected.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{label}: {}",
            rejected.text
        );
        assert_eq!(rejected.error_code(), Some("VALIDATION_ERROR"), "{label}");
    }

    let unknown_action = app
        .post(
            "/api/pipeline/dags/batch",
            &token,
            &json!({ "ids": ["dag-queued"], "action": { "type": "purge" } }),
        )
        .await;
    assert_eq!(
        unknown_action.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        unknown_action.text
    );

    let untouched = dag(&app, &token, "dag-queued").await;
    assert_eq!(untouched["status"], "PENDING");
    assert_eq!(step(&untouched, "queued")["status"], "PROCESSING");
    assert_eq!(step(&untouched, "queued")["job_status"], "PENDING");

    app.shutdown().await;
}

#[tokio::test]
async fn cancelling_a_dag_stops_its_steps_and_a_finished_dag_reports_already_terminal() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let input = app.write_output_file("cancel-dag/clip.flv", "clip");
    let (completed, _) = run_pipeline(
        &app,
        &token,
        std::slice::from_ref(&input),
        definition("succeeds", vec![execute("run", "exit 0", &[])]),
    )
    .await;
    let (failed, _) = run_pipeline(
        &app,
        &token,
        std::slice::from_ref(&input),
        definition("fails", vec![execute("run", "exit 1", &[])]),
    )
    .await;
    let queued_job = seed_queued_dag(&app, "dag-queued").await;

    for finished in [&completed, &failed] {
        let refused = app
            .delete(&format!("/api/pipeline/dag/{finished}"), &token)
            .await;
        refused.assert_error(StatusCode::UNPROCESSABLE_ENTITY, "DAG_ALREADY_TERMINAL");
    }
    assert_eq!(dag(&app, &token, &completed).await["status"], "COMPLETED");
    assert_eq!(dag(&app, &token, &failed).await["status"], "FAILED");

    let cancelled = app
        .delete("/api/pipeline/dag/dag-queued", &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(cancelled["dag_id"], "dag-queued");
    assert_eq!(cancelled["cancelled_steps"], 1);
    assert!(cancelled["message"].is_string());
    let after = dag(&app, &token, "dag-queued").await;
    assert_eq!(after["status"], "CANCELLED");
    assert_eq!(step(&after, "queued")["status"], "CANCELLED");
    assert_eq!(job(&app, &token, &queued_job).await["status"], "CANCELLED");

    let again = app.delete("/api/pipeline/dag/dag-queued", &token).await;
    again.assert_error(StatusCode::UNPROCESSABLE_ENTITY, "DAG_ALREADY_TERMINAL");

    let unknown = app.delete("/api/pipeline/dag/no-such-dag", &token).await;
    unknown.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");

    // Cancelling by pipeline id reports a count instead of failing on an
    // unknown or finished pipeline.
    seed_queued_dag(&app, "dag-queued-by-pipeline").await;
    for (pipeline_id, count) in [
        ("dag-queued-by-pipeline", 1),
        (completed.as_str(), 0),
        ("no-such-pipeline", 0),
    ] {
        let swept = app
            .delete(&format!("/api/pipeline/{pipeline_id}"), &token)
            .await
            .expect_status(StatusCode::OK)
            .body;
        assert_eq!(swept["success"], true, "{pipeline_id}");
        assert_eq!(swept["cancelled_count"], count, "{pipeline_id}");
    }
    assert_eq!(
        dag(&app, &token, "dag-queued-by-pipeline").await["status"],
        "CANCELLED"
    );
    assert_eq!(dag(&app, &token, &completed).await["status"], "COMPLETED");

    app.shutdown().await;
}

#[tokio::test]
async fn deleting_a_dag_removes_its_steps_and_jobs_and_stops_it_first() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let input = app.write_output_file("delete-dag/clip.flv", "clip");
    let (completed, settled) = run_pipeline(
        &app,
        &token,
        &[input],
        definition(
            "two steps",
            vec![
                execute("first", "exit 0", &[]),
                execute("second", "exit 0", &["first"]),
            ],
        ),
    )
    .await;
    let job_ids = [
        step_job_id(&settled, "first"),
        step_job_id(&settled, "second"),
    ];
    let queued_job = seed_queued_dag(&app, "dag-queued").await;

    for (dag_id, jobs) in [
        (completed.as_str(), job_ids.to_vec()),
        ("dag-queued", vec![queued_job]),
    ] {
        let deleted = app
            .delete(&format!("/api/pipeline/dag/{dag_id}/delete"), &token)
            .await
            .expect_status(StatusCode::OK)
            .body;
        assert_eq!(deleted["dag_id"], dag_id);
        let gone = app
            .get(&format!("/api/pipeline/dag/{dag_id}"), &token)
            .await;
        gone.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");
        for job_id in jobs {
            let gone = app
                .get(&format!("/api/pipeline/jobs/{job_id}"), &token)
                .await;
            gone.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");
        }
        let remaining = app
            .get(&format!("/api/pipeline/jobs?pipeline_id={dag_id}"), &token)
            .await
            .expect_status(StatusCode::OK)
            .body;
        assert_eq!(remaining["total"], 0);
        assert_eq!(remaining["items"], json!([]));
    }

    let unknown = app
        .delete("/api/pipeline/dag/no-such-dag/delete", &token)
        .await;
    unknown.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");
    let dags = app
        .get("/api/pipeline/dags", &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(dags["total"], 0);

    app.shutdown().await;
}

/// Stage a recording, then publish it where a file of the same name already
/// exists, which fails the publish step until that file is removed: a failure
/// the user can fix between attempts without any shell command.
fn publish_definition(app: &TestApp, extra: Vec<Value>) -> Value {
    let mut steps = vec![
        inline(
            "stage",
            "copy_move",
            json!({ "destination": app.output_path("staging"), "operation": "copy" }),
            &[],
        ),
        inline(
            "publish",
            "copy_move",
            json!({ "destination": app.output_path("published"), "operation": "copy" }),
            &["stage"],
        ),
        execute("announce", "exit 0", &["publish"]),
    ];
    steps.extend(extra);
    definition("publish", steps)
}

#[tokio::test]
async fn retrying_a_failed_dag_reruns_the_failed_branch_and_keeps_completed_steps() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let source = app.write_output_file("recordings/clip.flv", "recording");
    let conflict = app.write_output_file("published/clip.flv", "someone else's file");
    let staged = Path::new(&app.output_path("staging"))
        .join("clip.flv")
        .to_string_lossy()
        .into_owned();

    let (dag_id, failed) =
        run_pipeline(&app, &token, &[source], publish_definition(&app, vec![])).await;
    assert_eq!(failed["status"], "FAILED");
    assert_eq!(step(&failed, "stage")["status"], "COMPLETED");
    assert_eq!(step(&failed, "stage")["outputs"], json!([staged]));
    assert_eq!(step(&failed, "publish")["status"], "FAILED");
    // A failed step cancels the steps that depend on it.
    assert_eq!(step(&failed, "announce")["status"], "CANCELLED");
    assert_eq!(step(&failed, "announce")["job_id"], Value::Null);
    let stage_job = job(&app, &token, &step_job_id(&failed, "stage")).await;
    let publish_job = step_job_id(&failed, "publish");

    std::fs::remove_file(&conflict).expect("remove the conflicting file");
    let retried = app
        .post(
            &format!("/api/pipeline/dag/{dag_id}/retry"),
            &token,
            &json!({}),
        )
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(
        retried,
        json!({
            "dag_id": dag_id,
            "retried_steps": 2,
            "job_ids": [publish_job],
            "message": "Successfully retried 2 steps",
        })
    );

    let rerun = settled_dag(&app, &token, &dag_id).await;
    assert_eq!(rerun["status"], "COMPLETED", "{rerun}");
    assert_eq!(step(&rerun, "stage")["outputs"], json!([staged]));
    assert_eq!(
        step_job_id(&rerun, "stage"),
        stage_job["id"].as_str().expect("id")
    );
    let stage_after = job(&app, &token, stage_job["id"].as_str().expect("id")).await;
    assert_eq!(stage_after["retry_count"], 0);
    assert_eq!(stage_after["completed_at"], stage_job["completed_at"]);
    assert_eq!(step_job_id(&rerun, "publish"), publish_job);
    assert_eq!(job(&app, &token, &publish_job).await["retry_count"], 1);
    assert_eq!(step(&rerun, "announce")["status"], "COMPLETED");
    assert_eq!(
        std::fs::read_to_string(&conflict).expect("published file"),
        "recording"
    );

    let refused = app
        .post(
            &format!("/api/pipeline/dag/{dag_id}/retry"),
            &token,
            &json!({}),
        )
        .await;
    refused.assert_error(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    let unknown = app
        .post("/api/pipeline/dag/no-such-dag/retry", &token, &json!({}))
        .await;
    unknown.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");

    app.shutdown().await;
}

#[tokio::test]
async fn retry_failed_reruns_every_failed_and_cancelled_dag() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;

    let none = app
        .post("/api/pipeline/dags/retry_failed", &token, &json!({}))
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(
        none,
        json!({ "success": true, "count": 0, "message": "No failed or cancelled DAGs found" })
    );

    let input = app.write_output_file("bulk/clip.flv", "clip");
    let (failed, failed_detail) = run_pipeline(
        &app,
        &token,
        std::slice::from_ref(&input),
        definition("fails", vec![execute("run", "exit 1", &[])]),
    )
    .await;
    let (completed, _) = run_pipeline(
        &app,
        &token,
        std::slice::from_ref(&input),
        definition("succeeds", vec![execute("run", "exit 0", &[])]),
    )
    .await;
    let queued_job = seed_queued_dag(&app, "dag-queued").await;
    app.delete("/api/pipeline/dag/dag-queued", &token)
        .await
        .expect_status(StatusCode::OK);

    let bulk = app
        .post("/api/pipeline/dags/retry_failed", &token, &json!({}))
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(
        bulk,
        json!({
            "success": true,
            "count": 2,
            "message": "Successfully retried 2 failed or cancelled DAGs",
        })
    );

    let queued = dag(&app, &token, "dag-queued").await;
    assert_eq!(queued["status"], "PENDING");
    let requeued = job(&app, &token, &queued_job).await;
    assert_eq!(requeued["status"], "PENDING");
    assert_eq!(requeued["retry_count"], 1);
    assert_eq!(settled_dag(&app, &token, &failed).await["status"], "FAILED");
    let rerun = job(&app, &token, &step_job_id(&failed_detail, "run")).await;
    assert_eq!(rerun["retry_count"], 1);
    assert_eq!(dag(&app, &token, &completed).await["status"], "COMPLETED");

    app.shutdown().await;
}

/// Runs only on Unix: it needs a branch that keeps running (`sleep`) while
/// another one fails.
#[cfg(unix)]
#[tokio::test]
async fn a_failed_step_leaves_independent_branches_running_and_retry_reruns_every_unfinished_step()
{
    // Two CPU workers, so the sleeping branch never holds up `announce`.
    let app = TestApp::start_seeded("UPDATE global_config SET max_concurrent_cpu_jobs = 2;").await;
    let token = app.admin_token().await;
    let source = app.write_output_file("recordings/clip.flv", "recording");
    let conflict = app.write_output_file("published/clip.flv", "someone else's file");

    let dag_id = start_pipeline(
        &app,
        &token,
        &[source],
        publish_definition(&app, vec![execute("slow", SLEEP, &["stage"])]),
    )
    .await;
    let partly_failed = eventually(WAIT, "the publish step to fail", || async {
        let body = dag(&app, &token, &dag_id).await;
        (step(&body, "publish")["status"] == "FAILED").then_some(body)
    })
    .await;
    assert_eq!(step(&partly_failed, "announce")["status"], "CANCELLED");
    let slow_job = step_job_id(&partly_failed, "slow");
    job_reaches(&app, &token, &slow_job, "PROCESSING").await;
    // The workflow is not finished while an independent branch still runs.
    let running = dag(&app, &token, &dag_id).await;
    assert_eq!(running["status"], "PROCESSING");
    assert_eq!(step(&running, "slow")["status"], "PROCESSING");
    assert!(running["error"].is_string(), "{running}");

    let running_retry = app
        .post(
            &format!("/api/pipeline/dag/{dag_id}/retry"),
            &token,
            &json!({}),
        )
        .await;
    running_retry.assert_error(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");

    let cancelled = app
        .delete(&format!("/api/pipeline/dag/{dag_id}"), &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(cancelled["cancelled_steps"], 1);
    let stopped = dag(&app, &token, &dag_id).await;
    assert_eq!(stopped["status"], "CANCELLED");
    assert_eq!(step(&stopped, "slow")["status"], "CANCELLED");
    assert_eq!(job(&app, &token, &slow_job).await["status"], "CANCELLED");
    let stage_job = step_job_id(&stopped, "stage");
    let publish_job = step_job_id(&stopped, "publish");

    std::fs::remove_file(&conflict).expect("remove the conflicting file");
    let retried = app
        .post(
            &format!("/api/pipeline/dag/{dag_id}/retry"),
            &token,
            &json!({}),
        )
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(retried["retried_steps"], 3);
    let restarted: BTreeSet<String> = strings(&retried["job_ids"]).into_iter().collect();
    assert_eq!(restarted, BTreeSet::from([publish_job, slow_job.clone()]));

    let resumed = eventually(WAIT, "the fixed branch to complete", || async {
        let body = dag(&app, &token, &dag_id).await;
        (step(&body, "announce")["status"] == "COMPLETED").then_some(body)
    })
    .await;
    assert_eq!(resumed["status"], "PROCESSING");
    assert_eq!(step(&resumed, "publish")["status"], "COMPLETED");
    assert_eq!(step_job_id(&resumed, "stage"), stage_job);
    assert_eq!(job(&app, &token, &stage_job).await["retry_count"], 0);
    let slow = job_reaches(&app, &token, &slow_job, "PROCESSING").await;
    assert_eq!(slow["retry_count"], 1);

    // Stop the sleeping branch so shutdown does not wait for it.
    app.delete(&format!("/api/pipeline/dag/{dag_id}"), &token)
        .await
        .expect_status(StatusCode::OK);

    app.shutdown().await;
}

#[tokio::test]
async fn dag_list_stats_detail_and_graph_agree_on_status_strings() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let streamer_name = "E2E Pipeline Streamer";
    let streamer_id = id_of(
        &app.create_disabled_streamer(
            &token,
            json!({ "name": streamer_name, "url": "https://live.bilibili.com/4063253" }),
        )
        .await,
    );
    let input = app.write_output_file("list-dags/clip.flv", "clip");

    let diamond = app
        .post(
            "/api/pipeline/create",
            &token,
            &json!({
                "session_id": "session-diamond",
                "streamer_id": streamer_id,
                "input_paths": [input],
                "dag": definition(
                    "Diamond",
                    vec![
                        execute("root", "exit 0", &[]),
                        execute("left", "exit 0", &["root"]),
                        execute("right", "exit 0", &["root"]),
                        execute("join", "exit 0", &["left", "right"]),
                    ],
                ),
            }),
        )
        .await
        .expect_status(StatusCode::CREATED)
        .body["pipeline_id"]
        .as_str()
        .expect("pipeline_id")
        .to_string();
    let completed = settled_dag(&app, &token, &diamond).await;
    let failed = app
        .post(
            "/api/pipeline/create",
            &token,
            &json!({
                "session_id": "session-failed",
                "streamer_id": "streamer-gone",
                "input_paths": [input],
                "dag": definition("Fails", vec![execute("run", "exit 1", &[])]),
            }),
        )
        .await
        .expect_status(StatusCode::CREATED)
        .body["pipeline_id"]
        .as_str()
        .expect("pipeline_id")
        .to_string();
    let failed_detail = settled_dag(&app, &token, &failed).await;
    seed_queued_dag(&app, "dag-queued").await;

    // Detail of a completed diamond.
    assert_eq!(completed["id"], diamond.as_str());
    assert_eq!(completed["name"], "Diamond");
    assert_eq!(completed["status"], "COMPLETED");
    assert_eq!(completed["streamer_id"], streamer_id.as_str());
    assert_eq!(completed["session_id"], "session-diamond");
    assert_eq!(completed["total_steps"], 4);
    assert_eq!(completed["completed_steps"], 4);
    assert_eq!(completed["failed_steps"], 0);
    assert_eq!(completed["progress_percent"], 100.0);
    assert_eq!(completed["error"], Value::Null);
    assert_recent(
        completed["created_at"]
            .as_i64()
            .expect("epoch milliseconds"),
    );
    assert!(completed["updated_at"].as_i64() >= completed["created_at"].as_i64());
    assert!(completed["completed_at"].as_i64() >= completed["created_at"].as_i64());
    for (step_id, depends_on) in [
        ("root", json!([])),
        ("left", json!(["root"])),
        ("right", json!(["root"])),
        ("join", json!(["left", "right"])),
    ] {
        let detail = step(&completed, step_id);
        assert_eq!(detail["status"], "COMPLETED", "{step_id}");
        assert_eq!(detail["job_status"], "COMPLETED", "{step_id}");
        assert_eq!(detail["depends_on"], depends_on, "{step_id}");
        assert_eq!(detail["processor"], "execute", "{step_id}");
        assert_eq!(detail["outputs"], json!([input]), "{step_id}");
        assert!(detail["job_id"].is_string(), "{step_id}");
    }

    assert_eq!(failed_detail["status"], "FAILED");
    assert_eq!(failed_detail["failed_steps"], 1);
    assert!(failed_detail["error"].is_string());
    assert_eq!(step(&failed_detail, "run")["status"], "FAILED");

    let queued = dag(&app, &token, "dag-queued").await;
    assert_eq!(queued["status"], "PENDING");
    assert_eq!(step(&queued, "queued")["job_status"], "PENDING");

    // Step counts and the graph.
    let stats = app
        .get(&format!("/api/pipeline/dag/{diamond}/stats"), &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(
        stats,
        json!({
            "dag_id": diamond,
            "blocked": 0,
            "pending": 0,
            "processing": 0,
            "completed": 4,
            "failed": 0,
            "cancelled": 0,
            "total": 4,
            "progress_percent": 100.0,
        })
    );
    let queued_stats = app
        .get("/api/pipeline/dag/dag-queued/stats", &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(queued_stats["processing"], 1);
    assert_eq!(queued_stats["total"], 1);
    assert_eq!(queued_stats["progress_percent"], 0.0);

    let graph = app
        .get(&format!("/api/pipeline/dag/{diamond}/graph"), &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(graph["dag_id"], diamond.as_str());
    assert_eq!(graph["name"], "Diamond");
    let nodes = graph["nodes"].as_array().expect("nodes");
    assert_eq!(nodes.len(), 4);
    for node in nodes {
        assert_eq!(node["label"], "execute");
        assert_eq!(node["processor"], "execute");
        assert_eq!(node["status"], "COMPLETED");
        let id = node["id"].as_str().expect("node id");
        assert_eq!(node["job_id"], step(&completed, id)["job_id"]);
    }
    let edges: BTreeSet<(String, String)> = graph["edges"]
        .as_array()
        .expect("edges")
        .iter()
        .map(|edge| {
            (
                edge["from"].as_str().expect("from").to_string(),
                edge["to"].as_str().expect("to").to_string(),
            )
        })
        .collect();
    let expected_edges: BTreeSet<(String, String)> = [
        ("root", "left"),
        ("root", "right"),
        ("left", "join"),
        ("right", "join"),
    ]
    .into_iter()
    .map(|(from, to)| (from.to_string(), to.to_string()))
    .collect();
    assert_eq!(edges, expected_edges);

    // The list and its counts.
    let all = app
        .get("/api/pipeline/dags?limit=500", &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(all["total"], 3);
    assert_eq!(all["limit"], 100);
    assert_eq!(all["offset"], 0);
    for item in all["dags"].as_array().expect("dags") {
        let status = item["status"].as_str().expect("status");
        assert!(
            ["PENDING", "PROCESSING", "COMPLETED", "FAILED", "CANCELLED"].contains(&status),
            "{item}"
        );
        assert_recent(item["created_at"].as_i64().expect("epoch milliseconds"));
        assert!(item["updated_at"].as_i64() >= item["created_at"].as_i64());
    }

    let list_ids = |body: &Value| -> Vec<String> {
        body["dags"]
            .as_array()
            .expect("dags")
            .iter()
            .map(|item| item["id"].as_str().expect("id").to_string())
            .collect()
    };
    let completed_list = app
        .get("/api/pipeline/dags?status=completed", &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(completed_list["total"], 1);
    let item = &completed_list["dags"][0];
    assert_eq!(item["id"], diamond.as_str());
    assert_eq!(item["name"], "Diamond");
    assert_eq!(item["status"], "COMPLETED");
    assert_eq!(item["streamer_name"], streamer_name);
    assert_eq!(item["session_id"], "session-diamond");
    assert_eq!(item["total_steps"], 4);
    assert_eq!(item["completed_steps"], 4);
    assert_eq!(item["progress_percent"], 100.0);

    for (query, expected) in [
        ("status=FAILED", vec![failed.clone()]),
        ("status=pending", vec!["dag-queued".to_string()]),
        ("status=processing", vec![]),
        ("session_id=session-failed", vec![failed.clone()]),
        ("search=e2e%20pipeline%20streamer", vec![diamond.clone()]),
    ] {
        let listed = app
            .get(&format!("/api/pipeline/dags?{query}"), &token)
            .await
            .expect_status(StatusCode::OK)
            .body;
        assert_eq!(list_ids(&listed), expected, "{query}");
        assert_eq!(listed["total"], expected.len(), "{query}");
    }
    let failed_list = app
        .get("/api/pipeline/dags?status=FAILED", &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(failed_list["dags"][0]["streamer_name"], Value::Null);
    assert_eq!(failed_list["dags"][0]["failed_steps"], 1);

    let bogus = app.get("/api/pipeline/dags?status=bogus", &token).await;
    bogus.assert_error(StatusCode::BAD_REQUEST, "BAD_REQUEST");

    let counts = app
        .get("/api/pipeline/dags/stats", &token)
        .await
        .expect_status(StatusCode::OK)
        .body;
    assert_eq!(counts["pending_count"], 1);
    assert_eq!(counts["processing_count"], 0);
    assert_eq!(counts["completed_count"], 1);
    assert_eq!(counts["failed_count"], 1);
    assert_eq!(counts["cancelled_count"], 0);
    assert!(
        counts["avg_duration_secs"]
            .as_f64()
            .is_some_and(|secs| secs >= 0.0)
    );

    for path in [
        "/api/pipeline/dag/no-such-dag",
        "/api/pipeline/dag/no-such-dag/stats",
        "/api/pipeline/dag/no-such-dag/graph",
    ] {
        let missing = app.get(path, &token).await;
        missing.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");
    }

    app.shutdown().await;
}
