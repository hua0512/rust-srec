use std::future::Future;

use async_trait::async_trait;
use tokio::sync::{mpsc, oneshot};

use srec_core::domain::StreamerState;
use srec_sqlite::models::StreamerDbModel;
use srec_sqlite::repositories::{
    MonitorOutboxTxOps, SqlxNotificationRepository, SqlxStreamerRepository, StreamerRepository as _,
};

use super::*;
use crate::{NotificationChannel, NotificationServiceConfig, NotifyError};

const WAIT: Duration = Duration::from_secs(10);

/// Records the label (the fatal error message) of every send in order, and
/// holds the sends whose label is in `hold` until the test answers them.
struct RecordingChannel {
    sent: Arc<parking_lot::Mutex<Vec<String>>>,
    hold: HashSet<&'static str>,
    held: mpsc::UnboundedSender<(String, oneshot::Sender<bool>)>,
}

#[async_trait]
impl NotificationChannel for RecordingChannel {
    fn channel_type(&self) -> &'static str {
        "recording"
    }

    fn is_enabled(&self) -> bool {
        true
    }

    async fn send(&self, event: &NotificationEvent) -> Result<()> {
        let NotificationEvent::FatalError { message, .. } = event else {
            return Ok(());
        };
        self.sent.lock().push(message.clone());
        if !self.hold.contains(message.as_str()) {
            return Ok(());
        }
        let (answer, answered) = oneshot::channel();
        self.held
            .send((message.clone(), answer))
            .map_err(|_| NotifyError::Other("test stopped listening".into()))?;
        match answered.await {
            Ok(true) => Ok(()),
            _ => Err(NotifyError::Other(format!("{message} failed"))),
        }
    }
}

async fn database(streamers: &[&str]) -> SqlitePool {
    let pool = srec_sqlite::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    srec_sqlite::run_migrations(&pool).await.unwrap();
    for id in streamers {
        let mut streamer = StreamerDbModel::new(
            format!("Streamer {id}"),
            format!("https://example.test/{id}"),
            "platform-twitch",
        );
        streamer.id = (*id).to_owned();
        SqlxStreamerRepository::new(pool.clone(), pool.clone())
            .create_streamer(&streamer)
            .await
            .unwrap();
    }
    pool
}

const CREATED_AT: i64 = 1_735_000_000_000;

/// Enqueue a fatal-error alert for `streamer` labelled `label`; returns its row id.
async fn enqueue(pool: &SqlitePool, streamer: &str, label: &str) -> i64 {
    enqueue_at(pool, streamer, label, CREATED_AT).await
}

/// [`enqueue`] an alert that occurred, and was queued, at `created_at_ms`.
async fn enqueue_at(pool: &SqlitePool, streamer: &str, label: &str, created_at_ms: i64) -> i64 {
    let event = OutboxEvent::FatalError {
        streamer_id: streamer.to_owned(),
        streamer_name: format!("Streamer {streamer}"),
        error_type: FatalErrorType::NotFound,
        message: label.to_owned(),
        new_state: StreamerState::NotFound,
        occurred_at_ms: created_at_ms,
    };
    let mut conn = pool.acquire().await.unwrap();
    MonitorOutboxTxOps::enqueue_raw(
        &mut conn,
        Some(streamer),
        event.event_type(),
        &serde_json::to_string(&event).unwrap(),
        created_at_ms,
    )
    .await
    .unwrap();
    sqlx::query_scalar("SELECT last_insert_rowid()")
        .fetch_one(&mut *conn)
        .await
        .unwrap()
}

