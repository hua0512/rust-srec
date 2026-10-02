import type { I18n } from '@lingui/core';
import { msg, plural, t } from '@lingui/core/macro';

import { formatBytes } from '@/lib/format';
import type { UploadView } from '@/store/uploads';

// Shared by every view of live uploads (the header's upload status and the
// streamer card's indicator) so they show the same numbers.

/** The reported percent, capped at 100; undefined before any progress. */
export function uploadPercent(upload: Pick<UploadView, 'percent'>) {
  return upload.percent != null ? Math.min(upload.percent, 100) : undefined;
}

/** "1.2 GB / 4.0 GB", or only the bytes sent while the total is unknown. */
export function formatUploadBytes(
  upload: Pick<UploadView, 'bytesDone' | 'bytesTotal'>,
) {
  if (upload.bytesDone == null) return undefined;
  if (upload.bytesTotal == null) return formatBytes(upload.bytesDone);
  return `${formatBytes(upload.bytesDone)} / ${formatBytes(upload.bytesTotal)}`;
}

/** The uploader's name ("rclone"), or a generic label for an unknown one. */
export function uploaderLabel(
  upload: Pick<UploadView, 'uploader'>,
  i18n: I18n,
) {
  return upload.uploader || i18n._(msg`upload`);
}

export function activeUploadsLabel(count: number, i18n: I18n) {
  return t(
    i18n,
  )`${plural(count, { one: '# active upload', other: '# active uploads' })}`;
}
