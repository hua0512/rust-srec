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
    Response, TERMINAL_DAG_STATUSES, TestApp, WAIT, assert_recent, eventually, id_of, inline,
    millis, now_ms, read_file, sorted_keys, step,
};

/// A job type no worker pool claims, so a seeded job keeps its status.
const UNCLAIMED_JOB_TYPE: &str = "e2e_unclaimed";

fn execute(id: &str, command: &str, depends_on: &[&str]) -> Value {
    inline(id, "execute", json!({ "command": command }), depends_on)
}

/// `step` with `key` set, for the step fields beyond processor and order.
fn with(mut step: Value, key: &str, value: Value) -> Value {
    step[key] = value;
    step
}

fn definition(name: &str, steps: Vec<Value>) -> Value {
    json!({ "name": name, "steps": steps })
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .unwrap_or_else(|| panic!("expected an array, got {value}"))
        .iter()
        .map(|item| item.as_str().expect("string item").to_string())
        .collect()
}

/// `name` inside the directory `dir`, joined as the processors join them.
fn joined(dir: &str, name: &str) -> String {
    Path::new(dir).join(name).to_string_lossy().into_owned()
}

/// GET `path` and expect the API's 404.
async fn assert_missing(app: &TestApp, token: &str, path: &str) {
    let missing = app.get(path, token).await;
    missing.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");
}

async fn delete_ok(app: &TestApp, token: &str, path: &str) -> Value {
    app.delete(path, token).await.ok()
}

/// POST without a payload, as the retry and cancel actions take.
async fn post_empty(app: &TestApp, token: &str, path: &str) -> Response {
    app.post(path, token, &json!({})).await
}

async fn post_ok(app: &TestApp, token: &str, path: &str) -> Value {
    post_empty(app, token, path).await.ok()
}

async fn validate(app: &TestApp, token: &str, dag: Value) -> Value {
    app.post("/api/pipeline/validate", token, &json!({ "dag": dag }))
        .await
        .ok()
}

async fn create_pipeline_as(
    app: &TestApp,
    token: &str,
    (session_id, streamer_id): (&str, &str),
    inputs: &[String],
    dag: Value,
) -> Response {
    let body = json!({
        "session_id": session_id,
        "streamer_id": streamer_id,
        "input_paths": inputs,
        "dag": dag,
    });
    app.post("/api/pipeline/create", token, &body).await
}

async fn create_pipeline(app: &TestApp, token: &str, inputs: &[String], dag: Value) -> Response {
    let ids = ("session-e2e", "streamer-e2e");
    create_pipeline_as(app, token, ids, inputs, dag).await
}

/// The DAG id of a workflow the response says was created.
fn created_id(created: Response) -> String {
    let created = created.expect_status(StatusCode::CREATED);
    created.body["pipeline_id"]
        .as_str()
        .expect("pipeline_id")
        .to_string()
}

/// Create a workflow and return its DAG id.
async fn start_pipeline(app: &TestApp, token: &str, inputs: &[String], dag: Value) -> String {
    created_id(create_pipeline(app, token, inputs, dag).await)
}

/// Run a workflow to its end and return its id with its settled detail.
async fn run_pipeline(
    app: &TestApp,
    token: &str,
    inputs: &[String],
    dag: Value,
) -> (String, Value) {
    let dag_id = start_pipeline(app, token, inputs, dag).await;
    let settled = app.settled_dag(token, &dag_id).await;
    (dag_id, settled)
}

fn step_job_id(dag: &Value, step_id: &str) -> String {
    step(dag, step_id)["job_id"]
        .as_str()
        .unwrap_or_else(|| panic!("step {step_id} has a job: {dag}"))
        .to_string()
}

async fn job(app: &TestApp, token: &str, job_id: &str) -> Value {
    app.get_ok(&format!("/api/pipeline/jobs/{job_id}"), token)
        .await
}

async fn job_reaches(app: &TestApp, token: &str, job_id: &str, status: &str) -> Value {
    eventually(WAIT, &format!("job {job_id} to be {status}"), || async {
        let body = job(app, token, job_id).await;
        (body["status"] == status).then_some(body)
    })
    .await
}

async fn save_preset(app: &TestApp, token: &str, name: &str, dag: Value) -> Response {
    let body = json!({ "name": name, "dag": dag });
    app.post("/api/pipeline/presets", token, &body).await
}

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

