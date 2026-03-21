import type { z } from 'zod';
import type { StreamerSchema } from '@/api/schemas';
import type { Download } from '@/store/downloads';

export const RECOVERY_PROGRESS_MIN_BYTES = 8n * 1024n * 1024n;

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
  activeDownload?: Download | null,
): boolean {
  return (
    (streamer.state === 'LIVE' || streamer.state === 'TEMPORAL_DISABLED') &&
    (streamer.consecutive_error_count > 0 ||
      !!streamer.disabled_until ||
      !!streamer.last_error) &&
    hasStrongRecoverySignal(activeDownload)
  );
}
