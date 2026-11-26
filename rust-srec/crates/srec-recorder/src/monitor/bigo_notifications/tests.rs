use super::*;

fn member(uid: u32) -> Member {
    Member {
        uid: Some(uid),
        excluded: None,
    }
}

/// A rule with streamers `a` (uid 1) and `b` (uid 2), both subscribed
/// and checked at `now`, so their offline polls are answered.
fn subscribed(now: Instant) -> Rule {
    let mut rule = Rule::default();
    rule.set_streamers(HashMap::from([
        ("a".to_owned(), member(1)),
        ("b".to_owned(), member(2)),
    ]));
    while let Some(uid) = rule.next_subscription() {
        rule.subscribed(uid, true, now);
    }
    rule.checked("a", now, false);
    rule.checked("b", now, false);
    rule
}

fn due(rule: &mut Rule, now: Instant) -> Vec<String> {
    let mut ids = rule.take_due(now);
    ids.sort();
    ids
}

#[test]
fn offline_polls_are_answered_only_while_subscribed_after_a_real_check() {
    let start = Instant::now();
    let mut rule = Rule::default();
    rule.set_streamers(HashMap::from([("a".to_owned(), member(1))]));
    rule.checked("a", start, false);
    assert!(!rule.answers_offline("a", start), "not subscribed");

    assert_eq!(rule.next_subscription(), Some(1));
    let acked = start + Duration::from_secs(1);
    rule.subscribed(1, true, acked);
    // The check before the subscription may have missed a start.
    assert!(!rule.answers_offline("a", acked));

    let checked = acked + Duration::from_secs(1);
    rule.checked("a", checked, false);
    assert!(rule.answers_offline("a", checked));
    assert!(
        !rule.answers_offline("a", checked + SAFETY_CHECK),
        "safety check due"
    );

    rule.disconnected();
    assert!(!rule.answers_offline("a", checked));
    rule.next_subscription();
    let resubscribed = checked + Duration::from_secs(1);
    rule.subscribed(1, true, resubscribed);
    assert!(!rule.answers_offline("a", resubscribed), "reconnected");
    assert!(
        !rule.checked("a", resubscribed, true),
        "the start may predate the subscription, so it is no missed push"
    );
}

#[test]
fn a_streamer_that_was_excluded_is_answered_only_after_a_new_check() {
    let start = Instant::now();
    let mut rule = subscribed(start);
    let streamers = |a| HashMap::from([("a".to_owned(), a), ("b".to_owned(), member(2))]);
    rule.set_streamers(streamers(Member {
        uid: Some(1),
        excluded: Some(PollingReason::StreamerDisabled),
    }));
    rule.set_streamers(streamers(member(1)));
    // A start while it was excluded got no follow-up checks.
    assert!(!rule.answers_offline("a", start));
    assert!(rule.answers_offline("b", start), "b did not change");
    let checked = start + Duration::from_secs(1);
    rule.checked("a", checked, false);
    assert!(rule.answers_offline("a", checked));
}

#[test]
fn a_push_keeps_checks_real_until_one_reads_live_or_the_limit_passes() {
    let start = Instant::now();
    let mut rule = subscribed(start);
    let push = start + Duration::from_secs(10);
    rule.pushed(Some(1), push);
    assert!(!rule.answers_offline("a", push));
    assert!(rule.answers_offline("b", push));

    let mut asked = Vec::new();
    while let Some(at) = rule.next_due() {
        for id in rule.take_due(at) {
            asked.push((id, at - push));
        }
        // A check that still reads offline does not end the push.
        rule.checked("a", at, false);
    }
    let offsets: Vec<_> = RECHECKS
        .iter()
        .map(|offset| ("a".to_owned(), *offset))
        .collect();
    assert_eq!(asked, offsets);
    assert!(!rule.answers_offline("a", push + Duration::from_secs(90)));
    assert!(rule.answers_offline("a", push + PENDING_LIMIT));

    let again = push + PENDING_LIMIT;
    rule.pushed(Some(1), again);
    assert!(!rule.checked("a", again, true), "pushed");
    assert_eq!(rule.next_due(), None, "a live check ends the follow-ups");
    assert!(rule.checked("b", again, true), "b went live without a push");
    assert!(!rule.checked("b", again, true), "b was live already");
    // Ends are not pushed, so polls stay real until one reads offline.
    assert!(!rule.answers_offline("a", again));
    let ended = again + Duration::from_secs(1);
    rule.checked("a", ended, false);
    assert!(rule.answers_offline("a", ended));
}