async fn save_job_preset(app: &TestApp, token: &str, id: &str, name: &str) -> Value {
    app.post("/api/job/presets", token, &job_preset(id, name))
        .await
        .created()
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

/// Seed a workflow whose only step is queued behind busy workers: the DAG and
/// step rows are stored as PROCESSING and the step's job is PENDING. With no
/// job running, the API displays the DAG as PENDING. Returns the job id.
async fn seed_queued_dag(app: &TestApp, dag_id: &str) -> String {
    let now = now_ms();
    let job_id = format!("{dag_id}-job");
    let step_row_id = format!("{dag_id}-step");
    let steps = vec![inline("queued", UNCLAIMED_JOB_TYPE, Value::Null, &[])];
    let definition = definition(&format!("Queued {dag_id}"), steps).to_string();
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

/// A workflow whose step `fails` failed while step `queued` still waits on
/// its job, which a worker pool never claims: the state of a workflow whose
/// other branch is held up behind busy workers.
async fn seed_partly_failed_dag(app: &TestApp, dag_id: &str) {
    let now = now_ms();
    let steps = vec![
        execute("fails", "exit 1", &[]),
        inline("queued", UNCLAIMED_JOB_TYPE, Value::Null, &[]),
    ];
    let definition = definition(&format!("Partly failed {dag_id}"), steps).to_string();
    app.run_sql(&format!(
        "INSERT INTO dag_execution (id, dag_definition, status, streamer_id, session_id, \
             created_at, updated_at, error, total_steps, completed_steps, failed_steps) VALUES \
             ('{dag_id}', '{definition}', 'PROCESSING', 'streamer-seeded', 'session-seeded', \
             {now}, {now}, 'Step ''fails'' failed: exit code 1', 2, 0, 1);
         INSERT INTO job (id, job_type, status, config, state, created_at, updated_at, input, \
             outputs, priority, streamer_id, session_id, retry_count, pipeline_id, started_at, \
             completed_at, error) VALUES \
             ('{dag_id}-failed', 'execute', 'FAILED', '{{}}', '{{}}', {now}, {now}, \
             '[\"/recordings/clip.flv\"]', '[]', 0, 'streamer-seeded', 'session-seeded', 0, \
             '{dag_id}', {now}, {now}, 'exit code 1'), \
             ('{dag_id}-queued', '{UNCLAIMED_JOB_TYPE}', 'PENDING', '{{}}', '{{}}', {now}, {now}, \
             '[\"/recordings/clip.flv\"]', '[]', 0, 'streamer-seeded', 'session-seeded', 0, \
             '{dag_id}', NULL, NULL, NULL);
         INSERT INTO dag_step_execution (id, dag_id, step_id, job_id, status, \
             depends_on_step_ids, created_at, updated_at) VALUES \
             ('{dag_id}-fails', '{dag_id}', 'fails', '{dag_id}-failed', 'FAILED', '[]', {now}, {now}), \
             ('{dag_id}-queued-step', '{dag_id}', 'queued', '{dag_id}-queued', 'PROCESSING', '[]', \
             {now}, {now});
         UPDATE job SET dag_step_execution_id = '{dag_id}-fails' WHERE id = '{dag_id}-failed';
         UPDATE job SET dag_step_execution_id = '{dag_id}-queued-step' WHERE id = '{dag_id}-queued';"
    ))
    .await;
}

// ---------------------------------------------------------------------------
// POST /api/pipeline/validate
// ---------------------------------------------------------------------------

#[tokio::test]
async fn validate_rejects_malformed_graphs_with_the_messages_the_editor_shows() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;

    // A graph of no steps or too many is refused before it is analysed.
    let unanalysed = |error: &str| {
        json!({
            "valid": false,
            "errors": [error],
            "warnings": [],
            "root_steps": [],
            "leaf_steps": [],
            "max_depth": 0,
        })
    };
    let empty = validate(&app, &token, definition("empty", vec![])).await;
    assert_eq!(empty, unanalysed("DAG must have at least one step"));
    let oversized_steps: Vec<Value> = (0..1001)
        .map(|index| execute(&format!("s{index}"), "exit 0", &[]))
        .collect();
    let oversized = validate(&app, &token, definition("oversized", oversized_steps)).await;
    assert_eq!(
        oversized,
        unanalysed("DAG has 1001 steps, maximum allowed is 1000")
    );

    // Each case: steps, errors and, where pinned, the root steps.
    let cases = [
        (
            vec![execute("a", "exit 0", &[]), execute("a", "exit 0", &[])],
            json!(["Duplicate step ID: a"]),
            None,
        ),
        (
            vec![
                execute("root", "exit 0", &[]),
                execute("a", "exit 0", &["a"]),
            ],
            json!(["Step 'a' depends on itself"]),
            None,
        ),
        (
            vec![
                execute("a", "exit 0", &[]),
                execute("b", "exit 0", &["ghost"]),
            ],
            json!(["Step 'b' depends on non-existent step 'ghost'"]),
            None,
        ),
        (
            vec![
                execute("a", "exit 0", &["b"]),
                execute("b", "exit 0", &["a"]),
            ],
            json!([
                "DAG has no root steps (all steps have dependencies)",
                "Cycle detected involving: a -> b",
            ]),
            Some(json!([])),
        ),
        // A cycle below a sound root. Structural errors suppress the creation
        // checks, so the unknown processor on the root is not reported yet.
        (
            vec![
                inline("root", "transmogrify", json!({}), &[]),
                execute("a", "exit 0", &["root", "b"]),
                execute("b", "exit 0", &["a"]),
            ],
            json!(["Cycle detected involving: a -> b"]),
            Some(json!(["root"])),
        ),
    ];
    for (steps, errors, roots) in cases {
        let report = validate(&app, &token, definition("malformed", steps)).await;
        assert_eq!(report["valid"], false, "{report}");
        assert_eq!(report["errors"], errors, "{report}");
        if let Some(roots) = roots {
            assert_eq!(report["root_steps"], roots, "{report}");
        }
    }

    app.shutdown().await;
}

#[tokio::test]
async fn validate_reports_roots_leaves_and_depth_for_chains_fan_out_and_fan_in() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;

    let sound = |roots: &[&str], leaves: &[&str], max_depth: u64, warnings: &[&str]| {
        json!({
            "valid": true,
            "errors": [],
            "warnings": warnings,
            "root_steps": roots,
            "leaf_steps": leaves,
            "max_depth": max_depth,
        })
    };
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
    let cases = [
        (
            "chain",
            vec![
                execute("a", "exit 0", &[]),
                execute("b", "exit 0", &["a"]),
                execute("c", "exit 0", &["b"]),
            ],
            sound(&["a"], &["c"], 3, &[]),
        ),
        (
            "fan-out",
            vec![
                execute("a", "exit 0", &[]),
                execute("b", "exit 0", &["a"]),
                execute("c", "exit 0", &["a"]),
            ],
            sound(&["a"], &["b", "c"], 2, &[]),
        ),
        (
            "fan-in",
            vec![
                execute("a", "exit 0", &[]),
                execute("b", "exit 0", &[]),
                execute("c", "exit 0", &["a", "b"]),
            ],
            sound(&["a", "b"], &["c"], 2, &[]),
        ),
        (
            "single",
            vec![execute("only", "exit 0", &[])],
            sound(
                &["only"],
                &["only"],
                1,
                &["DAG has only one step - consider if a pipeline is necessary"],
            ),
        ),
        (
            "deep",
            deep_steps,
            sound(
                &["s0"],
                &["s10"],
                11,
                &["DAG has depth 11 - deep pipelines may be slow"],
            ),
        ),
    ];
    for (name, steps, expected) in cases {
        let report = validate(&app, &token, definition(name, steps)).await;
        assert_eq!(report, expected, "{name}");
    }

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

    let cases = [
        (
            vec![json!({ "id": "w", "step": { "type": "workflow", "name": "No Such Workflow" } })],
            "Workflow 'No Such Workflow' not found",
        ),
        (
            vec![
                execute("probe", "exit 0", &[]),
                inline("tidy", "delete", json!({}), &[]),
            ],
            "Step 'tidy' removes the pipeline's inputs while 'probe' still read them; make 'tidy' depend on that step so it runs after them",
        ),
        (
            vec![with(
                execute("a", "exit 0", &[]),
                "retry",
                json!({ "max_attempts": 0 }),
            )],
            "Step 'a' has a retry policy with zero attempts",
        ),
        (
            vec![with(execute("a", "exit 0", &[]), "timeout_secs", json!(0))],
            "Step 'a' has a zero timeout",
        ),
    ];
    for (steps, error) in cases {
        let report = validate(&app, &token, definition("refused", steps)).await;
        assert_eq!(report["valid"], false, "{report}");
        assert_eq!(report["errors"], json!([error]), "{report}");
    }

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
    save_job_preset(&app, &token, "e2e-validate-preset", "e2e validate copy").await;
    let inner = definition("E2E Validate Flow", vec![execute("inner", "exit 0", &[])]);
    save_preset(&app, &token, "E2E Validate Flow", inner)
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

    // Checking a definition never starts it.
    let dags = app.get_ok("/api/pipeline/dags", &token).await;
    assert_eq!(dags["total"], 0, "{dags}");

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
        .created();
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

    let path = format!("/api/pipeline/presets/{id}");
    let fetched = app.get_ok(&path, &token).await;
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
    let update = json!({
        "name": "E2E Weekly Archive",
        "description": null,
        "dag": renamed_definition,
    });
    let updated = app.put(&path, &token, &update).await.ok();
    assert_eq!(updated["id"], id.as_str());
    assert_eq!(updated["name"], "E2E Weekly Archive");
    assert_eq!(updated["description"], Value::Null);
    assert_eq!(millis(&updated["created_at"]), created_at);
    assert!(millis(&updated["updated_at"]) > created_updated_at);

    let reread = app.get_ok(&path, &token).await;
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

    let search = "/api/pipeline/presets?search=e2e%20weekly&limit=500";
    let listed = app.get_ok(search, &token).await;
    assert_eq!(listed["total"], 1);
    assert_eq!(listed["limit"], 100);
    assert_eq!(listed["offset"], 0);
    assert_eq!(listed["presets"][0]["id"], id.as_str());

    delete_ok(&app, &token, &path).await;
    assert_missing(&app, &token, &path).await;

    app.shutdown().await;
}

