//! Actor handle for type-safe message passing with backpressure support.
//!
//! The `ActorHandle` provides a way to send messages to actors with:
//! - Backpressure awareness (try_send with timeout fallback)
//! - Mailbox capacity monitoring
//! - Priority channel support for high-priority messages

use std::fmt;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Default mailbox capacity for actors.
pub const DEFAULT_MAILBOX_CAPACITY: usize = 256;

/// Default timeout for send operations when mailbox is full.
pub const DEFAULT_SEND_TIMEOUT: Duration = Duration::from_millis(100);

/// Backpressure warning threshold (80% of capacity).
pub const BACKPRESSURE_THRESHOLD: f64 = 0.8;

/// Error type for send operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendError {
    /// The actor has stopped and is no longer accepting messages.
    ActorStopped,
    /// The mailbox is full and the send timed out.
    MailboxFull,
    /// The send operation timed out.
    Timeout,
}

impl fmt::Display for SendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SendError::ActorStopped => write!(f, "Actor has stopped"),
            SendError::MailboxFull => write!(f, "Mailbox is full"),
            SendError::Timeout => write!(f, "Send operation timed out"),
        }
    }
}

impl std::error::Error for SendError {}

/// Metadata about an actor.
#[derive(Debug, Clone)]
pub struct ActorMetadata {
    /// Unique actor identifier.
    pub id: String,
    /// Actor type (e.g., "streamer", "platform").
    pub actor_type: String,
    /// When the actor was spawned.
    pub spawned_at: Instant,
    /// Whether this is a high-priority actor.
    pub high_priority: bool,
}

impl ActorMetadata {
    /// Create metadata for a streamer actor.
    pub fn streamer(id: impl Into<String>, high_priority: bool) -> Self {
        Self {
            id: id.into(),
            actor_type: "streamer".to_string(),
            spawned_at: Instant::now(),
            high_priority,
        }
    }

    /// Create metadata for a platform actor.
    pub fn platform(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            actor_type: "platform".to_string(),
            spawned_at: Instant::now(),
            high_priority: false,
        }
    }

    /// Get the actor's uptime.
    pub fn uptime(&self) -> Duration {
        self.spawned_at.elapsed()
    }
}

/// A handle to an actor for sending messages.
///
/// The handle provides backpressure-aware message sending with:
/// - Fast path using `try_send` when mailbox has capacity
/// - Slow path with timeout when mailbox is full
/// - Priority channel for high-priority messages
pub struct ActorHandle<M> {
    /// Sender for the actor's normal mailbox.
    sender: mpsc::Sender<M>,
    /// Sender for high-priority messages (optional).
    priority_sender: Option<mpsc::Sender<M>>,
    /// Cancellation token for this actor.
    cancellation_token: CancellationToken,
    /// Actor metadata.
    pub metadata: ActorMetadata,
    /// Maximum mailbox capacity.
    max_capacity: usize,
}

impl<M> ActorHandle<M> {
    /// Create a new actor handle.
    pub fn new(
        sender: mpsc::Sender<M>,
        cancellation_token: CancellationToken,
        metadata: ActorMetadata,
    ) -> Self {
        let max_capacity = sender.max_capacity();
        Self {
            sender,
            priority_sender: None,
            cancellation_token,
            metadata,
            max_capacity,
        }
    }

    /// Create a new actor handle with priority channel support.
    pub fn with_priority(
        sender: mpsc::Sender<M>,
        priority_sender: mpsc::Sender<M>,
        cancellation_token: CancellationToken,
        metadata: ActorMetadata,
    ) -> Self {
        let max_capacity = sender.max_capacity();
        Self {
            sender,
            priority_sender: Some(priority_sender),
            cancellation_token,
            metadata,
            max_capacity,
        }
    }

    /// Send a message with backpressure awareness.
    ///
    /// This method first attempts a non-blocking send. If the mailbox is full,
    /// it falls back to a blocking send with a timeout.
    ///
    /// # Errors
    ///
    /// Returns `SendError::ActorStopped` if the actor has stopped.
    /// Returns `SendError::Timeout` if the send times out.
    pub async fn send(&self, msg: M) -> Result<(), SendError> {
        self.send_with_timeout(msg, DEFAULT_SEND_TIMEOUT).await
    }

