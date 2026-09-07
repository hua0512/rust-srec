use super::*;
use crate::database::models::job::{DagStep, PipelineStep};
use std::time::Duration;

const SESSION: &str = "delivery-session";
const STREAMER: &str = "streamer";

fn pipeline() -> DagPipelineDefinition {
    DagPipelineDefinition::new(
        "pipeline",
        vec![DagStep::new("step", PipelineStep::preset("remux"))],
    )
}

fn configure(session_id: &str) -> PipelineCoordinationEvent {
    configure_pipelines(session_id, false, false)
}

fn configure_pipelines(
    session_id: &str,
    paired: bool,
    session_complete: bool,
) -> PipelineCoordinationEvent {
    PipelineCoordinationEvent::ConfigureSession {
        session_id: session_id.to_owned(),
        streamer_id: STREAMER.to_owned(),
        danmu_enabled: false,
        segment_pipeline: None,
        paired_segment_pipeline: paired.then(pipeline),
        session_complete_pipeline: session_complete.then(pipeline),
    }
}

fn started() -> PipelineCoordinationEvent {
    PipelineCoordinationEvent::SegmentDagStarted {
        session_id: SESSION.to_owned(),
        streamer_id: STREAMER.to_owned(),
        segment_index: 0,
        source: SourceType::Video,
    }
}

fn completed() -> PipelineCoordinationEvent {
    PipelineCoordinationEvent::SegmentDagCompleted {
        session_id: SESSION.to_owned(),
        streamer_id: STREAMER.to_owned(),
        segment_index: 0,
        source: SourceType::Video,
        outputs: vec![PathBuf::from("video.mp4")],
    }
}

async fn enqueue(
    coordinator: &PipelineCoordinator,
    event: PipelineCoordinationEvent,
) -> oneshot::Receiver<Vec<PipelineCommand>> {
    let (reply, receiver) = oneshot::channel();
    coordinator
        .submit(CoordinatorRequest::Apply { event, reply })
        .await;
    receiver
}

#[tokio::test]
async fn shutdown_drains_completion_and_delivers_finalization_before_inline_handoff() {
    let coordinator = PipelineCoordinator::new();
    coordinator.apply_event_inline(configure_pipelines(SESSION, false, true));
    coordinator.apply_event_inline(started());
    coordinator.apply_event_inline(PipelineCoordinationEvent::SessionEnded {
        session_id: SESSION.to_owned(),
        streamer_id: STREAMER.to_owned(),
        should_run_session_complete: true,
    });
    coordinator.apply_event_inline(PipelineCoordinationEvent::SessionEndPersisted {
        session_id: SESSION.to_owned(),
    });
    let cancel = CancellationToken::new();
    let mut actor = Box::pin(coordinator.start(cancel.clone()));
    assert!(futures::poll!(actor.as_mut()).is_pending());
    let reply = enqueue(&coordinator, completed()).await;
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(1), actor)
        .await
        .unwrap();
    let commands = reply.await.unwrap();
    assert!(matches!(
        commands.as_slice(),
        [PipelineCommand::CreateSessionCompleteDag { .. }]
    ));
    assert_eq!(
        coordinator
            .session_outstanding(SESSION)
            .await
            .unwrap()
            .pending_dags,
        0
    );
    assert!(
        coordinator.apply_event(completed()).await.is_empty(),
        "terminal delivery must not be replayed"
    );
    assert!(coordinator.lock_admission().is_none());
}

