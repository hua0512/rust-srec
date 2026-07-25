import { msg } from '@lingui/core/macro';
import type { MessageDescriptor } from '@lingui/core';

/**
 * Display names for the event types the backend reports, keyed by the `event_type` value it
 * stores on each log row and returns from `/notifications/event-types`.
 *
 * The API also carries a `label`, but it is a `&'static str` fixed at build time and always
 * English. The backend's own `rust_i18n` catalog is selected by the `RUST_SREC_LOCALE`
 * environment variable, which is a property of the server rather than of whoever is reading the
 * page, so neither source can follow the language picked in the browser.
 *
 * Keys must stay in step with `NotificationEventTypeInfo::ALL` in
 * `rust-srec/src/notification/events.rs`; `eventTypeLabel` falls back for anything missing.
 */
const EVENT_TYPE_LABELS: Record<string, MessageDescriptor> = {
  stream_online: msg`Stream online`,
  stream_offline: msg`Stream offline`,
  download_started: msg`Recording started`,
  download_completed: msg`Recording finished`,
  download_error: msg`Recording error`,
  download_cancelled: msg`Recording cancelled`,
  download_rejected: msg`Recording rejected`,
  segment_started: msg`Segment started`,
  segment_completed: msg`Segment finished`,
  config_updated: msg`Settings updated`,
  pipeline_started: msg`Pipeline started`,
  pipeline_completed: msg`Pipeline finished`,
  pipeline_failed: msg`Pipeline failed`,
  pipeline_cancelled: msg`Pipeline cancelled`,
  pipeline_queue_warning: msg`Pipeline queue backing up`,
  pipeline_queue_critical: msg`Pipeline queue critical`,
  fatal_error: msg`Fatal error`,
  out_of_space: msg`Out of disk space`,
  output_path_inaccessible: msg`Output folder unavailable`,
  gpu_unavailable: msg`GPU unavailable`,
  system_startup: msg`System started`,
  system_shutdown: msg`System shutting down`,
  credential_refreshed: msg`Credential refreshed`,
  credential_refresh_failed: msg`Credential refresh failed`,
  credential_invalid: msg`Credential invalid`,
  credential_expiring: msg`Credential expiring soon`,
};

/**
 * Resolve with `i18n._`.
 *
 * An event type added to the backend but not yet listed above still has to render as something
 * readable, so it falls back to the raw value with its underscores removed rather than to a
 * blank cell.
 */
export function eventTypeLabel(eventType: string): MessageDescriptor {
  // A descriptor rather than a bare string so callers have one type to pass to `i18n._`. An id
  // with no catalog entry resolves to itself, which is the humanized name.
  return EVENT_TYPE_LABELS[eventType] ?? { id: eventType.replace(/_/g, ' ') };
}
