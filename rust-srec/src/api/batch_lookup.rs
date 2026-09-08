//! Best-effort related data for API lists and exports.

use std::collections::{HashMap, HashSet};
use std::future::Future;

use crate::database::repositories::LOOKUP_BATCH_SIZE;

/// A failed batch falls back to its individual owners: one malformed/missing owner
/// must not remove healthy neighbors from a response that previously tolerated it.
/// Batches and recovery reads are sequential to avoid unbounded pool contention.
pub(crate) async fn collect<T, B, S, BF, SF>(
    ids: impl IntoIterator<Item = String>,
    mut batch: B,
    mut single: S,
) -> HashMap<String, T>
where
    B: FnMut(Vec<String>) -> BF,
    S: FnMut(String) -> SF,
    BF: Future<Output = crate::Result<HashMap<String, T>>>,
    SF: Future<Output = crate::Result<T>>,
{
    let mut seen = HashSet::new();
    let ids: Vec<_> = ids
        .into_iter()
        .filter(|id| seen.insert(id.clone()))
        .collect();
    let mut collected = HashMap::new();
    for ids in ids.chunks(LOOKUP_BATCH_SIZE) {
        match batch(ids.to_vec()).await {
            Ok(values) => collected.extend(values),
            Err(_) => {
                let mut failed = 0;
                for id in ids {
                    match single(id.clone()).await {
                        Ok(value) => {
                            collected.insert(id.clone(), value);
                        }
                        Err(_) => failed += 1,
                    }
                }
                tracing::warn!(
                    owner_count = ids.len(),
                    failed_owners = failed,
                    "Batch lookup failed; recovered related data by owner"
                );
            }
        }
    }
    collected
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::database::repositories::{
        FilterRepository, NotificationRepository, SqlxFilterRepository, SqlxNotificationRepository,
        SqlxStreamerRepository, StreamerRepository,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    pub(crate) async fn fixture() -> (sqlx::SqlitePool, Arc<AtomicUsize>) {
        let queries = Arc::new(AtomicUsize::new(0));
        let opened = queries.clone();
        let reused = queries.clone();
        // Every query below uses the pool executor once. Count both new and reused
        // acquisitions, without timing or global SQL logging hooks.
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .after_connect(move |_, _| {
                opened.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { Ok(()) })
            })
            .before_acquire(move |_, _| {
                reused.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { Ok(true) })
            })
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::database::run_migrations(&pool).await.unwrap();
        sqlx::raw_sql(
            r#"
            WITH RECURSIVE n(i) AS (VALUES(0) UNION ALL SELECT i + 1 FROM n WHERE i < 1000)
            INSERT INTO streamers(id,name,url,platform_config_id,state,priority)
            SELECT printf('owner-%04d',i), 'Name ' || i, 'https://example.com/' || i,
                   'platform-twitch','NOT_LIVE','NORMAL' FROM n;
            INSERT INTO notification_channel(id,name,channel_type,settings)
            SELECT id, name, 'Webhook', '{"enabled":true}' FROM streamers;
            INSERT INTO filters(id,streamer_id,filter_type,config)
            SELECT id || '-z', id, 'Z', '{"value":2}' FROM streamers WHERE id != 'owner-1000';
            INSERT INTO filters(id,streamer_id,filter_type,config)
            SELECT id || '-a', id, 'A', '{"value":1}' FROM streamers WHERE id != 'owner-1000';
            INSERT INTO notification_subscription(channel_id,event_name)
            SELECT id, 'ZEvent' FROM notification_channel WHERE id != 'owner-1000';
            INSERT INTO notification_subscription(channel_id,event_name)
            SELECT id, 'AEvent' FROM notification_channel WHERE id != 'owner-1000';
        "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        queries.store(0, Ordering::SeqCst);
        (pool, queries)
    }

    #[tokio::test]
    async fn sqlite_related_lookups_bound_queries_deduplicate_and_preserve_owner_order() {
        let (pool, queries) = fixture().await;
        let names = SqlxStreamerRepository::new(pool.clone(), pool.clone());
        let filters = SqlxFilterRepository::new(pool.clone(), pool.clone());
        let notifications = SqlxNotificationRepository::new(pool.clone(), pool.clone());
        let mut ids: Vec<_> = (0..=1000).map(|i| format!("owner-{i:04}")).collect();
        ids.push("missing".to_owned());
        ids.extend(std::iter::repeat_n("owner-0000".to_owned(), 500));
        let streamers = names.get_streamers_by_ids(&ids).await.unwrap();
        assert_eq!(queries.swap(0, Ordering::SeqCst), 3);
        assert_eq!(streamers.len(), 1001);
        let batched_filters = filters.get_filters_for_streamers(&ids).await.unwrap();
        assert_eq!(queries.swap(0, Ordering::SeqCst), 3);
        let batched_subscriptions = notifications
            .get_subscriptions_for_channels(&ids)
            .await
            .unwrap();
        assert_eq!(queries.swap(0, Ordering::SeqCst), 3);
        assert!(!batched_filters.contains_key("missing"));
        assert!(!batched_filters.contains_key("owner-1000"));
        assert!(!batched_subscriptions.contains_key("missing"));
        assert!(!batched_subscriptions.contains_key("owner-1000"));
        for id in ["owner-0000", "owner-0499", "owner-0500", "owner-0999"] {
            let individual = filters.get_filters_for_streamer(id).await.unwrap();
            assert_eq!(
                serde_json::to_value(&batched_filters[id]).unwrap(),
                serde_json::to_value(individual).unwrap()
            );
            assert_eq!(
                batched_subscriptions[id],
                notifications
                    .get_subscriptions_for_channel(id)
                    .await
                    .unwrap()
            );
        }
        queries.store(0, Ordering::SeqCst);
        assert!(names.get_streamers_by_ids(&[]).await.unwrap().is_empty());
        assert!(
            filters
                .get_filters_for_streamers(&[])
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            notifications
                .get_subscriptions_for_channels(&[])
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(queries.load(Ordering::SeqCst), 0);
        pool.close().await;
    }

    #[tokio::test]
    async fn failed_batch_recovers_healthy_owners_without_losing_other_batches() {
        let batch_calls = AtomicUsize::new(0);
        let single_calls = AtomicUsize::new(0);
        let values = collect(
            (0..1001).map(|i| i.to_string()).chain(["0".to_owned()]),
            |ids| {
                let batch = batch_calls.fetch_add(1, Ordering::SeqCst);
                async move {
                    if batch == 1 {
                        return Err(crate::Error::Other("injected lookup failure".to_owned()));
                    }
                    Ok(ids.into_iter().map(|id| (id.clone(), id)).collect())
                }
            },
            |id| {
                single_calls.fetch_add(1, Ordering::SeqCst);
                async move {
                    if id == "501" {
                        Err(crate::Error::not_found("owner", id))
                    } else {
                        Ok(id)
                    }
                }
            },
        )
        .await;
        assert_eq!(batch_calls.load(Ordering::SeqCst), 3);
        assert_eq!(single_calls.load(Ordering::SeqCst), 500);
        assert_eq!(values.len(), 1000);
        assert!(!values.contains_key("501"));
        assert_eq!(values["0"], "0");
        assert_eq!(values["999"], "999");
        assert_eq!(values["1000"], "1000");
    }
}