#[tokio::test]
async fn pipeline_presets_refuse_a_workflow_that_cannot_run_and_store_nothing() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;

    let empty = save_preset(&app, &token, "E2E Rejected Empty", definition("x", vec![])).await;
    empty.assert_error(StatusCode::BAD_REQUEST, "BAD_REQUEST");

    // The checks are the ones `/validate` runs; every error creation would
    // report is returned, joined by "; ".
    let two_faults = save_preset(
        &app,
        &token,
        "E2E Rejected Twice",
        definition(
            "x",
            vec![
                with(execute("slow", "exit 0", &[]), "timeout_secs", json!(0)),
                inline("odd", "transmogrify", json!({}), &[]),
            ],
        ),
    )
    .await;
    two_faults.assert_error(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    assert!(
        two_faults.body["message"].as_str().is_some_and(|message| message.starts_with(
            "Step 'slow' has a zero timeout; Pipeline references unknown processor(s): 'transmogrify' (step 'odd')"
        )),
        "{}",
        two_faults.text
    );
    let cycle = save_preset(
        &app,
        &token,
        "E2E Rejected Cycle",
        definition(
            "x",
            vec![
                execute("root", "exit 0", &[]),
                execute("a", "exit 0", &["root", "b"]),
                execute("b", "exit 0", &["a"]),
            ],
        ),
    )
    .await;
    cycle.assert_error(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    assert_eq!(
        cycle.body["message"], "DAG pipeline contains a cycle",
        "{}",
        cycle.text
    );

    let stored = app
        .get_ok("/api/pipeline/presets?search=E2E%20Rejected", &token)
        .await;
    assert_eq!(stored["total"], 0, "{stored}");

    // An update is checked the same way and leaves the stored workflow as it was.
    let kept = save_preset(&app, &token, "E2E Kept", archive_definition("E2E Kept"))
        .await
        .created();
    let path = format!("/api/pipeline/presets/{}", id_of(&kept));
    let unknown = definition("x", vec![inline("odd", "transmogrify", json!({}), &[])]);
    let refused = app
        .put(
            &path,
            &token,
            &json!({ "name": "E2E Kept Broken", "dag": unknown }),
        )
        .await;
    refused.assert_error(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    let empty = definition("x", vec![]);
    let empty_update = app
        .put(
            &path,
            &token,
            &json!({ "name": "E2E Kept Empty", "dag": empty }),
        )
        .await;
    empty_update.assert_error(StatusCode::BAD_REQUEST, "BAD_REQUEST");
    let unchanged = app.get_ok(&path, &token).await;
    assert_eq!(unchanged["name"], "E2E Kept");
    assert_eq!(unchanged["dag"], archive_definition("E2E Kept"));

    app.shutdown().await;
}

#[tokio::test]
async fn pipeline_preset_preview_lists_each_job_and_a_topological_order() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;

    save_job_preset(&app, &token, "e2e-preview-preset", "e2e preview copy").await;
    let inner = definition("E2E Preview Inner", vec![execute("inner", "exit 0", &[])]);
    save_preset(&app, &token, "E2E Preview Inner", inner)
        .await
        .expect_status(StatusCode::CREATED);

    let steps = vec![
        inline("finish", "delete", json!({}), &["flow", "notify"]),
        json!({ "id": "flow", "step": { "type": "workflow", "name": "E2E Preview Inner" }, "depends_on": ["prep"] }),
        execute("notify", "exit 0", &["prep"]),
        json!({ "id": "prep", "step": { "type": "preset", "name": "e2e preview copy" } }),
    ];
    let preset = save_preset(
        &app,
        &token,
        "E2E Preview",
        definition("E2E Preview", steps),
    )
    .await
    .created();
    let id = preset["id"].as_str().expect("id");

    let preview = app
        .get_ok(&format!("/api/pipeline/presets/{id}/preview"), &token)
        .await;
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

    assert_missing(&app, &token, "/api/pipeline/presets/no-such-preset/preview").await;

    app.shutdown().await;
}

// ---------------------------------------------------------------------------
// Job presets
// ---------------------------------------------------------------------------

#[tokio::test]
async fn job_presets_resolve_by_exact_name_and_validate_their_writes() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;

    // Name order puts "e2e a ..." ahead of "e2e z ...".
    let target = save_job_preset(&app, &token, "e2e-target", "e2e z target").await;
    assert_eq!(target["id"], "e2e-target");
    assert_eq!(
        target["config"],
        "{\"destination\":\"/archive\",\"operation\":\"copy\"}"
    );
    millis(&target["created_at"]);
    save_job_preset(&app, &token, "e2e-decoy", "e2e a target decoy").await;

    let searched = app
        .get_ok("/api/job/presets?search=e2e&limit=1", &token)
        .await;
    assert_eq!(searched["presets"][0]["id"], "e2e-decoy");
    assert_eq!(searched["total"], 2);

    let by_name = "/api/job/presets?name=e2e%20z%20target&limit=500";
    let by_name = app.get_ok(by_name, &token).await;
    assert_eq!(by_name["total"], 1);
    assert_eq!(by_name["limit"], 100);
    assert_eq!(by_name["offset"], 0);
    assert_eq!(by_name["presets"].as_array().map(Vec::len), Some(1));
    assert_eq!(by_name["presets"][0]["id"], "e2e-target");
    assert!(by_name["presets"][0]["config"].is_string());
    assert!(
        strings(&by_name["categories"]).contains(&"file_ops".to_string()),
        "{by_name}"
    );

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
        .ok();
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

    let reread = app.get_ok("/api/job/presets/e2e-target", &token).await;
    assert_eq!(reread["name"], "e2e z target");
    assert_eq!(reread["config"], "{\"destination\":\"/elsewhere\"}");

    delete_ok(&app, &token, "/api/job/presets/e2e-decoy").await;
    assert_missing(&app, &token, "/api/job/presets/e2e-decoy").await;

    app.shutdown().await;
}

#[tokio::test]
async fn cloning_a_job_preset_copies_it_under_a_new_id_and_refuses_a_taken_name() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;

    save_job_preset(&app, &token, "e2e-source", "e2e source").await;

    let clone_path = "/api/job/presets/e2e-source/clone";
    let copy_name = json!({ "new_name": "e2e source copy" });
    let clone = app.post(clone_path, &token, &copy_name).await.created();
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
        .get_ok(&format!("/api/job/presets/{clone_id}"), &token)
        .await;
    assert_eq!(stored["name"], "e2e source copy");

    // Creating under a taken name is a 409; cloning to one is a 422.
    let taken = app.post(clone_path, &token, &copy_name).await;
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
        .get_ok("/api/job/presets?search=e2e%20source", &token)
        .await;
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
    .created();
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
    let settled = app.settled_dag(&token, dag_id).await;
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

    // A structural fault below a sound root, and an unknown processor: the
    // two stages creation checks a definition in.
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
            "unknown processor",
            definition("x", vec![inline("odd", "transmogrify", json!({}), &[])]),
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

    let dags = app.get_ok("/api/pipeline/dags", &token).await;
    assert_eq!(dags["total"], 1, "{dags}");
    let jobs = app.get_ok("/api/pipeline/jobs", &token).await;
    assert_eq!(jobs["total"], 2, "{jobs}");

    app.shutdown().await;
}

/// An execute step settles its workflow as completed or failed; the job of a
/// failed workflow's step can be retried only through that workflow.
#[tokio::test]
async fn execute_steps_settle_their_workflow_and_a_failed_step_job_retries_only_through_it() {
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

    let (dag_id, failed) = run_pipeline(
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
    let job_id = step_job_id(&failed, "run");
    let failure = job(&app, &token, &job_id).await;
    assert_eq!(failure["status"], "FAILED");
    assert!(failure["error_message"].is_string(), "{failure}");
    assert_eq!(failure["belongs_to_workflow"], true);
    assert_eq!(failure["pipeline_id"], dag_id.as_str());

    let retry = format!("/api/pipeline/jobs/{job_id}/retry");
    let refused = post_empty(&app, &token, &retry).await;
    refused.assert_error(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    let after = app.dag(&token, &dag_id).await;
    assert_eq!(after["status"], "FAILED");
    assert_eq!(step(&after, "run")["status"], "FAILED");
    assert_eq!(step_job_id(&after, "run"), job_id);
    let unchanged = job(&app, &token, &job_id).await;
    assert_eq!(unchanged["status"], "FAILED");
    assert_eq!(unchanged["retry_count"], 0);

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
    assert!(failure["error_message"].is_string(), "{failure}");
    assert_eq!(read_file(&existing), "earlier recording");
    assert_eq!(read_file(&source), "new recording");

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
    assert_eq!(read_file(&existing), "new recording");

    app.shutdown().await;
}

#[tokio::test]
async fn a_delete_step_removes_the_previous_steps_outputs_and_keeps_the_source() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let source = app.write_output_file("recordings/part.flv", "recording");
    let copies = app.output_path("copies");
    let copy = joined(&copies, "part.flv");

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
    assert_eq!(read_file(&source), "recording");

    app.shutdown().await;
}

/// A step given several inputs writes one output per input, so a single
/// configured `output_path` would make every input overwrite the same file.
/// The step fails before it runs anything and the inputs stay as they were.
#[tokio::test]
async fn batch_steps_refuse_a_single_output_path_for_several_inputs() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let inputs = [
        app.write_output_file("batch/first.flv", "first"),
        app.write_output_file("batch/second.flv", "second"),
    ];

    for (processor, output) in [
        ("audio_extract", "batch/out.mp3"),
        ("metadata", "batch/out.flv"),
    ] {
        let output = app.output_path(output);
        let (_, settled) = run_pipeline(
            &app,
            &token,
            &inputs,
            definition(
                processor,
                vec![inline(
                    "batch",
                    processor,
                    json!({ "output_path": output }),
                    &[],
                )],
            ),
        )
        .await;
        assert_eq!(settled["status"], "FAILED", "{processor}: {settled}");
        let failed = job(&app, &token, &step_job_id(&settled, "batch")).await;
        let message = failed["error_message"].as_str().unwrap_or_default();
        assert!(
            message.contains(&format!(
                "{processor}: config.output_path is not supported for batch inputs"
            )),
            "{processor}: {failed}"
        );
        assert!(!Path::new(&output).exists(), "{processor} wrote {output}");
    }
    assert_eq!(read_file(&inputs[0]), "first");
    assert_eq!(read_file(&inputs[1]), "second");

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

/// Only a running job with a progress report carries `progress`; every
/// other job, whatever its status, reports it as null in its detail and in
/// both job lists, rather than omitting it or showing a stale value. Only a
/// job that has finished can be deleted.
#[tokio::test]
async fn jobs_without_live_progress_report_null_progress_and_only_finished_ones_can_be_deleted() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let statuses = ["PENDING", "PROCESSING", "COMPLETED", "FAILED", "CANCELLED"];
    let id = |status: &str| format!("job-{}", status.to_lowercase());
    for status in statuses {
        seed_standalone_job(&app, &id(status), status, 0).await;
        let detail = job(&app, &token, &id(status)).await;
        assert_eq!(detail["status"], status);
        assert_eq!(detail.get("progress"), Some(&Value::Null), "{detail}");
    }
    for path in ["/api/pipeline/jobs", "/api/pipeline/jobs/page"] {
        let page = app.get_ok(path, &token).await;
        let items = page["items"].as_array().expect("items");
        assert_eq!(items.len(), statuses.len(), "{path}: {page}");
        for item in items {
            assert_eq!(item.get("progress"), Some(&Value::Null), "{path}: {item}");
        }
    }

    for status in statuses {
        let path = format!("/api/pipeline/jobs/{}", id(status));
        let deleted = app.delete(&path, &token).await;
        if matches!(status, "PENDING" | "PROCESSING") {
            deleted.assert_error(StatusCode::CONFLICT, "CONFLICT");
            assert_eq!(app.get_ok(&path, &token).await["status"], status);
        } else {
            let deleted = deleted.ok();
            assert_eq!(deleted["success"], true, "{status}");
            assert_missing(&app, &token, &path).await;
        }
    }

    // Deleting a job that does not exist succeeds as a no-op.
    delete_ok(&app, &token, "/api/pipeline/jobs/no-such-job").await;

    app.shutdown().await;
}