#[test]
fn a_push_without_a_streamer_wakes_everyone_only_when_unpaired() {
    let start = Instant::now();
    let seconds = |n| start + Duration::from_secs(n);

    // BIGO's pair: the nameless push, then the named one.
    let mut rule = subscribed(start);
    rule.pushed(None, start);
    rule.pushed(Some(1), seconds(1));
    assert_eq!(due(&mut rule, start + PAIR_WINDOW), ["a"]);
    // The other order wakes no one either: only a's follow-ups remain.
    rule.pushed(None, seconds(2));
    assert_eq!(rule.next_due(), Some(seconds(6)));

    let mut rule = subscribed(start);
    rule.pushed(None, start);
    assert_eq!(due(&mut rule, seconds(4)), Vec::<String>::new());
    assert_eq!(due(&mut rule, start + PAIR_WINDOW), ["a", "b"]);
    assert!(!rule.answers_offline("b", start + PAIR_WINDOW));
    // Streamers an earlier push still holds are not woken again.
    rule.pushed(None, seconds(10));
    assert_eq!(due(&mut rule, seconds(20)), Vec::<String>::new());
    assert!(
        !rule.checked("b", seconds(20), true),
        "a nameless push may have been b's"
    );
}

/// The streamer goes live without a push, and offline again a second
/// later.
fn missed(rule: &mut Rule, streamer_id: &str, at: Instant) {
    assert!(rule.checked(streamer_id, at, true), "missed push");
    rule.checked(streamer_id, at + Duration::from_secs(1), false);
}

fn reason(rule: &Rule, streamer_id: &str) -> Option<PollingReason> {
    let (methods, _, _) = rule.methods(None);
    methods
        .into_iter()
        .find(|method| method.streamer_id == streamer_id)
        .and_then(|method| method.reason)
}

#[test]
fn polls_are_real_after_starts_in_a_row_without_a_push_until_one_arrives() {
    let start = Instant::now();
    let seconds = |n| start + Duration::from_secs(n);
    let mut rule = subscribed(start);
    missed(&mut rule, "b", seconds(10));
    missed(&mut rule, "b", seconds(20));
    assert!(rule.answers_offline("a", seconds(25)));
    missed(&mut rule, "a", seconds(30));
    assert!(!rule.answers_offline("a", seconds(35)));

    // Any push, even a nameless one, shows that pushes arrive.
    rule.pushed(None, seconds(40));
    assert!(rule.answers_offline("a", seconds(40)));
    missed(&mut rule, "b", seconds(50));
    missed(&mut rule, "b", seconds(60));
    missed(&mut rule, "b", seconds(70));
    assert!(
        rule.answers_offline("a", seconds(75)),
        "one streamer that keeps resuming does not trip it"
    );
    missed(&mut rule, "a", seconds(80));
    assert!(!rule.answers_offline("a", seconds(85)));
    assert_eq!(
        reason(&rule, "a"),
        Some(PollingReason::NotificationsNotArriving)
    );

    // It concerns the account, so a new connection keeps it.
    rule.disconnected();
    while let Some(uid) = rule.next_subscription() {
        rule.subscribed(uid, true, seconds(90));
    }
    rule.checked("a", seconds(100), false);
    rule.checked("b", seconds(100), false);
    assert!(!rule.answers_offline("a", seconds(100)), "reconnected");
    assert!(
        rule.checked("b", seconds(110), true),
        "real polls still find missed pushes"
    );

    rule.pushed(None, seconds(120));
    assert!(rule.answers_offline("a", seconds(120)));
    assert_eq!(reason(&rule, "a"), None);
}

#[test]
fn only_an_outage_past_the_grace_period_alerts_once_and_reports_its_end() {
    let start = Instant::now();
    let mut listener = ListenerStatus {
        platform_id: None,
        state: ListenerState::Connecting,
        reason: Some(PollingReason::Disconnected),
        message: None,
        missed_pushes: 0,
        last_missed_at: None,
        outage: None,
    };
    listener.down(start);
    assert!(!listener.take_alert(start + DOWN_ALERT_AFTER - Duration::from_secs(1)));
    assert!(!listener.up(), "a brief drop ends without an alert");

    listener.down(start);
    // Each retry of the same outage keeps its start.
    listener.down(start + Duration::from_secs(60));
    assert_eq!(listener.alert_at(), Some(start + DOWN_ALERT_AFTER));
    assert!(listener.take_alert(start + DOWN_ALERT_AFTER));
    assert!(
        !listener.take_alert(start + DOWN_ALERT_AFTER * 2),
        "once per outage"
    );
    assert_eq!(listener.alert_at(), None);
    assert!(listener.up(), "reconnecting answers the alert");
}
