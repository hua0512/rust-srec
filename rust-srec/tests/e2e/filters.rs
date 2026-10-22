//! Stream filters: the routes that store them, and filters holding back a
//! live streamer on the fake platform. Which titles, categories and times
//! each filter kind matches is pinned by the unit tests beside it.
//!
//! A held-back streamer is checked as usual but gets no session: its check
//! history records the check as `filtered` with the reason.

use chrono::Timelike;
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::support::{
    FakePlatform, FlvLoop, LocalHttpServer, Seed, TestApp, WAIT, eventually, id_of, succeed,
};

async fn wait_for_state(app: &TestApp, token: &str, id: &str, state: &str) {
    let path = format!("/api/streamers/{id}");
    eventually(WAIT, &format!("the streamer to be {state}"), || async {
        (app.get_ok(&path, token).await["state"] == state).then_some(())
    })
    .await;
}

async fn add_filter(
    app: &TestApp,
    token: &str,
    id: &str,
    filter_type: &str,
    config: Value,
) -> Value {
    let body = json!({ "streamer_id": id, "filter_type": filter_type, "config": config });
    app.post(&format!("/api/streamers/{id}/filters"), token, &body)
        .await
        .created()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn title_and_category_filters_hold_back_a_live_streamer_until_both_match() {
    let fake = FakePlatform::start().await;
    let room = fake.room("filtered");
    let app = TestApp::start_with(Seed::live()).await;
    let token = app.admin_token().await;
    // Created disabled and enabled once filtered, so its first check already
    // applies the filters.
    let id = app.disabled_streamer(&token, "Filtered", room.url()).await;
    let keyword = json!({ "include": [], "exclude": ["rerun"] });
    add_filter(&app, &token, &id, "KEYWORD", keyword).await;
    let category = json!({ "categories": ["Just Chatting"] });
    add_filter(&app, &token, &id, "CATEGORY", category).await;
    room.set_live(room.live("Rerun of yesterday").category("Just Chatting"));
    let enable = json!({ "enabled": true });
    let path = format!("/api/streamers/{id}");
    app.put(&path, &token, &enable)
        .await
        .expect_status(StatusCode::OK);

    for (title, category, reason) in [
        ("Rerun of yesterday", "Just Chatting", "TitleMismatch"),
        ("Live again", "Gaming", "CategoryMismatch"),
    ] {
        room.set_live(room.live(title).category(category));
        let check = eventually(WAIT, &format!("a check of {title:?}"), || async {
            app.checks(&token, &id)
                .await
                .into_iter()
                .find(|check| check["title"] == title && check["category"] == category)
        })
        .await;
        assert_eq!(check["outcome"], "filtered", "{check}");
        assert_eq!(check["filter_reason"], reason, "{check}");
        // The check is recorded before the state it leads to.
        wait_for_state(&app, &token, &id, "OUT_OF_SCHEDULE").await;
        assert_eq!(app.sessions(&token, &id).await, Vec::<Value>::new());
        assert_eq!(fake.requests_to(&room, "live.flv").len(), 0, "{title}");
    }

    room.set_live(room.live("Live again").category("Just Chatting"));
    room.wait_for_media_connection(1).await;
    let sessions = app.sessions(&token, &id).await;
    assert_eq!(sessions.len(), 1, "{sessions:?}");
    assert_eq!(sessions[0]["title"], "Live again");
    wait_for_state(&app, &token, &id, "LIVE").await;

    app.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_schedule_that_excludes_now_ends_a_recording_as_out_of_schedule() {
    let fake = FakePlatform::start().await;
    let room = fake.room("scheduled");
    room.set_live(room.live("Scheduled show"));
    let receiver = LocalHttpServer::start(200, "ok").await;
    let app =
        TestApp::start_with(Seed::live().session_complete_pipeline(&succeed("after-schedule")))
            .await;
    let token = app.admin_token().await;
    let channel = json!({
        "name": "Live alerts",
        "channel_type": "Webhook",
        "settings": { "url": receiver.url("/hook"), "locale": "en", "min_priority": 2 },
    });
    let channel = app
        .post("/api/notifications/channels", &token, &channel)
        .await
        .expect_status(StatusCode::CREATED);
    let subscriptions = format!(
        "/api/notifications/channels/{}/subscriptions",
        id_of(&channel.body)
    );
    let events = json!({ "events": ["stream_online", "stream_offline"] });
    app.put(&subscriptions, &token, &events)
        .await
        .expect_status(StatusCode::OK);
    let id = id_of(
        &app.create_enabled_streamer(&token, json!({ "name": "Scheduled", "url": room.url() }))
            .await,
    );
    room.wait_for_media_connection(1).await;
    room.push(FlvLoop::tiny().stream(2));
    eventually(WAIT, "the recording to be opened", || async {
        (std::fs::read_dir(app.output_dir())
            .expect("read the output directory")
            .count()
            > 0)
        .then_some(())
    })
    .await;

    // A window two hours from now on every day. Days are stored by their
    // full name, times exactly as written, seconds included.
    let hour = (chrono::Utc::now().hour() + 2) % 24;
    let (start, end) = (format!("{hour:02}:00:00"), format!("{hour:02}:30:59"));
    let schedule = json!({
        "days_of_week": ["Mon", "tue", "WED", "Thu", "Fri", "Sat", "Sun"],
        "start_time": start,
        "end_time": end,
    });
    let filter = add_filter(&app, &token, &id, "TIME_BASED", schedule).await;
    assert_eq!(
        filter["config"],
        json!({
            "days_of_week": [
                "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday",
            ],
            "start_time": start,
            "end_time": end,
        })
    );

    // The room is still live, so the backend has to hang up by itself.
    eventually(WAIT, "the backend to hang up", || async {
        room.media_closed().then_some(())
    })
    .await;
    let session = eventually(WAIT, "the session to end", || async {
        let sessions = app.sessions(&token, &id).await;
        assert_eq!(sessions.len(), 1, "{sessions:?}");
        (sessions[0]["is_live"] == false).then(|| sessions[0].clone())
    })
    .await;
    let session_id = id_of(&session);
    let detail = app.session(&token, &session_id).await;
    let events = detail["events"].as_array().expect("session events");
    let kinds: Vec<&Value> = events.iter().map(|event| &event["kind"]).collect();
    assert_eq!(kinds, ["session_started", "session_ended"], "{detail}");
    assert_eq!(
        events[1]["payload"],
        json!({ "kind": "session_ended", "cause": { "type": "out_of_schedule" }, "via_hysteresis": false })
    );
    wait_for_state(&app, &token, &id, "OUT_OF_SCHEDULE").await;
    let last = app.checks(&token, &id).await.pop().expect("a check");
    assert_eq!(last["outcome"], "filtered", "{last}");
    assert_eq!(last["filter_reason"], "OutOfSchedule", "{last}");
    assert_eq!(last["title"], "Scheduled show", "{last}");

    let dags = eventually(WAIT, "the session-complete workflow", || async {
        let dags = app.dags(&token, &session_id).await;
        (dags.len() == 1 && dags[0]["status"] == "COMPLETED").then_some(dags)
    })
    .await;
    assert_eq!(dags[0]["name"], "after-schedule");

    // Removing the filter resumes recording the room, which is still live,
    // in a new session.
    let filter_path = format!("/api/streamers/{id}/filters/{}", id_of(&filter));
    app.delete(&filter_path, &token)
        .await
        .expect_status(StatusCode::OK);
    room.wait_for_media_connection(2).await;
    let received = eventually(WAIT, "the second stream_online", || async {
        let received = receiver.received();
        (received.len() >= 2).then_some(received)
    })
    .await;
    // The first session ended before its workflow ran and finished, well
    // before the second one started, so a stream_offline for it would have
    // arrived by now.
    let events: Vec<Value> = received
        .iter()
        .map(|request| request.json()["event_type"].clone())
        .collect();
    assert_eq!(events, [json!("stream_online"), json!("stream_online")]);
    assert_eq!(app.sessions(&token, &id).await.len(), 2);

    app.shutdown().await;
}

#[tokio::test]
async fn an_edited_schedule_keeps_its_stored_timezone_unless_it_names_one() {
    let app = TestApp::start().await;
    let token = app.admin_token().await;
    let streamer = json!({ "name": "Zoned", "url": "https://live.bilibili.com/9101" });
    let id = id_of(&app.create_disabled_streamer(&token, streamer).await);
    let filters = format!("/api/streamers/{id}/filters");
    let schedule = || {
        json!({
            "days_of_week": ["Monday"],
            "start_time": "09:00",
            "end_time": "17:00",
            "extension": { "preserve": true },
        })
    };

    // An editor that resends the schedule without a timezone keeps the
    // stored one and unknown members; an explicit null selects UTC.
    let mut zoned = schedule();
    zoned["timezone"] = json!("Europe/Madrid");
    let created = add_filter(&app, &token, &id, "TIME_BASED", zoned).await;
    let path = format!("{filters}/{}", id_of(&created));
    let edited = app
        .patch(
            &path,
            &token,
            &json!({ "filter_type": "TIME_BASED", "config": schedule() }),
        )
        .await
        .ok();
    assert_eq!(edited["config"]["timezone"], "Europe/Madrid", "{edited}");
    assert_eq!(edited["config"]["extension"], json!({ "preserve": true }));
    assert_eq!(app.get_ok(&path, &token).await["config"], edited["config"]);
    let mut utc = schedule();
    utc["timezone"] = Value::Null;
    let reset = app
        .patch(&path, &token, &json!({ "config": utc }))
        .await
        .ok();
    assert!(reset["config"]["timezone"].is_null(), "{reset}");
    assert!(app.get_ok(&path, &token).await["config"]["timezone"].is_null());

    // A new schedule without a timezone stores none, and keeps unknown
    // members.
    let created = add_filter(&app, &token, &id, "TIME_BASED", schedule()).await;
    assert!(created["config"].get("timezone").is_none(), "{created}");
    assert_eq!(created["config"]["extension"], json!({ "preserve": true }));

    // Changing a cron filter into a schedule does not carry its timezone over.
    let cron = json!({ "expression": "0 * * * * *", "timezone": "Europe/Madrid" });
    let cron = add_filter(&app, &token, &id, "CRON", cron).await;
    let retyped = app
        .patch(
            &format!("{filters}/{}", id_of(&cron)),
            &token,
            &json!({ "filter_type": "TIME_BASED", "config": schedule() }),
        )
        .await
        .ok();
    assert_eq!(retyped["filter_type"], "TIME_BASED");
    assert!(retyped["config"].get("timezone").is_none(), "{retyped}");

    // A cron filter keeps `local` and unknown members too; both kinds refuse
    // an unknown or blank timezone.
    let local = json!({
        "expression": "0 * * * * *", "timezone": "local", "extension": { "preserve": true },
    });
    let local = add_filter(&app, &token, &id, "CRON", local).await;
    assert_eq!(local["config"]["timezone"], "local");
    assert_eq!(local["config"]["extension"], json!({ "preserve": true }));
    for kind in ["TIME_BASED", "CRON"] {
        for zone in ["invalid/zone", ""] {
            let mut config = if kind == "CRON" {
                json!({ "expression": "0 * * * * *" })
            } else {
                schedule()
            };
            config["timezone"] = json!(zone);
            let body = json!({ "streamer_id": id, "filter_type": kind, "config": config });
            let refused = app.post(&filters, &token, &body).await;
            refused.assert_error(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
        }
    }

    app.shutdown().await;
}
