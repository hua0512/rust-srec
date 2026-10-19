import type { MessageDescriptor } from '@lingui/core';
import { msg } from '@lingui/core/macro';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';

import type {
  JobProgressKind,
  JobProgressSnapshot,
} from '@/api/schemas/session';
import { Progress } from '@/components/ui/progress';
import { formatBytes } from '@/lib/format';

const BYTES_LABELS: Record<JobProgressKind, MessageDescriptor> = {
  ffmpeg: msg`Processed`,
  rclone: msg`Uploaded`,
  baidupcs: msg`Uploaded`,
  compression: msg`Compressed`,
  copy: msg`Copied`,
};

/** One-based position of the file in progress, for jobs of two or more files. */
function filePosition({
  files_done: done,
  files_total: count,
}: JobProgressSnapshot): { index: number; count: number } | null {
  if (done == null || count == null || count < 2) return null;
  // Uploaders report every file done while the job is still finishing.
  return { index: Math.min(done + 1, count), count };
}

/**
 * Byte progress of a running job: the share done, bytes so far, and the file
 * being worked on in a batch. Renders nothing for snapshots without byte
 * totals, such as FFmpeg's time-based progress.
 */
export function JobProgressBytes({
  snapshot,
}: {
  snapshot: JobProgressSnapshot;
}) {
  const { i18n } = useLingui();
  const { bytes_done: done, bytes_total: total } = snapshot;
  if (done == null || total == null) return null;

  const percent =
    snapshot.percent ?? (total > 0 ? Math.min((done / total) * 100, 100) : 100);
  const position = filePosition(snapshot);

  return (
    <div className="col-span-2 p-4 rounded-xl bg-background/50 border border-border/50">
      <div className="flex items-baseline justify-between mb-2">
        <span className="text-muted-foreground text-xs font-medium uppercase tracking-wider">
          {i18n._(BYTES_LABELS[snapshot.kind])}
        </span>
        <span className="text-sm font-bold font-mono">
          {percent.toFixed(0)}%
        </span>
      </div>
      <Progress value={percent} />
      <div className="mt-2 flex justify-between gap-2 text-xs text-muted-foreground font-mono">
        <span>
          {formatBytes(done)} / {formatBytes(total)}
        </span>
        {position && (
          <span>
            <Trans>
              File {position.index} of {position.count}
            </Trans>
          </span>
        )}
      </div>
    </div>
  );
}