#[tokio::test]
async fn job_logs_are_paged_leveled_and_capped_and_an_unknown_job_has_none() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let input = app.write_output_file("logs/clip.flv", "clip");
    let (_, failed) = run_pipeline(
        &app,
        &token,
        &[input],
        definition("fails", vec![execute("run", "exit 1", &[])]),
    )
    .await;
    assert_eq!(failed["status"], "FAILED");
    let job_id = step_job_id(&failed, "run");
    let logs = format!("/api/pipeline/jobs/{job_id}/logs");

    let all = app.get_ok(&logs, &token).await;
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

    let second = app
        .get_ok(&format!("{logs}?limit=1&offset=1"), &token)
        .await;
    assert_eq!(second["total"], total);
    assert_eq!(second["limit"], 1);
    assert_eq!(second["offset"], 1);
    // Lines logged in the same millisecond have no defined order, so only
    // the page size and membership are fixed.
    let page = second["items"].as_array().expect("items");
    assert_eq!(page.len(), 1, "{second}");
    assert!(items.contains(&page[0]), "{second}");

    let capped = app.get_ok(&format!("{logs}?limit=5000"), &token).await;
    assert_eq!(capped["limit"], 1000);
    // Levels use the uppercase names the job page highlights.
    let level_of = |needle: &str| {
        capped["items"]
            .as_array()
            .expect("items")
            .iter()
            .find(|item| item["message"].as_str().is_some_and(|m| m.contains(needle)))
            .unwrap_or_else(|| panic!("a log line containing {needle}: {capped}"))["level"]
            .clone()
    };
    assert_eq!(level_of("Executing command"), "INFO");
    assert_eq!(level_of("Command failed with status"), "ERROR");

    let unknown = app
        .get_ok("/api/pipeline/jobs/no-such-job/logs", &token)
        .await;
    assert_eq!(unknown["items"], json!([]));
    assert_eq!(unknown["total"], 0);

    // A finished job has no live progress and, not being an upload, no
    // upload records.
    assert_missing(
        &app,
        &token,
        &format!("/api/pipeline/jobs/{job_id}/progress"),
    )
    .await;
    let uploads = app
        .get_ok(&format!("/api/pipeline/jobs/{job_id}/uploads"), &token)
        .await;
    assert_eq!(uploads, json!({ "items": [] }));

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

    let retry = |id: &str| format!("/api/pipeline/jobs/{id}/retry");
    let retried = post_ok(&app, &token, &retry("job-failed")).await;
    assert_eq!(retried["id"], "job-failed");
    assert_eq!(retried["status"], "PENDING");
    assert_eq!(retried["retry_count"], 3);
    assert_eq!(retried["error_message"], Value::Null);
    assert_eq!(retried["started_at"], Value::Null);
    assert_eq!(retried["completed_at"], Value::Null);
    let reread = job(&app, &token, "job-failed").await;
    assert_eq!(reread["status"], "PENDING");
    assert_eq!(reread["retry_count"], 3);

    let cancelled = post_ok(&app, &token, &retry("job-cancelled")).await;
    assert_eq!(cancelled["status"], "PENDING");
    assert_eq!(cancelled["retry_count"], 1);
    assert_eq!(cancelled["error_message"], Value::Null);

    // The retried job is pending again, so a second retry is refused too.
    for id in ["job-failed", "job-completed", "job-pending"] {
        let refused = post_empty(&app, &token, &retry(id)).await;
        refused.assert_error(StatusCode::CONFLICT, "CONFLICT");
    }
    assert_eq!(
        job(&app, &token, "job-completed").await["status"],
        "COMPLETED"
    );

    let unknown = post_empty(&app, &token, &retry("no-such-job")).await;
    unknown.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");

    app.shutdown().await;
}

/// Cancelling a queued job sticks and cancels its workflow. A finished job is
/// refused, including the failed step job of a workflow that still runs, and
/// that refusal leaves the workflow running.
#[tokio::test]
async fn cancelling_a_queued_job_sticks_and_cancels_its_workflow_but_a_finished_job_is_refused() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    seed_standalone_job(&app, "job-pending", "PENDING", 0).await;
    seed_standalone_job(&app, "job-completed", "COMPLETED", 0).await;
    seed_standalone_job(&app, "job-failed", "FAILED", 0).await;
    let step_job = seed_queued_dag(&app, "dag-queued").await;
    seed_partly_failed_dag(&app, "dag-partly-failed").await;
    // With no job running, an unfinished workflow is displayed as PENDING.
    let before = app.dag(&token, "dag-partly-failed").await;
    assert_eq!(before["status"], "PENDING", "{before}");

    let cancel = |id: &str| format!("/api/pipeline/jobs/{id}/cancel");
    let cancelled = post_ok(&app, &token, &cancel("job-pending")).await;
    assert_eq!(cancelled["success"], true);
    assert!(cancelled["message"].is_string(), "{cancelled}");
    assert_eq!(
        job(&app, &token, "job-pending").await["status"],
        "CANCELLED"
    );
    // Cancelling again is accepted and changes nothing.
    post_ok(&app, &token, &cancel("job-pending")).await;
    assert_eq!(
        job(&app, &token, "job-pending").await["status"],
        "CANCELLED"
    );

    // Cancelling a workflow's step job cancels the whole workflow.
    post_ok(&app, &token, &cancel(&step_job)).await;
    assert_eq!(job(&app, &token, &step_job).await["status"], "CANCELLED");
    let workflow = app.dag(&token, "dag-queued").await;
    assert_eq!(workflow["status"], "CANCELLED");
    assert_eq!(step(&workflow, "queued")["status"], "CANCELLED");

    // A step job of a cancelled workflow is retried through the workflow too.
    let retry = format!("/api/pipeline/jobs/{step_job}/retry");
    let refused = post_empty(&app, &token, &retry).await;
    refused.assert_error(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    assert_eq!(job(&app, &token, &step_job).await["status"], "CANCELLED");
    assert_eq!(app.dag(&token, "dag-queued").await["status"], "CANCELLED");

    for (id, status) in [
        ("job-completed", "COMPLETED"),
        ("job-failed", "FAILED"),
        ("dag-partly-failed-failed", "FAILED"),
    ] {
        let refused = post_empty(&app, &token, &cancel(id)).await;
        refused.assert_error(StatusCode::CONFLICT, "CONFLICT");
        assert_eq!(job(&app, &token, id).await["status"], status);
    }
    let running = app.dag(&token, "dag-partly-failed").await;
    assert_eq!(running["status"], "PENDING", "{running}");
    assert_eq!(step(&running, "fails")["status"], "FAILED");
    assert_eq!(step(&running, "queued")["status"], "PROCESSING");
    assert_eq!(
        job(&app, &token, "dag-partly-failed-queued").await["status"],
        "PENDING"
    );

    let unknown = post_empty(&app, &token, &cancel("no-such-job")).await;
    unknown.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");

    app.shutdown().await;
}