#[tokio::test]
async fn actor_future_drop_drains_queries_and_events_in_accepted_order() {
    let coordinator = PipelineCoordinator::new();
    let mut actor = Box::pin(coordinator.start(CancellationToken::new()));
    assert!(futures::poll!(actor.as_mut()).is_pending());
    let first = enqueue(&coordinator, started()).await;
    let (reply, midway) = oneshot::channel();
    coordinator
        .submit(CoordinatorRequest::SessionOutstanding {
            session_id: SESSION.to_owned(),
            reply,
        })
        .await;
    let last = enqueue(&coordinator, completed()).await;
    drop(actor);
    first.await.unwrap();
    last.await.unwrap();
    assert_eq!(midway.await.unwrap().unwrap().pending_dags, 1);
    assert_eq!(
        coordinator
            .session_outstanding(SESSION)
            .await
            .unwrap()
            .pending_dags,
        0
    );
    assert_eq!(coordinator.active_session_count().await, 1);
}

#[tokio::test]
async fn full_queue_shutdown_releases_waiters_without_accepting_cancelled_sends() {
    let coordinator = PipelineCoordinator::new();
    let cancel = CancellationToken::new();
    let mut actor = Box::pin(coordinator.start(cancel.clone()));
    assert!(futures::poll!(actor.as_mut()).is_pending());
    let mut replies = Vec::new();
    for index in 0..COORDINATOR_CAPACITY {
        replies.push(enqueue(&coordinator, configure(&format!("accepted-{index}"))).await);
    }
    let mut abandoned = Box::pin(coordinator.apply_event(configure("never-accepted")));
    assert!(futures::poll!(abandoned.as_mut()).is_pending());
    drop(abandoned);
    let mut waiting = Box::pin(coordinator.apply_event(configure("after-drain")));
    assert!(futures::poll!(waiting.as_mut()).is_pending());
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(1), actor)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), waiting)
        .await
        .unwrap();
    for reply in replies {
        reply.await.unwrap();
    }
    assert_eq!(
        coordinator.active_session_count().await,
        COORDINATOR_CAPACITY + 1
    );
    assert!(
        coordinator
            .session_outstanding("never-accepted")
            .await
            .is_none()
    );
    assert!(
        coordinator
            .session_outstanding("after-drain")
            .await
            .is_some()
    );
}

#[tokio::test]
async fn old_capacity_reservation_cannot_publish_after_shutdown_and_restart() {
    let coordinator = PipelineCoordinator::new();
    let mut actor = Box::pin(coordinator.start(CancellationToken::new()));
    assert!(futures::poll!(actor.as_mut()).is_pending());
    let old_sender = coordinator.lock_admission().as_ref().unwrap().clone();
    let permit = old_sender.reserve().await.unwrap();
    let accepted = enqueue(&coordinator, configure("before-shutdown")).await;
    drop(actor); // Must not wait for the still-held reservation.
    accepted.await.unwrap();
    let mut replacement = Box::pin(coordinator.start(CancellationToken::new()));
    assert!(futures::poll!(replacement.as_mut()).is_pending());
    let (reply, receiver) = oneshot::channel();
    let request = CoordinatorRequest::Apply {
        event: configure("after-restart"),
        reply,
    };
    let unaccepted = coordinator
        .publish_reserved(&old_sender, permit, request)
        .expect("old permit must not grant admission to the new generation");
    assert_eq!(coordinator.active_session_count_inline(), 1);
    coordinator.submit(unaccepted).await;
    drop(replacement);
    receiver.await.unwrap();
    assert_eq!(coordinator.active_session_count().await, 2);
}

