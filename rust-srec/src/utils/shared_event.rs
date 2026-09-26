//! Optional shared fan-out for consumers that need the same encoded event.

use std::sync::{Arc, OnceLock};

use bytes::Bytes;
use tokio::sync::broadcast;

#[derive(Debug)]
pub(crate) struct SharedEvent<T> {
    pub(crate) event: T,
    encoded: OnceLock<Option<Bytes>>,
}

impl<T> SharedEvent<T> {
    pub(crate) fn new(event: T) -> Self {
        Self {
            event,
            encoded: OnceLock::new(),
        }
    }

    /// Filtering belongs to the caller: the cached representation must be
    /// independent of any individual subscriber's selection.
    pub(crate) fn encoded(&self, encode: impl FnOnce(&T) -> Option<Bytes>) -> Option<Bytes> {
        self.encoded.get_or_init(|| encode(&self.event)).clone()
    }
}

pub(crate) struct SharedEventSender<T> {
    tx: broadcast::Sender<Arc<SharedEvent<T>>>,
    publication: parking_lot::Mutex<()>,
}

impl<T> SharedEventSender<T> {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            tx: broadcast::channel(capacity).0,
            publication: parking_lot::Mutex::new(()),
        }
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<Arc<SharedEvent<T>>> {
        self.tx.subscribe()
    }

    pub(crate) fn receiver_count(&self) -> usize {
        self.tx.receiver_count()
    }
}

/// Keep domain subscribers on their existing contract and avoid shared-envelope
/// allocation when only they are listening. Encoding stays lazy, so a group of
/// subscribers that all filter out an event does not serialize it at all.
pub(crate) fn publish_shared<T: Clone>(
    domain: &broadcast::Sender<T>,
    shared: &SharedEventSender<T>,
    event: T,
) -> bool {
    if shared.receiver_count() == 0 {
        return domain.send(event).is_ok();
    }
    // Concurrent producers must publish in the same order to both audiences.
    let _publication = shared.publication.lock();
    let domain_sent = domain.receiver_count() > 0 && domain.send(event.clone()).is_ok();
    let shared_sent = shared.tx.send(Arc::new(SharedEvent::new(event))).is_ok();
    domain_sent || shared_sent
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn shared_fanout_encodes_once_and_preserves_domain_delivery() {
        let (domain, mut domain_rx) = broadcast::channel(8);
        let shared = SharedEventSender::new(8);
        let mut receivers = vec![shared.subscribe()];
        for _ in 0..7 {
            receivers.push(shared.subscribe());
        }
        assert!(publish_shared(&domain, &shared, "payload".to_string()));
        assert_eq!(domain_rx.try_recv().unwrap(), "payload");
        let encodes = AtomicUsize::new(0);
        let mut first_bytes: Option<Bytes> = None;
        for receiver in &mut receivers {
            let event = receiver.try_recv().unwrap();
            let bytes = event
                .encoded(|payload| {
                    encodes.fetch_add(1, Ordering::SeqCst);
                    Some(Bytes::copy_from_slice(payload.as_bytes()))
                })
                .unwrap();
            assert_eq!(bytes.as_ref(), b"payload");
            if let Some(first) = &first_bytes {
                assert_eq!(first.as_ptr(), bytes.as_ptr());
            }
            first_bytes = Some(bytes);
        }
        assert_eq!(encodes.load(Ordering::SeqCst), 1);
        drop(domain_rx);
        drop(receivers);
        assert!(!publish_shared(&domain, &shared, "unobserved".to_string()));
        assert_eq!(shared.tx.len(), 0);
    }
    #[test]
    fn concurrent_publishers_keep_domain_and_socket_order_identical() {
        let (domain, mut domain_rx) = broadcast::channel(512);
        let shared = SharedEventSender::new(512);
        let mut shared_rx = shared.subscribe();
        let start = std::sync::Barrier::new(8);
        std::thread::scope(|threads| {
            for producer in 0..8 {
                let domain = &domain;
                let shared = &shared;
                let start = &start;
                threads.spawn(move || {
                    start.wait();
                    for sequence in 0..32 {
                        assert!(publish_shared(domain, shared, (producer, sequence)));
                    }
                });
            }
        });
        let mut domain_events = Vec::new();
        let mut socket_events = Vec::new();
        while let Ok(event) = domain_rx.try_recv() {
            domain_events.push(event);
        }
        while let Ok(event) = shared_rx.try_recv() {
            socket_events.push(event.event);
        }
        assert_eq!(domain_events.len(), 256);
        assert_eq!(socket_events, domain_events);
    }
}