#[cfg(unix)]
const SLEEP: &str = "sleep 30";

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
    let job_id = step_job_id(&app.dag(&token, &dag_id).await, "wait");
    job_reaches(&app, &token, &job_id, "PROCESSING").await;

    let cancelled = post_ok(&app, &token, &format!("/api/pipeline/jobs/{job_id}/cancel")).await;
    assert_eq!(cancelled["success"], true);
    assert_eq!(job(&app, &token, &job_id).await["status"], "CANCELLED");
    let cancelled_dag = app.dag(&token, &dag_id).await;
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
    assert_eq!(app.dag(&token, &dag_id).await["status"], "CANCELLED");

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
                with(
                    execute("first", "exit 0", &[]),
                    "retry",
                    json!({ "max_attempts": 3, "backoff_secs": 60 }),
                ),
                execute("second", "exit 0", &["first"]),
            ],
        ),
    )
    .await;
    assert_eq!(completed["status"], "COMPLETED");

    let completed_jobs = "/api/pipeline/jobs?status=COMPLETED&limit=500";
    let completed_jobs = app.get_ok(completed_jobs, &token).await;
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
        .get_ok("/api/pipeline/jobs/page?status=FAILED&limit=500", &token)
        .await;
    assert_eq!(sorted_keys(&page), ["items", "limit", "offset"]);
    assert_eq!(page["limit"], 100);
    assert_eq!(page["items"].as_array().map(Vec::len), Some(1));
    assert_eq!(page["items"][0]["id"], "job-standalone-failed");
    assert_eq!(page["items"][0]["belongs_to_workflow"], false);

    let workflow_jobs = format!("/api/pipeline/jobs?pipeline_id={dag_id}");
    let workflow_jobs = app.get_ok(&workflow_jobs, &token).await;
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

    let stats = app.get_ok("/api/pipeline/stats", &token).await;
    assert_eq!(stats["pending_count"], 0);
    assert_eq!(stats["processing_count"], 0);
    assert_eq!(stats["completed_count"], 3);
    assert_eq!(stats["failed_count"], 1);
    assert_eq!(stats["cancelled_count"], 0);
    let stalled = app.get_ok("/api/pipeline/stalled-operations", &token).await;
    assert_eq!(stalled, json!({ "items": [] }));

    app.shutdown().await;
}

// ---------------------------------------------------------------------------
// DAG executions
// ---------------------------------------------------------------------------

/// One workflow in each state the DAG actions distinguish.
struct DagFixture {
    completed: String,
    failed: String,
    /// The job of the failed workflow's only step.
    failed_job: String,
    /// The job of `dag-queued`, seeded by [`seed_queued_dag`].
    queued_job: String,
}

/// Run a one-step workflow that completes and one that fails, and seed
/// `dag-queued`.
async fn completed_failed_and_queued_dags(app: &TestApp, token: &str) -> DagFixture {
    let input = app.write_output_file("dags/clip.flv", "clip");
    let (completed, _) = run_pipeline(
        app,
        token,
        std::slice::from_ref(&input),
        definition("succeeds", vec![execute("run", "exit 0", &[])]),
    )
    .await;
    let (failed, failed_detail) = run_pipeline(
        app,
        token,
        &[input],
        definition("fails", vec![execute("run", "exit 1", &[])]),
    )
    .await;
    DagFixture {
        completed,
        failed,
        failed_job: step_job_id(&failed_detail, "run"),
        queued_job: seed_queued_dag(app, "dag-queued").await,
    }
}

async fn batch(app: &TestApp, token: &str, ids: Value, action: &str) -> Response {
    let body = json!({ "ids": ids, "action": { "type": action } });
    app.post("/api/pipeline/dags/batch", token, &body).await
}

#[tokio::test]
async fn batch_dag_actions_report_a_result_per_item_and_omit_code_on_success() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let DagFixture {
        completed, failed, ..
    } = completed_failed_and_queued_dags(&app, &token).await;

    let ids = json!([completed, "dag-queued", "no-such-dag"]);
    let cancel = batch(&app, &token, ids, "cancel").await.ok();
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
    assert_eq!(app.dag(&token, "dag-queued").await["status"], "CANCELLED");
    assert_eq!(app.dag(&token, &completed).await["status"], "COMPLETED");

    let retry = batch(&app, &token, json!([failed, completed]), "retry")
        .await
        .ok();
    assert_eq!(retry["succeeded"], 1);
    assert_eq!(
        retry["results"][0],
        json!({ "id": failed, "success": true })
    );
    assert_eq!(retry["results"][1]["success"], false);
    assert_eq!(retry["results"][1]["code"], "VALIDATION_ERROR");
    assert_eq!(app.settled_dag(&token, &failed).await["status"], "FAILED");

    let delete = batch(&app, &token, json!([completed, "no-such-dag"]), "delete")
        .await
        .ok();
    assert_eq!(delete["requested"], 2);
    assert_eq!(delete["succeeded"], 1);
    assert_eq!(
        delete["results"][0],
        json!({ "id": completed, "success": true })
    );
    assert_eq!(delete["results"][1]["code"], "NOT_FOUND");
    assert_missing(&app, &token, &format!("/api/pipeline/dag/{completed}")).await;

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
        let rejected = batch(&app, &token, ids, "cancel").await;
        assert_eq!(
            rejected.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{label}: {}",
            rejected.text
        );
        assert_eq!(rejected.error_code(), Some("VALIDATION_ERROR"), "{label}");
    }

    let unknown_action = batch(&app, &token, json!(["dag-queued"]), "purge").await;
    assert_eq!(
        unknown_action.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        unknown_action.text
    );

    let untouched = app.dag(&token, "dag-queued").await;
    assert_eq!(untouched["status"], "PENDING");
    assert_eq!(step(&untouched, "queued")["status"], "PROCESSING");
    assert_eq!(step(&untouched, "queued")["job_status"], "PENDING");

    app.shutdown().await;
}

