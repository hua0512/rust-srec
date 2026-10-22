//! Processor behaviour that depends on the workflow around it: a copy or move
//! that partly fails, and its retry through the workflow.
//!
//! Each processor's own edge cases are pinned by its unit tests; the
//! scenarios here run real workflows on files under the app's output
//! directory.

use std::path::Path;

use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::support::{TestApp, id_of, inline, read_file, step};

fn exists(path: &str) -> bool {
    Path::new(path).exists()
}

/// A failed copy or move is retried from where it stopped: a destination
/// file of the input's size counts as transferred and a move removes the
/// source it left behind, while an input the failed attempt already moved
/// counts as done. The first attempt fails on the collision as a partial
/// failure that names the input, cancels what depends on the step and keeps
/// the copy it made.
#[tokio::test]
async fn retrying_a_copy_or_move_resumes_past_what_the_failed_attempt_transferred() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;

    for operation in ["copy", "move"] {
        let done = app.write_output_file(&format!("{operation}/rec/done.flv"), "recorded");
        let pending = app.write_output_file(&format!("{operation}/rec/pending.flv"), "later");
        // Joined as the processor joins its destination and a file name.
        let dest = app.output_path(&format!("{operation}/dest"));
        let in_dest = |name: &str| Path::new(&dest).join(name).to_string_lossy().into_owned();
        let (dest_done, dest_pending) = (in_dest("done.flv"), in_dest("pending.flv"));
        // What an earlier, interrupted transfer of `done` left behind.
        std::fs::create_dir_all(&dest).expect("create the destination");
        std::fs::write(&dest_done, "recorded").expect("write the earlier transfer");

        let created = app
            .post(
                "/api/pipeline/create",
                &token,
                &json!({
                    "session_id": "session-e2e",
                    "streamer_id": "streamer-e2e",
                    "input_paths": [done, pending],
                    "dag": {
                        "name": format!("{operation} then announce"),
                        "steps": [
                            inline(
                                "transfer",
                                "copy_move",
                                json!({ "destination": dest, "operation": operation }),
                                &[],
                            ),
                            inline("announce", "execute", json!({ "command": "exit 0" }), &["transfer"]),
                        ],
                    },
                }),
            )
            .await
            .expect_status(StatusCode::CREATED);
        let dag_id = created.body["pipeline_id"].as_str().expect("pipeline_id");
        let failed = app.settled_dag(&token, dag_id).await;
        assert_eq!(failed["status"], "FAILED", "{operation}: {failed}");
        let workflow_error = failed["error"].as_str().unwrap_or_default();
        assert!(
            workflow_error.contains("1 of 2 inputs failed"),
            "{operation}: {failed}"
        );
        assert_eq!(step(&failed, "transfer")["status"], "FAILED");
        assert_eq!(step(&failed, "announce")["status"], "CANCELLED");
        assert_eq!(step(&failed, "announce")["job_id"], Value::Null);
        let transfer_id = step(&failed, "transfer")["job_id"]
            .as_str()
            .expect("transfer job");
        let transfer = app
            .get_ok(&format!("/api/pipeline/jobs/{transfer_id}"), &token)
            .await;
        assert_eq!(transfer["status"], "FAILED", "{operation}");
        let collision =
            format!("Destination file already exists and overwrite is disabled: {dest_done}");
        assert_eq!(
            transfer["error_message"],
            format!("1 of 2 inputs failed: {done}: {collision}"),
            "{operation}"
        );
        assert_eq!(
            transfer["execution_info"]["items_produced"],
            json!([dest_pending]),
            "{operation}"
        );
        // The processor's log of the input it could not transfer is kept, once.
        let logs = format!("/api/pipeline/jobs/{}/logs?limit=1000", id_of(&transfer));
        let logs = app.get_ok(&logs, &token).await;
        let logged = logs["items"]
            .as_array()
            .expect("items")
            .iter()
            .filter(|item| item["message"] == collision.as_str())
            .count();
        assert_eq!(logged, 1, "{operation}: {logs}");
        assert_eq!(read_file(&dest_pending), "later", "{operation}");
        assert_eq!(exists(&pending), operation == "copy", "{operation}");

        app.post(
            &format!("/api/pipeline/dag/{dag_id}/retry"),
            &token,
            &json!({}),
        )
        .await
        .expect_status(StatusCode::OK);
        let resumed = app.settled_dag(&token, dag_id).await;
        assert_eq!(resumed["status"], "COMPLETED", "{operation}: {resumed}");
        assert_eq!(
            step(&resumed, "transfer")["outputs"],
            json!([dest_done, dest_pending]),
            "{operation}"
        );
        assert_eq!(step(&resumed, "announce")["status"], "COMPLETED");
        assert_eq!(read_file(&dest_done), "recorded", "{operation}");
        assert_eq!(read_file(&dest_pending), "later", "{operation}");
        assert_eq!(exists(&done), operation == "copy", "{operation}");
        assert_eq!(exists(&pending), operation == "copy", "{operation}");
    }

    app.shutdown().await;
}
