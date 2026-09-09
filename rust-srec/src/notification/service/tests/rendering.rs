use super::*;
use crate::notification::events::RenderedEvent;

struct RenderChannel {
    locale: &'static str,
    fail_once: bool,
    seen: Arc<parking_lot::Mutex<Vec<(usize, RenderedEvent)>>>,
}

#[async_trait::async_trait]
impl NotificationChannel for RenderChannel {
    fn channel_type(&self) -> &'static str {
        "render-test"
    }
    fn is_enabled(&self) -> bool {
        true
    }
    fn locale(&self) -> Option<&str> {
        Some(self.locale)
    }
    async fn send(&self, _: &NotificationEvent) -> Result<()> {
        panic!("delivery must supply shared rendering")
    }
    async fn send_rendered(&self, _: &NotificationEvent, rendered: &RenderedEvent) -> Result<()> {
        let mut seen = self.seen.lock();
        seen.push((std::ptr::from_ref(rendered) as usize, rendered.clone()));
        if self.fail_once && seen.len() == 1 {
            Err(crate::Error::Other("retry fixture".into()))
        } else {
            Ok(())
        }
    }
}

#[tokio::test]
async fn delivery_shares_locale_rendering_and_retry_skips_successful_recipients() {
    let service = NotificationService::with_config(NotificationServiceConfig {
        initial_retry_delay_ms: 3_600_000,
        max_retry_delay_ms: 3_600_000,
        ..Default::default()
    });
    let mut observations = Vec::new();
    for (key, locale, fail_once) in [("a", "en", false), ("b", "en", true), ("c", "zh-CN", false)] {
        let seen = Arc::new(parking_lot::Mutex::new(Vec::new()));
        service.registry.write().insert(Arc::new(RuntimeChannel {
            key: key.into(),
            db_channel_id: None,
            display_name: key.into(),
            channel_type: "render-test".into(),
            breaker: service.new_breaker(),
            channel: Arc::new(RenderChannel {
                locale,
                fail_once,
                seen: seen.clone(),
            }),
        }));
        observations.push(seen);
    }
    let event = NotificationEvent::SystemStartup {
        version: "shared".into(),
        timestamp: Utc::now(),
    };
    service.notify(event.clone()).await.unwrap();
    let first = observations
        .iter()
        .map(|seen| seen.lock()[0].clone())
        .collect::<Vec<_>>();
    assert_eq!(first[0].0, first[1].0);
    assert_ne!(first[0].0, first[2].0);
    assert_eq!(first[0].1, RenderedEvent::new(&event, "en"));
    assert_eq!(first[2].1, RenderedEvent::new(&event, "zh-CN"));
    let id = *service.pending_queue.iter().next().unwrap().key();
    service.process_notification(id).await;
    assert_eq!(
        observations
            .iter()
            .map(|seen| seen.lock().len())
            .collect::<Vec<_>>(),
        vec![1, 2, 1]
    );
    assert_eq!(service.stats().pending_count, 0);
    tokio::time::timeout(Duration::from_secs(1), service.stop())
        .await
        .unwrap();
}
