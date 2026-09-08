use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

#[tokio::test]
async fn refresh_fanout_runs_independent_owners_concurrently_with_a_fixed_limit() {
    let entered = Arc::new(AtomicUsize::new(0));
    let active = Arc::new(AtomicUsize::new(0));
    let maximum = Arc::new(AtomicUsize::new(0));
    let completed = Arc::new(AtomicUsize::new(0));
    let started = Arc::new(tokio::sync::Notify::new());
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let work = {
        let (entered, active, maximum, completed, started, gate) = (
            entered.clone(),
            active.clone(),
            maximum.clone(),
            completed.clone(),
            started.clone(),
            gate.clone(),
        );
        tokio::spawn(async move {
            run_config_refreshes((0..37).map(|id| id.to_string()), |_| {
                let (entered, active, maximum, completed, started, gate) = (
                    entered.clone(),
                    active.clone(),
                    maximum.clone(),
                    completed.clone(),
                    started.clone(),
                    gate.clone(),
                );
                async move {
                    let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                    maximum.fetch_max(current, Ordering::SeqCst);
                    if entered.fetch_add(1, Ordering::SeqCst) + 1 == MAX_CONCURRENT_CONFIG_REFRESHES
                    {
                        started.notify_one();
                    }
                    gate.acquire().await.unwrap().forget();
                    active.fetch_sub(1, Ordering::SeqCst);
                    completed.fetch_add(1, Ordering::SeqCst);
                }
            })
            .await;
        })
    };
    tokio::time::timeout(Duration::from_secs(2), started.notified())
        .await
        .unwrap();
    assert_eq!(
        entered.load(Ordering::SeqCst),
        MAX_CONCURRENT_CONFIG_REFRESHES
    );
    assert_eq!(
        active.load(Ordering::SeqCst),
        MAX_CONCURRENT_CONFIG_REFRESHES
    );
    gate.add_permits(37);
    tokio::time::timeout(Duration::from_secs(2), work)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(completed.load(Ordering::SeqCst), 37);
    assert_eq!(
        maximum.load(Ordering::SeqCst),
        MAX_CONCURRENT_CONFIG_REFRESHES
    );
    assert_eq!(active.load(Ordering::SeqCst), 0);
}
