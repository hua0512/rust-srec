//! What a restart keeps: [`TestApp::restart`] shuts the backend down and
//! starts a new one on the same data directory, as restarting the process
//! does.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};

use crate::support::{
    FakePlatform, FlvLoop, Seed, TestApp, WAIT, eventually, files_under, id_of, now_ms,
    seeded_streamer, sql_text, succeed,
};

/// A restart in the middle of a recording keeps its session: the stopped
/// part is stored, the new backend's first check finds the room still live
/// and records on into the same session, and the session-complete workflow
/// runs once, when the broadcast ends.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restart_during_a_recording_continues_the_same_session() {
    let fake = FakePlatform::start().await;
    let room = fake.room("restarted");
    room.set_live(room.live("Through a restart"));
    let app =
        TestApp::start_with(Seed::live().session_complete_pipeline(&succeed("after-restart")))
            .await;
    let token = app.admin_token().await;
    let id = id_of(
        &app.create_enabled_streamer(&token, json!({ "name": "Restarted", "url": room.url() }))
            .await,
    );

    let mut flv = FlvLoop::tiny();
    room.wait_for_media_connection(1).await;
    room.push(flv.stream(1));
    eventually(WAIT, "the recording to be opened", || async {
        (!files_under(app.output_dir()).is_empty()).then_some(())
    })
    .await;
    let sessions_path = format!("/api/sessions?streamer_id={id}&include_empty=true");
    let before = app.get_ok(&sessions_path, &token).await["items"].clone();
    let session_id = id_of(&before[0]);

    let app = app.restart().await;
    assert!(room.media_closed(), "the old backend kept recording");
    room.wait_for_media_connection(2).await;
    room.push(flv.stream(1));
    room.set_offline();
    room.end();

    let session = eventually(WAIT, "the session to end", || async {
        let sessions = app.get_ok(&sessions_path, &token).await["items"].clone();
        let sessions = sessions.as_array().expect("session items");
        assert_eq!(sessions.len(), 1, "{sessions:?}");
        (sessions[0]["is_live"] == false).then(|| sessions[0].clone())
    })
    .await;
    assert_eq!(id_of(&session), session_id);
    let detail = app
        .get_ok(&format!("/api/sessions/{session_id}"), &token)
        .await;
    let ends: Vec<&Value> = detail["events"]
        .as_array()
        .expect("session events")
        .iter()
        .filter(|event| event["kind"] == "session_ended")
        .collect();
    assert_eq!(ends.len(), 1, "{detail}");
    assert_eq!(
        ends[0]["payload"]["cause"],
        json!({ "type": "streamer_offline" })
    );

    let page = app
        .get_ok(&format!("/api/sessions/{session_id}/segments"), &token)
        .await;
    let mut parts: Vec<PathBuf> = page["items"]
        .as_array()
        .expect("segment items")
        .iter()
        .map(|segment| PathBuf::from(segment["file_path"].as_str().expect("file path")))
        .collect();
    parts.sort();
    assert_eq!(parts.len(), 2, "{page}");
    assert_eq!(files_under(app.output_dir()), parts);

    let dags = eventually(WAIT, "the session-complete workflow", || async {
        let dags = app
            .get_ok(
                &format!("/api/pipeline/dags?session_id={session_id}"),
                &token,
            )
            .await["dags"]
            .clone();
        let dags = dags.as_array().expect("dags").clone();
        (dags.iter().any(|dag| dag["status"] == "COMPLETED")).then_some(dags)
    })
    .await;
    assert_eq!(dags.len(), 1, "{dags:?}");
    assert_eq!(dags[0]["name"], "after-restart");

    app.shutdown().await;
}

