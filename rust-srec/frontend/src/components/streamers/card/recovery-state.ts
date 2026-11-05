import type { z } from 'zod';
import type { StreamerSchema } from '@/api/schemas';
import type { Download } from '@/store/downloads';

const RECOVERY_PROGRESS_MIN_BYTES = 8n * 1024n * 1024n;

export function hasStrongRecoverySignal(
  activeDownload?: Download | null,
): boolean {
  if (!activeDownload) {
    return false;
  }

  return (
    activeDownload.segmentsCompleted > 0 ||
    (activeDownload.bytesDownloaded >= RECOVERY_PROGRESS_MIN_BYTES &&
      activeDownload.speedBytesPerSec > 0n)
  );
}

export function isStreamerRecovering(
  streamer: z.infer<typeof StreamerSchema>,
  hasRecoverySignal: boolean,
): boolean {
  return (
    (streamer.state === 'LIVE' || streamer.state === 'TEMPORAL_DISABLED') &&
    (streamer.consecutive_error_count > 0 ||
      !!streamer.disabled_until ||
      !!streamer.last_error) &&
    hasRecoverySignal
  );
}

const STOP_STATES = [
  'FATAL_ERROR',
  'OUT_OF_SPACE',
  'DISABLED',
  'CANCELLED',
  'ERROR',
] as const;

export type StopState = (typeof STOP_STATES)[number];

function isStopState(state: string): state is StopState {
  return (STOP_STATES as readonly string[]).includes(state);
}

/** What keeps a streamer's checks from running normally. */
export interface MonitoringState {
  disabledUntil: Date | null;
  isTemporarilyPaused: boolean;
  /** Set to the state when monitoring has stopped, `null` otherwise. */
  stopState: StopState | null;
  isRecovering: boolean;
}

/**
 * `now` is null until hydration: the server's clock cannot decide whether a
 * pause is over without risking a mismatch, so only the state field counts.
 */
export function monitoringState(
  streamer: z.infer<typeof StreamerSchema>,
  hasRecoverySignal: boolean,
  now: Date | null,
): MonitoringState {
  const disabledUntil = streamer.disabled_until
    ? new Date(streamer.disabled_until)
    : null;
  return {
    disabledUntil,
    isTemporarilyPaused:
      streamer.state === 'TEMPORAL_DISABLED' ||
      (now !== null && disabledUntil !== null && disabledUntil > now),
    stopState: isStopState(streamer.state) ? streamer.state : null,
    isRecovering: isStreamerRecovering(streamer, hasRecoverySignal),
  };
}