    /// Send a message with a custom timeout.
    pub async fn send_with_timeout(&self, msg: M, timeout: Duration) -> Result<(), SendError> {
        // Fast path: try non-blocking send first
        match self.sender.try_send(msg) {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Full(msg)) => {
                // Slow path: wait for permit with timeout
                match tokio::time::timeout(timeout, self.sender.reserve()).await {
                    Ok(Ok(permit)) => {
                        permit.send(msg);
                        Ok(())
                    }
                    Ok(Err(_)) => Err(SendError::ActorStopped),
                    Err(_) => Err(SendError::Timeout),
                }
            }
            Err(mpsc::error::TrySendError::Closed(_)) => Err(SendError::ActorStopped),
        }
    }

    /// Send a high-priority message.
    ///
    /// If a priority channel is configured, the message is sent through it.
    /// Otherwise, it falls back to the normal channel.
    pub async fn send_priority(&self, msg: M) -> Result<(), SendError> {
        if let Some(ref priority_sender) = self.priority_sender {
            match priority_sender.try_send(msg) {
                Ok(()) => Ok(()),
                Err(mpsc::error::TrySendError::Full(msg)) => {
                    match tokio::time::timeout(DEFAULT_SEND_TIMEOUT, priority_sender.reserve())
                        .await
                    {
                        Ok(Ok(permit)) => {
                            permit.send(msg);
                            Ok(())
                        }
                        Ok(Err(_)) => Err(SendError::ActorStopped),
                        Err(_) => Err(SendError::Timeout),
                    }
                }
                Err(mpsc::error::TrySendError::Closed(_)) => Err(SendError::ActorStopped),
            }
        } else {
            self.send(msg).await
        }
    }

    /// Try to send a message without blocking.
    ///
    /// Returns immediately with an error if the mailbox is full.
    pub fn try_send(&self, msg: M) -> Result<(), SendError> {
        match self.sender.try_send(msg) {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Full(_)) => Err(SendError::MailboxFull),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(SendError::ActorStopped),
        }
    }

    /// Get the current and maximum mailbox capacity.
    ///
    /// Returns `(current_available, max_capacity)`.
    pub fn mailbox_capacity(&self) -> (usize, usize) {
        (self.sender.capacity(), self.max_capacity)
    }

    /// Get the current mailbox usage as a percentage (0.0 to 1.0).
    pub fn mailbox_usage(&self) -> f64 {
        let (available, max) = self.mailbox_capacity();
        if max == 0 {
            return 0.0;
        }
        1.0 - (available as f64 / max as f64)
    }

    /// Check if backpressure should be applied (mailbox >= 80% full).
    pub fn should_apply_backpressure(&self) -> bool {
        self.mailbox_usage() >= BACKPRESSURE_THRESHOLD
    }

    /// Check if the mailbox is full.
    pub fn is_mailbox_full(&self) -> bool {
        self.sender.capacity() == 0
    }

    /// Cancel this actor.
    pub fn cancel(&self) {
        self.cancellation_token.cancel();
    }

    /// Check if this actor has been cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.cancellation_token.is_cancelled()
    }

    /// Get a child cancellation token for this actor.
    pub fn child_token(&self) -> CancellationToken {
        self.cancellation_token.child_token()
    }

    /// Get the actor's ID.
    pub fn id(&self) -> &str {
        &self.metadata.id
    }

    /// Check if this is a high-priority actor.
    pub fn is_high_priority(&self) -> bool {
        self.metadata.high_priority
    }
}

impl<M> Clone for ActorHandle<M> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            priority_sender: self.priority_sender.clone(),
            cancellation_token: self.cancellation_token.clone(),
            metadata: self.metadata.clone(),
            max_capacity: self.max_capacity,
        }
    }
}