/// Cancelling stops an unfinished workflow and refuses a finished one;
/// `retry_failed` then reruns every failed and cancelled workflow.
#[tokio::test]
async fn cancelling_a_dag_stops_its_steps_and_retry_failed_reruns_every_failed_and_cancelled_dag() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let retry_failed = async || post_ok(&app, &token, "/api/pipeline/dags/retry_failed").await;
    let none = retry_failed().await;
    assert_eq!(none["success"], true);
    assert_eq!(none["count"], 0);
    assert!(none["message"].is_string(), "{none}");

    let DagFixture {
        completed,
        failed,
        failed_job,
        queued_job,
    } = completed_failed_and_queued_dags(&app, &token).await;

    for finished in [&completed, &failed] {
        let refused = app
            .delete(&format!("/api/pipeline/dag/{finished}"), &token)
            .await;
        refused.assert_error(StatusCode::UNPROCESSABLE_ENTITY, "DAG_ALREADY_TERMINAL");
    }
    assert_eq!(app.dag(&token, &completed).await["status"], "COMPLETED");
    assert_eq!(app.dag(&token, &failed).await["status"], "FAILED");

    let cancelled = delete_ok(&app, &token, "/api/pipeline/dag/dag-queued").await;
    assert_eq!(cancelled["dag_id"], "dag-queued");
    assert_eq!(cancelled["cancelled_steps"], 1);
    assert!(cancelled["message"].is_string());
    let after = app.dag(&token, "dag-queued").await;
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
        let swept = delete_ok(&app, &token, &format!("/api/pipeline/{pipeline_id}")).await;
        assert_eq!(swept["success"], true, "{pipeline_id}");
        assert_eq!(swept["cancelled_count"], count, "{pipeline_id}");
    }
    assert_eq!(
        app.dag(&token, "dag-queued-by-pipeline").await["status"],
        "CANCELLED"
    );
    assert_eq!(app.dag(&token, &completed).await["status"], "COMPLETED");

    let bulk = retry_failed().await;
    assert_eq!(bulk["success"], true);
    assert_eq!(bulk["count"], 3);
    assert!(bulk["message"].is_string(), "{bulk}");
    for (dag_id, job_id) in [
        ("dag-queued", queued_job.as_str()),
        ("dag-queued-by-pipeline", "dag-queued-by-pipeline-job"),
    ] {
        assert_eq!(app.dag(&token, dag_id).await["status"], "PENDING");
        let requeued = job(&app, &token, job_id).await;
        assert_eq!(requeued["status"], "PENDING", "{dag_id}");
        assert_eq!(requeued["retry_count"], 1, "{dag_id}");
    }
    assert_eq!(app.settled_dag(&token, &failed).await["status"], "FAILED");
    assert_eq!(job(&app, &token, &failed_job).await["retry_count"], 1);
    assert_eq!(app.dag(&token, &completed).await["status"], "COMPLETED");

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
        let deleted = delete_ok(&app, &token, &format!("/api/pipeline/dag/{dag_id}/delete")).await;
        assert_eq!(deleted["dag_id"], dag_id);
        assert_missing(&app, &token, &format!("/api/pipeline/dag/{dag_id}")).await;
        for job_id in jobs {
            assert_missing(&app, &token, &format!("/api/pipeline/jobs/{job_id}")).await;
        }
        let remaining = format!("/api/pipeline/jobs?pipeline_id={dag_id}");
        let remaining = app.get_ok(&remaining, &token).await;
        assert_eq!(remaining["total"], 0);
        assert_eq!(remaining["items"], json!([]));
    }

    let unknown = app
        .delete("/api/pipeline/dag/no-such-dag/delete", &token)
        .await;
    unknown.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");
    let dags = app.get_ok("/api/pipeline/dags", &token).await;
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
    let staged = joined(&app.output_path("staging"), "clip.flv");

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
    let retry = format!("/api/pipeline/dag/{dag_id}/retry");
    let retried = post_ok(&app, &token, &retry).await;
    assert_eq!(retried["dag_id"], dag_id.as_str());
    assert_eq!(retried["retried_steps"], 2);
    assert_eq!(retried["job_ids"], json!([publish_job]));
    assert!(retried["message"].is_string(), "{retried}");

    let rerun = app.settled_dag(&token, &dag_id).await;
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
    assert_eq!(read_file(&conflict), "recording");

    let refused = post_empty(&app, &token, &retry).await;
    refused.assert_error(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    let unknown = post_empty(&app, &token, "/api/pipeline/dag/no-such-dag/retry").await;
    unknown.assert_error(StatusCode::NOT_FOUND, "NOT_FOUND");

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
    let partly_failed = eventually(
        WAIT,
        "the publish step to fail while the slow step has a job",
        || async {
            let body = app.dag(&token, &dag_id).await;
            (step(&body, "publish")["status"] == "FAILED"
                && step(&body, "slow")["job_id"].is_string())
            .then_some(body)
        },
    )
    .await;
    assert_eq!(step(&partly_failed, "announce")["status"], "CANCELLED");
    let slow_job = step_job_id(&partly_failed, "slow");
    job_reaches(&app, &token, &slow_job, "PROCESSING").await;
    // The workflow is not finished while an independent branch still runs.
    let running = app.dag(&token, &dag_id).await;
    assert_eq!(running["status"], "PROCESSING");
    assert_eq!(step(&running, "slow")["status"], "PROCESSING");
    assert!(running["error"].is_string(), "{running}");

    let retry = format!("/api/pipeline/dag/{dag_id}/retry");
    let running_retry = post_empty(&app, &token, &retry).await;
    running_retry.assert_error(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");

    let cancelled = delete_ok(&app, &token, &format!("/api/pipeline/dag/{dag_id}")).await;
    assert_eq!(cancelled["cancelled_steps"], 1);
    let stopped = app.dag(&token, &dag_id).await;
    assert_eq!(stopped["status"], "CANCELLED");
    assert_eq!(step(&stopped, "slow")["status"], "CANCELLED");
    assert_eq!(job(&app, &token, &slow_job).await["status"], "CANCELLED");
    let stage_job = step_job_id(&stopped, "stage");
    let publish_job = step_job_id(&stopped, "publish");

    std::fs::remove_file(&conflict).expect("remove the conflicting file");
    let retried = post_ok(&app, &token, &retry).await;
    assert_eq!(retried["retried_steps"], 3);
    let restarted: BTreeSet<String> = strings(&retried["job_ids"]).into_iter().collect();
    assert_eq!(restarted, BTreeSet::from([publish_job, slow_job.clone()]));

    let resumed = eventually(WAIT, "the fixed branch to complete", || async {
        let body = app.dag(&token, &dag_id).await;
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
    delete_ok(&app, &token, &format!("/api/pipeline/dag/{dag_id}")).await;

    app.shutdown().await;
}

#[tokio::test]
async fn dag_list_stats_detail_and_graph_agree_on_status_strings() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let streamer_name = "E2E Pipeline Streamer";
    let streamer_id = app
        .disabled_streamer(&token, streamer_name, "https://live.bilibili.com/4063253")
        .await;
    let input = app.write_output_file("list-dags/clip.flv", "clip");

    let inputs = std::slice::from_ref(&input);
    let diamond_steps = vec![
        execute("root", "exit 0", &[]),
        execute("left", "exit 0", &["root"]),
        execute("right", "exit 0", &["root"]),
        execute("join", "exit 0", &["left", "right"]),
    ];
    let ids = ("session-diamond", streamer_id.as_str());
    let diamond = definition("Diamond", diamond_steps);
    let diamond = created_id(create_pipeline_as(&app, &token, ids, inputs, diamond).await);
    let completed = app.settled_dag(&token, &diamond).await;
    let ids = ("session-failed", "streamer-gone");
    let fails = definition("Fails", vec![execute("run", "exit 1", &[])]);
    let failed = created_id(create_pipeline_as(&app, &token, ids, inputs, fails).await);
    let failed_detail = app.settled_dag(&token, &failed).await;
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

    let queued = app.dag(&token, "dag-queued").await;
    assert_eq!(queued["status"], "PENDING");
    assert_eq!(step(&queued, "queued")["job_status"], "PENDING");

    // Step counts and the graph.
    let stats = app
        .get_ok(&format!("/api/pipeline/dag/{diamond}/stats"), &token)
        .await;
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
        .get_ok("/api/pipeline/dag/dag-queued/stats", &token)
        .await;
    assert_eq!(queued_stats["processing"], 1);
    assert_eq!(queued_stats["total"], 1);
    assert_eq!(queued_stats["progress_percent"], 0.0);

    let graph = app
        .get_ok(&format!("/api/pipeline/dag/{diamond}/graph"), &token)
        .await;
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
    assert_eq!(graph["edges"].as_array().map(Vec::len), Some(4), "{graph}");

    // The list and its counts.
    let all = app.get_ok("/api/pipeline/dags?limit=500", &token).await;
    assert_eq!(all["total"], 3);
    assert_eq!(all["limit"], 100);
    assert_eq!(all["offset"], 0);
    let items = all["dags"].as_array().expect("dags");
    for item in items {
        let status = item["status"].as_str().expect("status");
        assert!(
            ["PENDING", "PROCESSING", "COMPLETED", "FAILED", "CANCELLED"].contains(&status),
            "{item}"
        );
        assert_recent(item["created_at"].as_i64().expect("epoch milliseconds"));
        assert!(item["updated_at"].as_i64() >= item["created_at"].as_i64());
    }
    let listed = |id: &str| {
        items
            .iter()
            .find(|item| item["id"] == id)
            .unwrap_or_else(|| panic!("{id} in {all}"))
    };
    let item = listed(&diamond);
    assert_eq!(item["name"], "Diamond");
    assert_eq!(item["status"], "COMPLETED");
    assert_eq!(item["streamer_name"], streamer_name);
    assert_eq!(item["session_id"], "session-diamond");
    assert_eq!(item["total_steps"], 4);
    assert_eq!(item["completed_steps"], 4);
    assert_eq!(item["progress_percent"], 100.0);
    assert_eq!(listed(&failed)["streamer_name"], Value::Null);
    assert_eq!(listed(&failed)["failed_steps"], 1);

    let list_ids = |body: &Value| -> Vec<String> {
        body["dags"]
            .as_array()
            .expect("dags")
            .iter()
            .map(|item| item["id"].as_str().expect("id").to_string())
            .collect()
    };
    for (query, expected) in [
        ("status=completed", vec![diamond.clone()]),
        ("status=FAILED", vec![failed.clone()]),
        ("status=pending", vec!["dag-queued".to_string()]),
        ("status=processing", vec![]),
        ("session_id=session-failed", vec![failed.clone()]),
        ("search=e2e%20pipeline%20streamer", vec![diamond.clone()]),
    ] {
        let listed = app
            .get_ok(&format!("/api/pipeline/dags?{query}"), &token)
            .await;
        assert_eq!(list_ids(&listed), expected, "{query}");
        assert_eq!(listed["total"], expected.len(), "{query}");
    }
    let bogus = app.get("/api/pipeline/dags?status=bogus", &token).await;
    bogus.assert_error(StatusCode::BAD_REQUEST, "BAD_REQUEST");

    let counts = app.get_ok("/api/pipeline/dags/stats", &token).await;
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
        assert_missing(&app, &token, path).await;
    }

    app.shutdown().await;
}

// ---------------------------------------------------------------------------
// Workflow behaviour: expansion, steps without inputs, step policies, cancel
// and startup
// ---------------------------------------------------------------------------

/// A workflow step expands into the steps of the saved workflow it names,
/// prefixed with its own id, so one workflow can appear twice; what depends on
/// it depends on its leaves, in the order the workflow defines them.
#[tokio::test]
async fn workflow_steps_expand_under_their_own_ids_and_hand_on_their_leaves_in_order() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let input = app.write_output_file("expand/clip.flv", "clip");
    let leaves = ["leaf_c", "leaf_a", "leaf_e", "leaf_b", "leaf_d"];
    let mut fan_steps = vec![execute("root", "exit 0", &[])];
    fan_steps.extend(leaves.iter().map(|leaf| execute(leaf, "exit 0", &["root"])));
    for (name, steps) in [
        (
            "E2E Pair",
            vec![execute("A", "exit 0", &[]), execute("B", "exit 0", &["A"])],
        ),
        ("E2E Fan", fan_steps),
    ] {
        save_preset(&app, &token, name, definition(name, steps))
            .await
            .expect_status(StatusCode::CREATED);
    }
    let workflow = |id: &str, name: &str, depends_on: &[&str]| {
        json!({
            "id": id,
            "step": { "type": "workflow", "name": name },
            "depends_on": depends_on,
        })
    };

    let (_, settled) = run_pipeline(
        &app,
        &token,
        &[input],
        definition(
            "nested",
            vec![
                workflow("W1", "E2E Pair", &[]),
                workflow("W2", "E2E Pair", &["W1"]),
                workflow("F", "E2E Fan", &["W2"]),
                execute("Z", "exit 0", &["F"]),
            ],
        ),
    )
    .await;
    assert_eq!(settled["status"], "COMPLETED", "{settled}");
    let prefixed_leaves: Vec<String> = leaves.iter().map(|leaf| format!("F__{leaf}")).collect();
    let mut expected: Vec<(String, Value)> = vec![
        ("W1__A".into(), json!([])),
        ("W1__B".into(), json!(["W1__A"])),
        ("W2__A".into(), json!(["W1__B"])),
        ("W2__B".into(), json!(["W2__A"])),
        ("F__root".into(), json!(["W2__B"])),
        ("Z".into(), json!(prefixed_leaves)),
    ];
    expected.extend(
        prefixed_leaves
            .iter()
            .map(|leaf| (leaf.clone(), json!(["F__root"]))),
    );
    let steps = settled["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), expected.len(), "{settled}");
    for (step_id, depends_on) in expected {
        assert_eq!(
            step(&settled, &step_id)["depends_on"],
            depends_on,
            "{step_id}"
        );
    }

    app.shutdown().await;
}

/// A step whose dependencies left it no files settles without a job, so a
/// delete after a delete does not fail the workflow; an execute step runs
/// even with no files, since its command may not need any. Such a step can
/// also be left cancelled with every dependency completed, when its workflow
/// is stopped while the step is being released; a retry settles it the same
/// way. That state is put in place here by rewriting the finished workflow.
#[tokio::test]
async fn a_step_left_without_inputs_settles_without_a_job_on_completion_and_on_retry() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let source = app.write_output_file("no-inputs/clip.flv", "clip");

    let (dag_id, settled) = run_pipeline(
        &app,
        &token,
        std::slice::from_ref(&source),
        definition(
            "tidy up",
            vec![
                inline(
                    "copy",
                    "copy_move",
                    json!({ "destination": app.output_path("no-inputs/copies"), "operation": "copy" }),
                    &[],
                ),
                inline("tidy", "delete", json!({}), &["copy"]),
                execute("notify", "exit 0", &["tidy"]),
                inline("sweep", "delete", json!({}), &["notify"]),
            ],
        ),
    )
    .await;
    assert_eq!(settled["status"], "COMPLETED", "{settled}");
    assert_eq!(settled["completed_steps"], 4);
    assert_eq!(step(&settled, "tidy")["outputs"], json!([]));
    let notify = job(&app, &token, &step_job_id(&settled, "notify")).await;
    assert_eq!(notify["status"], "COMPLETED");
    assert_eq!(notify["input_path"], json!([]));
    let sweep = step(&settled, "sweep");
    assert_eq!(sweep["status"], "COMPLETED");
    assert_eq!(sweep["job_id"], Value::Null);
    assert_eq!(sweep["outputs"], json!([]));
    let jobs = app
        .get_ok(&format!("/api/pipeline/jobs?pipeline_id={dag_id}"), &token)
        .await;
    assert_eq!(jobs["total"], 3, "{jobs}");
    assert!(Path::new(&source).exists());

    app.run_sql(&format!(
        "UPDATE dag_step_execution SET status = 'CANCELLED' WHERE dag_id = '{dag_id}' AND step_id = 'sweep';
         UPDATE dag_execution SET status = 'FAILED', error = 'Stopped', completed_steps = 3 WHERE id = '{dag_id}';"
    ))
    .await;
    assert_eq!(app.dag(&token, &dag_id).await["status"], "FAILED");
    let retried = post_ok(&app, &token, &format!("/api/pipeline/dag/{dag_id}/retry")).await;
    assert_eq!(retried["retried_steps"], 1, "{retried}");
    assert_eq!(retried["job_ids"], json!([]));
    let resettled = app.settled_dag(&token, &dag_id).await;
    assert_eq!(resettled["status"], "COMPLETED", "{resettled}");
    assert_eq!(step(&resettled, "sweep")["status"], "COMPLETED");
    assert_eq!(step(&resettled, "sweep")["job_id"], Value::Null);
    assert_eq!(
        job(&app, &token, &step_job_id(&resettled, "notify")).await["retry_count"],
        0
    );

    app.shutdown().await;
}