#[tokio::test]
async fn abort_drains_accepted_work_once_even_when_caller_dropped_its_reply() {
    let coordinator = Arc::new(PipelineCoordinator::new());
    let task_coordinator = coordinator.clone();
    let actor = tokio::spawn(async move { task_coordinator.start(CancellationToken::new()).await });
    tokio::time::timeout(Duration::from_secs(1), async {
        while coordinator.lock_admission().is_none() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let mut caller = Box::pin(coordinator.apply_event(started()));
    assert!(futures::poll!(caller.as_mut()).is_pending());
    drop(caller);
    actor.abort();
    assert!(actor.await.unwrap_err().is_cancelled());
    assert_eq!(
        coordinator
            .session_outstanding(SESSION)
            .await
            .unwrap()
            .pending_dags,
        1
    );
    let mut replacement = Box::pin(coordinator.start(CancellationToken::new()));
    assert!(futures::poll!(replacement.as_mut()).is_pending());
    let reply = enqueue(&coordinator, completed()).await;
    drop(replacement);
    reply.await.unwrap();
    assert_eq!(
        coordinator
            .session_outstanding(SESSION)
            .await
            .unwrap()
            .pending_dags,
        0
    );
}

#[tokio::test]
async fn closed_sender_fallback_preserves_counts_and_outstanding_state() {
    let coordinator = PipelineCoordinator::new();
    coordinator.apply_event_inline(configure_pipelines(SESSION, true, false));
    coordinator.apply_event_inline(PipelineCoordinationEvent::VideoSegmentCompleted {
        session_id: SESSION.to_owned(),
        streamer_id: STREAMER.to_owned(),
        segment_index: 0,
        path: PathBuf::from("video.mp4"),
    });
    let (sender, receiver) = mpsc::channel(COORDINATOR_CAPACITY);
    *coordinator.lock_admission() = Some(sender);
    drop(receiver);
    assert_eq!(coordinator.active_pair_count().await, 1);
    assert_eq!(coordinator.active_session_count().await, 1);
    assert!(coordinator.session_outstanding(SESSION).await.is_some());
    coordinator.apply_event(configure("inline")).await;
    assert_eq!(coordinator.active_session_count().await, 2);
}

fn controlled_actor(coordinator: &PipelineCoordinator) -> CoordinatorActor<'_> {
    let (sender, receiver) = mpsc::channel(COORDINATOR_CAPACITY);
    *coordinator.lock_admission() = Some(sender.clone());
    CoordinatorActor {
        coordinator,
        sender,
        receiver,
    }
}

#[tokio::test]
async fn lost_read_reply_retries_behind_already_accepted_events() {
    let coordinator = PipelineCoordinator::new();
    let mut actor = controlled_actor(&coordinator);
    let mut count = Box::pin(coordinator.active_session_count());
    assert!(futures::poll!(count.as_mut()).is_pending());
    drop(actor.receiver.try_recv().unwrap()); // Simulate losing only the reply transport.
    let earlier = enqueue(&coordinator, configure(SESSION)).await;
    assert!(
        futures::poll!(count.as_mut()).is_pending(),
        "read must reenter admission, not overtake queued events"
    );
    drop(actor);
    earlier.await.unwrap();
    assert_eq!(count.await, 1);
}

#[tokio::test]
async fn lost_reply_after_reduction_never_reapplies_a_non_idempotent_event() {
    let coordinator = PipelineCoordinator::new();
    for segment_index in [0, 1] {
        coordinator.apply_event_inline(PipelineCoordinationEvent::PairedDagStarted {
            session_id: SESSION.to_owned(),
            streamer_id: STREAMER.to_owned(),
            segment_index,
        });
    }
    assert_eq!(
        coordinator
            .session_outstanding_inline(SESSION)
            .unwrap()
            .pending_dags,
        2
    );
    let mut actor = controlled_actor(&coordinator);
    let mut application = Box::pin(coordinator.apply_event(
        PipelineCoordinationEvent::PairedDagCompleted {
            session_id: SESSION.to_owned(),
        },
    ));
    assert!(futures::poll!(application.as_mut()).is_pending());
    let CoordinatorRequest::Apply { event, reply } = actor.receiver.try_recv().unwrap() else {
        panic!("expected the submitted event");
    };
    PipelineCoordinator::lock_state(&coordinator.inner).apply_event(event);
    drop(reply);
    assert!(application.await.is_empty());
    drop(actor);
    assert_eq!(
        coordinator
            .session_outstanding(SESSION)
            .await
            .unwrap()
            .pending_dags,
        1,
        "replaying the completed event would incorrectly settle the second DAG"
    );
}