impl<M> fmt::Debug for ActorHandle<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ActorHandle")
            .field("metadata", &self.metadata)
            .field("capacity", &self.mailbox_capacity())
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_actor_handle_send() {
        let (tx, mut rx) = mpsc::channel::<u32>(10);
        let token = CancellationToken::new();
        let metadata = ActorMetadata::streamer("test", false);
        let handle = ActorHandle::new(tx, token, metadata);

        // Send should succeed
        handle.send(42).await.unwrap();

        // Receive the message
        let msg = rx.recv().await.unwrap();
        assert_eq!(msg, 42);
    }

    #[tokio::test]
    async fn test_actor_handle_try_send() {
        let (tx, _rx) = mpsc::channel::<u32>(10);
        let token = CancellationToken::new();
        let metadata = ActorMetadata::streamer("test", false);
        let handle = ActorHandle::new(tx, token, metadata);

        // Try send should succeed
        handle.try_send(42).unwrap();
    }

    #[tokio::test]
    async fn test_actor_handle_mailbox_full() {
        let (tx, _rx) = mpsc::channel::<u32>(1);
        let token = CancellationToken::new();
        let metadata = ActorMetadata::streamer("test", false);
        let handle = ActorHandle::new(tx, token, metadata);

        // Fill the mailbox
        handle.try_send(1).unwrap();

        // Next try_send should fail
        let result = handle.try_send(2);
        assert_eq!(result, Err(SendError::MailboxFull));
    }

    #[tokio::test]
    async fn test_actor_handle_actor_stopped() {
        let (tx, rx) = mpsc::channel::<u32>(10);
        let token = CancellationToken::new();
        let metadata = ActorMetadata::streamer("test", false);
        let handle = ActorHandle::new(tx, token, metadata);

        // Drop the receiver
        drop(rx);

        // Send should fail with ActorStopped
        let result = handle.send(42).await;
        assert_eq!(result, Err(SendError::ActorStopped));
    }

    #[tokio::test]
    async fn test_actor_handle_mailbox_capacity() {
        let (tx, _rx) = mpsc::channel::<u32>(10);
        let token = CancellationToken::new();
        let metadata = ActorMetadata::streamer("test", false);
        let handle = ActorHandle::new(tx, token, metadata);

        let (available, max) = handle.mailbox_capacity();
        assert_eq!(max, 10);
        assert_eq!(available, 10);

        // Send a message
        handle.try_send(1).unwrap();

        let (available, max) = handle.mailbox_capacity();
        assert_eq!(max, 10);
        assert_eq!(available, 9);
    }

    #[tokio::test]
    async fn test_actor_handle_backpressure_threshold() {
        let (tx, _rx) = mpsc::channel::<u32>(10);
        let token = CancellationToken::new();
        let metadata = ActorMetadata::streamer("test", false);
        let handle = ActorHandle::new(tx, token, metadata);

        // Initially no backpressure
        assert!(!handle.should_apply_backpressure());

        // Fill to 80%
        for i in 0..8 {
            handle.try_send(i).unwrap();
        }

        // Now should apply backpressure
        assert!(handle.should_apply_backpressure());
    }

    #[tokio::test]
    async fn test_actor_handle_cancellation() {
        let (tx, _rx) = mpsc::channel::<u32>(10);
        let token = CancellationToken::new();
        let metadata = ActorMetadata::streamer("test", false);
        let handle = ActorHandle::new(tx, token, metadata);

        assert!(!handle.is_cancelled());
        handle.cancel();
        assert!(handle.is_cancelled());
    }

    #[tokio::test]
    async fn test_actor_handle_priority_send() {
        let (normal_tx, mut normal_rx) = mpsc::channel::<u32>(10);
        let (priority_tx, mut priority_rx) = mpsc::channel::<u32>(10);
        let token = CancellationToken::new();
        let metadata = ActorMetadata::streamer("test", true);
        let handle = ActorHandle::with_priority(normal_tx, priority_tx, token, metadata);

        // Send normal message
        handle.send(1).await.unwrap();

        // Send priority message
        handle.send_priority(2).await.unwrap();

        // Normal message goes to normal channel
        assert_eq!(normal_rx.recv().await.unwrap(), 1);

        // Priority message goes to priority channel
        assert_eq!(priority_rx.recv().await.unwrap(), 2);
    }

    #[test]
    fn test_actor_metadata_streamer() {
        let metadata = ActorMetadata::streamer("streamer-1", true);
        assert_eq!(metadata.id, "streamer-1");
        assert_eq!(metadata.actor_type, "streamer");
        assert!(metadata.high_priority);
    }

    #[test]
    fn test_actor_metadata_platform() {
        let metadata = ActorMetadata::platform("twitch");
        assert_eq!(metadata.id, "twitch");
        assert_eq!(metadata.actor_type, "platform");
        assert!(!metadata.high_priority);
    }

    #[test]
    fn test_send_error_display() {
        assert_eq!(SendError::ActorStopped.to_string(), "Actor has stopped");
        assert_eq!(SendError::MailboxFull.to_string(), "Mailbox is full");
        assert_eq!(SendError::Timeout.to_string(), "Send operation timed out");
    }
}