/// The `error_message` of each `pipeline_failed` notification logged for
/// `job_id`, newest first.
async fn pipeline_failure_notices(app: &TestApp, token: &str, job_id: &str) -> Vec<String> {
    let events = "/api/notifications/events?event_type=pipeline_failed";
    app.get_ok(events, token)
        .await
        .as_array()
        .expect("events")
        .iter()
        .filter_map(|event| {
            // The payload is the event serialized as a JSON string.
            let payload: Value =
                serde_json::from_str(event["payload"].as_str().expect("payload")).ok()?;
            let failure = &payload["PipelineFailed"];
            (failure["job_id"] == job_id).then(|| {
                failure["error_message"]
                    .as_str()
                    .expect("error_message")
                    .to_string()
            })
        })
        .collect()
}

/// A failed step with attempts left keeps its workflow running and is run
/// again; once the attempts are spent the workflow fails. The job keeps its
/// plain error; only the notification of a failure that will be retried says
/// so.
#[tokio::test]
async fn a_step_with_a_retry_budget_runs_again_before_its_workflow_fails() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let input = app.write_output_file("budget/clip.flv", "clip");

    let dag_id = start_pipeline(
        &app,
        &token,
        &[input],
        definition(
            "budget",
            vec![with(
                execute("run", "exit 1", &[]),
                "retry",
                json!({ "max_attempts": 2, "backoff_secs": 2 }),
            )],
        ),
    )
    .await;
    let job_id = step_job_id(&app.dag(&token, &dag_id).await, "run");
    let first = job_reaches(&app, &token, &job_id, "FAILED").await;
    assert_eq!(first["retry_count"], 0, "{first}");
    assert_eq!(first["max_attempts"], 2);
    assert!(first["retry_after"].is_string(), "{first}");
    let first_error = first["error_message"].as_str().expect("error_message");
    assert!(!first_error.contains("retrying"), "{first}");
    let waiting = app.dag(&token, &dag_id).await;
    assert!(
        !TERMINAL_DAG_STATUSES.contains(&waiting["status"].as_str().unwrap_or_default()),
        "{waiting}"
    );
    assert_eq!(step(&waiting, "run")["status"], "PROCESSING");

    let spent = eventually(WAIT, "the second attempt to fail", || async {
        let body = job(&app, &token, &job_id).await;
        (body["status"] == "FAILED" && body["retry_count"] == 1).then_some(body)
    })
    .await;
    assert_eq!(spent["retry_after"], Value::Null, "{spent}");
    let spent_error = spent["error_message"].as_str().expect("error_message");
    assert!(!spent_error.contains("retrying"), "{spent}");
    let failed = app.settled_dag(&token, &dag_id).await;
    assert_eq!(failed["status"], "FAILED");
    assert_eq!(step(&failed, "run")["status"], "FAILED");
    let notices = eventually(WAIT, "both failures to be notified", || async {
        let notices = pipeline_failure_notices(&app, &token, &job_id).await;
        (notices.len() == 2).then_some(notices)
    })
    .await;
    assert!(
        notices[1].contains("; attempt 1 of 2 failed, retrying at "),
        "{notices:?}"
    );
    assert!(!notices[0].contains("retrying"), "{notices:?}");

    app.shutdown().await;
}