/// A job still running when the backend stops is run again by the next one,
/// and a job that was waiting for a worker still runs; both workflows
/// complete.
///
/// Runs only on Unix: it needs a job that keeps running (`sleep`) the first
/// time and succeeds the second.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restart_runs_again_the_job_it_interrupted_and_the_jobs_still_queued() {
    // The shutdown lets a running job finish for 10 s before stopping it;
    // this job would run 30 s, so the wait proves nothing.
    rust_srec::pipeline::override_shutdown_drain_grace(Duration::from_millis(100));
    // One CPU worker, so the second workflow waits behind the first.
    let app = TestApp::start_seeded("UPDATE global_config SET max_concurrent_cpu_jobs = 1;").await;
    let token = app.admin_token().await;
    let input = app.write_output_file("restart/clip.flv", "clip");
    let marker = app.output_path("restart/started");
    let create = async |name: &str, command: &str| {
        let created = app
            .post(
                "/api/pipeline/create",
                &token,
                &json!({
                    "session_id": "session-e2e",
                    "streamer_id": "streamer-e2e",
                    "input_paths": [input],
                    "dag": {
                        "name": name,
                        "steps": [{
                            "id": "run",
                            "step": {
                                "type": "inline",
                                "processor": "execute",
                                "config": { "command": command },
                            },
                            "depends_on": [],
                        }],
                    },
                }),
            )
            .await
            .expect_status(reqwest::StatusCode::CREATED);
        created.body["pipeline_id"]
            .as_str()
            .expect("pipeline_id")
            .to_owned()
    };
    let dag_job = async |app: &TestApp, dag_id: &str| {
        eventually(WAIT, &format!("a job for workflow {dag_id}"), || async {
            let dag = app
                .get_ok(&format!("/api/pipeline/dag/{dag_id}"), &token)
                .await;
            dag["steps"][0]["job_id"].as_str().map(str::to_owned)
        })
        .await
    };
    let job_status = async |app: &TestApp, job_id: &str| {
        app.get_ok(&format!("/api/pipeline/jobs/{job_id}"), &token)
            .await["status"]
            .clone()
    };

    let interrupted = create(
        "interrupted",
        &format!("test -e '{marker}' || (touch '{marker}' && sleep 30)"),
    )
    .await;
    let queued = create("queued", "exit 0").await;
    let interrupted_job = dag_job(&app, &interrupted).await;
    let queued_job = dag_job(&app, &queued).await;
    eventually(WAIT, "the first job to start", || async {
        (job_status(&app, &interrupted_job).await == "PROCESSING"
            && std::path::Path::new(&marker).exists())
        .then_some(())
    })
    .await;
    assert_eq!(job_status(&app, &queued_job).await, "PENDING");

    let app = app.restart().await;
    for dag_id in [&interrupted, &queued] {
        let status = app.settled_dag(&token, dag_id).await["status"].clone();
        assert_eq!(status, "COMPLETED", "{dag_id}");
    }
    for job_id in [&interrupted_job, &queued_job] {
        assert_eq!(job_status(&app, job_id).await, "COMPLETED", "{job_id}");
    }

    app.shutdown().await;
}

/// An ended session with `session_complete_dispatched` and one recorded part.
fn ended_session(id: &str, streamer_id: &str, dispatched: bool) -> String {
    let ended = now_ms() - 60_000;
    let started = ended - 600_000;
    format!(
        "INSERT INTO live_sessions (id, streamer_id, streamer_name, start_time, end_time, \
             titles, total_size_bytes, session_complete_dispatched) \
         VALUES ('{id}', '{streamer_id}', '{streamer_id}', {started}, {ended}, '[]', 1000, {});\n\
         INSERT INTO session_segments (id, session_id, segment_index, file_path, duration_secs, \
             size_bytes, created_at, completed_at, persisted_at) \
         VALUES ('{id}-part', '{id}', 0, '/recordings/{id}.flv', 10.0, 1000, {ended}, {ended}, \
             {ended});\n",
        i32::from(dispatched),
    )
}

/// Startup recovery runs what an ended session still owes, once: a part that was
/// stored without its segment workflow gets one, and the session-complete workflow
/// follows it. A session whose end was already dispatched gets nothing, and the next
/// start finds nothing owed. The streamer is disabled, so nothing but recovery acts.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn startup_recovery_runs_what_an_ended_session_still_owes_exactly_once() {
    let fake = FakePlatform::start().await;
    let room = fake.room("owed");
    let app = TestApp::start_with(
        Seed::live()
            .platform("pipeline", sql_text(&succeed("segment").to_string()))
            .session_complete_pipeline(&succeed("after-recovery"))
            .rows(&format!(
                "{}{}{}",
                seeded_streamer("owed", &room, "DISABLED", &[]),
                ended_session("owed-session", "owed", false),
                ended_session("old-session", "owed", true),
            )),
    )
    .await;
    let token = app.admin_token().await;

    let names = |dags: &[Value]| {
        let mut names: Vec<String> = dags
            .iter()
            .map(|dag| dag["name"].as_str().expect("name").to_owned())
            .collect();
        names.sort();
        names
    };
    let owed = eventually(WAIT, "the owed workflows to complete", || async {
        let dags = app.dags(&token, "owed-session").await;
        (dags.len() == 2 && dags.iter().all(|dag| dag["status"] == "COMPLETED")).then_some(dags)
    })
    .await;
    assert_eq!(names(&owed), ["after-recovery", "segment"]);
    assert_eq!(
        app.query_i64(
            "SELECT segment_index FROM dag_execution \
             WHERE session_id = 'owed-session' AND segment_source = 'video'"
        )
        .await,
        0
    );
    assert_eq!(
        app.query_i64(
            "SELECT session_complete_dispatched FROM live_sessions WHERE id = 'owed-session'"
        )
        .await,
        1
    );
    assert_eq!(app.dags(&token, "old-session").await, Vec::<Value>::new());

    // Startup recovery has finished when the restarted backend answers.
    let app = app.restart().await;
    assert_eq!(names(&app.dags(&token, "owed-session").await), names(&owed));
    assert_eq!(app.dags(&token, "old-session").await, Vec::<Value>::new());

    app.shutdown().await;
}