async fn eventually<F, Fut>(what: &str, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    tokio::time::timeout(WAIT, async {
        while !check().await {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
}

async fn event_logs_of(pool: &SqlitePool, streamer: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM notification_event_log WHERE streamer_id = ?")
        .bind(streamer)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// One streamer's alert is stuck on a channel that hangs and then fails. A
/// full page of that streamer's later alerts waits behind it, in order, while
/// another streamer's alert, queued after all of them on the next page,
/// settles at once.
#[tokio::test]
async fn stuck_alert_holds_back_only_the_later_alerts_of_its_streamer() {
    let pool = database(&["s-a", "s-b"]).await;
    let later: Vec<String> = (2..=PAGE_SIZE + 1).map(|n| format!("A{n}")).collect();
    enqueue(&pool, "s-a", "A1").await;
    for label in &later {
        enqueue(&pool, "s-a", label).await;
    }
    let b1 = enqueue(&pool, "s-b", "B1").await;

    let service = Arc::new(NotificationService::with_repository(
        NotificationServiceConfig {
            max_retries: 1,
            ..Default::default()
        },
        Arc::new(SqlxNotificationRepository::new(pool.clone(), pool.clone())),
    ));
    let sent = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let (held_tx, mut held) = mpsc::unbounded_channel();
    service.insert_test_channel(
        "recording",
        Arc::new(RecordingChannel {
            sent: sent.clone(),
            hold: HashSet::from(["A1"]),
            held: held_tx,
        }),
    );
    let drainer = Arc::new(NotificationOutboxDrainer::new(
        pool.clone(),
        service.clone(),
        CancellationToken::new(),
    ));
    let drain = tokio::spawn({
        let drainer = drainer.clone();
        async move { drainer.drain_until_wait().await }
    });

    let (label, answer) = tokio::time::timeout(WAIT, held.recv())
        .await
        .expect("A1 must start")
        .unwrap();
    assert_eq!(label, "A1");
    eventually("B1 to settle while A1 is stuck", || async {
        sqlx::query_scalar::<_, bool>(
            "SELECT delivered_at IS NOT NULL FROM monitor_event_outbox WHERE id = ?",
        )
        .bind(b1)
        .fetch_one(&pool)
        .await
        .unwrap()
            && event_logs_of(&pool, "s-b").await == 1
    })
    .await;
    let mut started = sent.lock().clone();
    started.sort();
    assert_eq!(started, ["A1", "B1"]);
    assert_eq!(
        event_logs_of(&pool, "s-a").await,
        1,
        "no later alert of the stuck streamer may be admitted"
    );

    answer.send(false).unwrap();
    let report = tokio::time::timeout(WAIT, drain)
        .await
        .expect("the drain must finish once A1 fails")
        .unwrap()
        .unwrap();
    assert_eq!(
        report,
        OutboxDrainReport {
            delivered: later.len() + 2,
            skipped: 0,
            failed: 0,
            needs_wait: false,
        }
    );
    let order_of_a: Vec<String> = sent
        .lock()
        .iter()
        .filter(|label| label.starts_with('A'))
        .cloned()
        .collect();
    assert_eq!(order_of_a[0], "A1");
    assert_eq!(order_of_a[1..], later[..]);
    service.stop().await;
}

/// Make writing the event log of the alert labelled `label` fail. The tests
/// that use it subscribe no channel, so that entry is the alert's only record
/// and failing to write it fails the settlement.
async fn fail_event_log_of(pool: &SqlitePool, label: &str) {
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "CREATE TRIGGER fail_event_log_{label} BEFORE INSERT ON notification_event_log \
         WHEN NEW.payload LIKE '%\"{label}\"%' \
         BEGIN SELECT RAISE(ABORT, 'event log unavailable'); END"
    )))
    .execute(pool)
    .await
    .unwrap();
}

/// A row whose settlement fails waits for a later retry; its streamer's later
/// rows wait behind it while other streamers' rows are still delivered. The
/// streamer is deleted first, which clears the rows' `streamer_id` column, so
/// only their payload ties them to it.
#[tokio::test]
async fn failed_alert_holds_back_only_the_later_alerts_of_its_streamer() {
    let pool = database(&["s-a", "s-b"]).await;
    let a1 = enqueue(&pool, "s-a", "A1").await;
    let a2 = enqueue(&pool, "s-a", "A2").await;
    let b1 = enqueue(&pool, "s-b", "B1").await;
    sqlx::query("DELETE FROM streamers WHERE id = 's-a'")
        .execute(&pool)
        .await
        .unwrap();
    fail_event_log_of(&pool, "A1").await;
    let service = Arc::new(NotificationService::with_repository(
        NotificationServiceConfig::default(),
        Arc::new(SqlxNotificationRepository::new(pool.clone(), pool.clone())),
    ));
    let drainer =
        NotificationOutboxDrainer::new(pool.clone(), service.clone(), CancellationToken::new());

    let report = tokio::time::timeout(WAIT, drainer.drain_until_wait())
        .await
        .expect("the drain must finish")
        .unwrap();
    assert_eq!(
        report,
        OutboxDrainReport {
            delivered: 1,
            skipped: 0,
            failed: 1,
            needs_wait: true,
        }
    );
    let rows: Vec<(i64, Option<String>, bool, i64)> = sqlx::query_as(
        "SELECT id, streamer_id, delivered_at IS NOT NULL, attempts \
         FROM monitor_event_outbox ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        rows,
        [
            (a1, None, false, 1),
            (a2, None, false, 0),
            (b1, Some("s-b".to_owned()), true, 1)
        ]
    );
    service.stop().await;
}

/// The background loop admits a failed row again once its backoff expires,
/// and the later rows of its streamer follow it in order.
#[tokio::test]
async fn background_drain_retries_a_failed_alert_before_the_later_alerts_of_its_streamer() {
    let pool = database(&["s-a"]).await;
    let a1 = enqueue(&pool, "s-a", "A1").await;
    let a2 = enqueue(&pool, "s-a", "A2").await;
    fail_event_log_of(&pool, "A1").await;
    let service = Arc::new(NotificationService::with_repository(
        NotificationServiceConfig::default(),
        Arc::new(SqlxNotificationRepository::new(pool.clone(), pool.clone())),
    ));
    let cancellation = CancellationToken::new();
    let drainer = Arc::new(NotificationOutboxDrainer::new(
        pool.clone(),
        service.clone(),
        cancellation.clone(),
    ));
    let run = tokio::spawn({
        let drainer = drainer.clone();
        async move { drainer.run().await }
    });

    eventually("A1's first attempt to fail", || async {
        sqlx::query_scalar::<_, i64>("SELECT attempts FROM monitor_event_outbox WHERE id = ?")
            .bind(a1)
            .fetch_one(&pool)
            .await
            .unwrap()
            == 1
    })
    .await;
    sqlx::query("DROP TRIGGER fail_event_log_A1")
        .execute(&pool)
        .await
        .unwrap();
    eventually("both rows to be marked delivered", || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM monitor_event_outbox \
             WHERE id IN (?, ?) AND delivered_at IS NOT NULL",
        )
        .bind(a1)
        .bind(a2)
        .fetch_one(&pool)
        .await
        .unwrap()
            == 2
    })
    .await;
    // Only a delivery that settles writes an event-log entry, so the entries
    // are in the order the alerts were delivered.
    let logged: Vec<String> =
        sqlx::query_scalar("SELECT payload FROM notification_event_log ORDER BY rowid")
            .fetch_all(&pool)
            .await
            .unwrap();
    let order: Vec<String> = logged
        .iter()
        .map(|payload| match serde_json::from_str(payload).unwrap() {
            NotificationEvent::FatalError { message, .. } => message,
            event => panic!("unexpected event {event:?}"),
        })
        .collect();
    assert_eq!(order, ["A1", "A2"]);

    cancellation.cancel();
    tokio::time::timeout(WAIT, run)
        .await
        .expect("the drainer must stop once cancelled")
        .unwrap();
    service.stop().await;
}

