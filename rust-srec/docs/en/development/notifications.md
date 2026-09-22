# Notification internals

For contributors implementing channels and delivery workers. Operator setup is in [Notifications](../concepts/notifications.md).

## SMTP ownership

Email channels reuse SMTP connections for successive messages. Each configuration replacement owns a separate connection pool; deliveries already admitted against the previous configuration retain their original channel. Disabled or priority-filtered messages do not initialize SMTP transport. Email delivery remains immediate: the obsolete `batch_window_secs` setting is ignored in stored JSON and is no longer part of the Rust `EmailConfig` interface.

## Localized rendering

For each delivery attempt, the backend reads the server language once and shares rendered title/body text between channels using the same effective language. A retry takes a new language snapshot. Channel-specific escaping, email MIME parts and Telegram entities are applied afterward.

## Delivery Behavior {#delivery-behavior}

External channel delivery retries transient failures with backoff and uses a circuit breaker for repeatedly failing channels. Exhausted deliveries are dead-lettered and notification events remain available in the event history according to the configured retention period.

Database channel reloads publish channels and subscriptions together. A channel keeps its circuit-breaker history while its database ID stays loaded, including name, destination, credential, and subscription edits. Removal or disabling ends that generation; re-adding starts a fresh breaker. Notifications already admitted, including their retries, continue using the channel instance selected before the reload. A repository read failure preserves the loaded registry.

These mechanisms reduce transient loss but do not create an end-to-end delivery guarantee. Monitor the receiving service, use the channel test after configuration changes, and configure a second destination for critical events.

Web Push uses a 2,048-event FIFO queue and normal worker batches of up to 64 events. When the queue is full, the newest event is dropped at every priority, including critical. A missing or closed worker and service shutdown also reject new push events. The `notification_stats.web_push_dropped` counter records these admission failures, with rate-limited warning logs. Event-history persistence and external channel delivery continue independently; dropped push events are not replayed. Shutdown attempts to flush admitted events within the existing background-task shutdown budget, but admission does not guarantee delivery.

## Queue and Web Push Delivery {#queue-and-web-push-delivery}

Ordinary channel delivery serializes queue admission and evicts the oldest pending
notification at capacity, cancelling its scheduled retries. A zero queue limit
disables ordinary channel admission; event logging and Web Push remain independent.
An already-running send may finish after eviction. Open circuit breakers do not
consume delivery attempts, and retries respect the cooldown even when the failure
that opened the breaker has just occurred.

Web Push clears persisted throttling state on successful delivery, including the
first HTTP attempt. Stale-subscription deletion is reported only after SQLite
confirms it. Both normal and abbreviated JSON payloads must fit the byte limit;
oversized metadata fails before an HTTP request is sent.

## Backend Notification Interfaces {#backend-notification-interfaces}

Channel registry/reload operations, source-event mappings, delivery/retries and the
Web Push queue/worker each have a dedicated module. The public event catalog and
localized rendering methods remain available through `notification::events`.
Custom Rust channels can keep implementing `NotificationChannel::send`; the new
optional `send_rendered` method receives a `RenderedEvent` for integrations that
want to reuse the shared title/body. Direct built-in sends use the same filtering
and payload construction as queued delivery. The default `test` method follows
normal delivery filtering.