/// A command that runs for about 30 seconds unless it is stopped.
fn long_running_command() -> &'static str {
    if cfg!(windows) {
        "ping -n 31 127.0.0.1 >NUL"
    } else {
        "sleep 30"
    }
}

#[tokio::test]
async fn a_step_timeout_fails_a_job_that_outlives_it() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let input = app.write_output_file("timeout/clip.flv", "clip");

    let (_, failed) = run_pipeline(
        &app,
        &token,
        &[input],
        definition(
            "times out",
            vec![with(
                execute("wait", long_running_command(), &[]),
                "timeout_secs",
                json!(1),
            )],
        ),
    )
    .await;
    assert_eq!(failed["status"], "FAILED", "{failed}");
    let timed_out = job(&app, &token, &step_job_id(&failed, "wait")).await;
    let error = timed_out["error_message"].as_str().unwrap_or_default();
    assert!(error.starts_with("Job timed out after 1s"), "{timed_out}");

    app.shutdown().await;
}

/// A queued job whose type no worker pool claims would wait forever, so
/// startup fails it; a queued job of a known type is left to run.
#[tokio::test]
async fn startup_fails_queued_jobs_no_processor_claims_and_leaves_the_rest_to_run() {
    let now = now_ms();
    let app = TestApp::start_seeded(&format!(
        "INSERT INTO job (id, job_type, status, config, state, created_at, updated_at, input, \
             outputs, priority, streamer_id, session_id, retry_count) VALUES \
         ('job-orphaned', 'totally_not_a_processor', 'PENDING', '{{}}', '{{}}', {now}, {now}, \
             '[\"/recordings/orphaned.flv\"]', '[]', 0, 'streamer-seeded', 'session-seeded', 0), \
         ('job-known', 'execute', 'PENDING', '{{\"command\":\"exit 0\"}}', '{{}}', {now}, {now}, \
             '[\"/recordings/known.flv\"]', '[]', 0, 'streamer-seeded', 'session-seeded', 0);"
    ))
    .await;
    let token = app.admin_token().await;

    let orphaned = job(&app, &token, "job-orphaned").await;
    assert_eq!(orphaned["status"], "FAILED", "{orphaned}");
    let error = orphaned["error_message"].as_str().unwrap_or_default();
    assert!(error.contains("totally_not_a_processor"), "{orphaned}");
    eventually(WAIT, "the known job to run", || async {
        (job(&app, &token, "job-known").await["status"] == "COMPLETED").then_some(())
    })
    .await;

    app.shutdown().await;
}

// ---------------------------------------------------------------------------
// Interrupted outputs
// ---------------------------------------------------------------------------

/// The infix a temporary output's name carries between its final stem and
/// extension.
const TEMP_INFIX: &str = ".tmp-6d1c3d5e-8a3b-4c41-9d2e-1f0a9b7c2e11";

/// Record `path` as the temporary file an interrupted job left for `output`.
async fn seed_interrupted_output(app: &TestApp, id: &str, path: &str, output: &str) {
    let now = now_ms();
    let pool = app.database().await;
    sqlx::query(
        "INSERT INTO staged_outputs (id, path, output_path, job_id, attempt_id, created_at, \
             interrupted_at) VALUES (?, ?, ?, 'job-interrupted', 'attempt', ?, ?)",
    )
    .bind(id)
    .bind(path)
    .bind(output)
    .bind(now - 60_000)
    .bind(now - 30_000)
    .execute(&pool)
    .await
    .expect("seed an interrupted output");
    pool.close().await;
}

async fn interrupted_output_count(app: &TestApp, id: &str) -> i64 {
    app.query_i64(&format!(
        "SELECT COUNT(*) FROM staged_outputs WHERE id = '{id}'"
    ))
    .await
}

/// Settling a leftover whose file is already gone forgets it; one in a folder
/// that cannot be reached stays listed with its state unknown; a batch deletes
/// the files it settles; a link with a temporary name is never followed.
#[tokio::test]
async fn interrupted_outputs_settle_gone_unreachable_batched_and_linked_files() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    seed_interrupted_output(
        &app,
        "gone",
        &app.output_path(&format!("gone{TEMP_INFIX}.mp4")),
        &app.output_path("gone.mp4"),
    )
    .await;
    // In a folder that is missing, as on a drive that is not mounted.
    seed_interrupted_output(
        &app,
        "unplugged",
        &app.output_path(&format!("unplugged/clip{TEMP_INFIX}.mp4")),
        &app.output_path("unplugged/clip.mp4"),
    )
    .await;
    let partial = app.write_output_file(&format!("partial{TEMP_INFIX}.mp4"), b"partial");
    seed_interrupted_output(&app, "partial", &partial, &app.output_path("partial.mp4")).await;

    // A file already gone is settled without a deletion.
    let settled = delete_ok(&app, &token, "/api/pipeline/interrupted-outputs/gone").await;
    assert_eq!(settled, json!({ "id": "gone", "file_deleted": false }));
    assert_eq!(interrupted_output_count(&app, "gone").await, 0);

    // A file whose folder is unreachable stays listed, its state unknown. A
    // listing that overlaps another one in the same process skips the file
    // checks and reports every state unknown, so wait for one that ran them.
    let listed = eventually(WAIT, "a listing that checked the files", || async {
        let listed = app
            .get_ok("/api/pipeline/interrupted-outputs", &token)
            .await;
        listed["outputs"]
            .as_array()
            .expect("outputs")
            .iter()
            .any(|output| output["id"] == "partial" && output["exists"] == true)
            .then_some(listed)
    })
    .await;
    let outputs = listed["outputs"].as_array().expect("outputs");
    let entry = |id: &str| {
        outputs
            .iter()
            .find(|output| output["id"] == id)
            .unwrap_or_else(|| panic!("{id} in {listed}"))
    };
    assert_eq!(outputs.len(), 2, "{listed}");
    assert_eq!(entry("unplugged")["size_bytes"], Value::Null);
    assert_eq!(entry("unplugged")["exists"], Value::Null);
    assert_eq!(entry("partial")["size_bytes"], 7);
    assert_eq!(entry("partial")["exists"], true);
    assert_eq!(listed["total_size_bytes"], 7);
    assert_eq!(interrupted_output_count(&app, "unplugged").await, 1);

    let batch = app
        .post(
            "/api/pipeline/interrupted-outputs/batch-delete",
            &token,
            &json!({ "ids": ["partial", "no-such-output"], "keep_files": false }),
        )
        .await
        .ok();
    assert_eq!(batch["requested"], 2, "{batch}");
    assert_eq!(batch["succeeded"], 1);
    assert_eq!(batch["failed"], 1);
    assert_eq!(batch["results"][0]["id"], "partial");
    assert_eq!(batch["results"][0]["success"], true);
    assert_eq!(batch["results"][1]["code"], "NOT_FOUND");
    assert!(!Path::new(&partial).exists());
    assert_eq!(interrupted_output_count(&app, "partial").await, 0);

    // Only on Unix, where the test can create a symbolic link without extra
    // privileges: a link with the temporary name is refused and kept, and so
    // is the file it points to.
    #[cfg(unix)]
    {
        let target = app.write_output_file("recording.mp4", "keep");
        let link = app.output_path(&format!("video{TEMP_INFIX}.mp4"));
        std::os::unix::fs::symlink(&target, &link).expect("create the symlink");
        seed_interrupted_output(&app, "link", &link, &app.output_path("video.mp4")).await;
        let refused = app
            .delete("/api/pipeline/interrupted-outputs/link", &token)
            .await;
        refused.assert_error(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
        assert!(std::fs::symlink_metadata(&link).is_ok(), "the link is kept");
        assert_eq!(std::fs::read_to_string(&target).expect("target"), "keep");
        assert_eq!(interrupted_output_count(&app, "link").await, 1);
    }

    app.shutdown().await;
}