/// A row delivered again after its first delivery went unrecorded is logged
/// and broadcast once, while a newer row that reuses the id of a pruned row is
/// a new alert and is logged and broadcast again.
#[tokio::test]
async fn each_alert_is_logged_and_broadcast_once_across_replays_and_reused_ids() {
    let pool = database(&["s-a"]).await;
    let service = Arc::new(NotificationService::with_repository(
        NotificationServiceConfig::default(),
        Arc::new(SqlxNotificationRepository::new(pool.clone(), pool.clone())),
    ));
    let mut broadcast = service.subscribe();
    let drainer =
        NotificationOutboxDrainer::new(pool.clone(), service.clone(), CancellationToken::new());
    let delivered_one = OutboxDrainReport {
        delivered: 1,
        ..Default::default()
    };

    let first = enqueue(&pool, "s-a", "A1").await;
    assert_eq!(drainer.drain_until_wait().await.unwrap(), delivered_one);
    // As after a restart between the event-log write and `delivered_at`.
    sqlx::query("UPDATE monitor_event_outbox SET delivered_at = NULL WHERE id = ?")
        .bind(first)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(drainer.drain_until_wait().await.unwrap(), delivered_one);
    // Pruning the newest row frees its id for the next one.
    sqlx::query("DELETE FROM monitor_event_outbox WHERE id = ?")
        .bind(first)
        .execute(&pool)
        .await
        .unwrap();
    let second = enqueue_at(&pool, "s-a", "A2", CREATED_AT + 1).await;
    assert_eq!(second, first);
    assert_eq!(drainer.drain_until_wait().await.unwrap(), delivered_one);

    assert_eq!(event_logs_of(&pool, "s-a").await, 2);
    let mut broadcasts = Vec::new();
    while let Ok(event) = broadcast.try_recv() {
        let NotificationEvent::FatalError { message, .. } = event else {
            panic!("unexpected broadcast {event:?}");
        };
        broadcasts.push(message);
    }
    assert_eq!(broadcasts, ["A1", "A2"]);
    service.stop().await;
}
