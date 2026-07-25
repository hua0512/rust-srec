import { CloudUpload } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { plural } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';

import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from '@/components/ui/tooltip';
import { formatBytes, formatSpeed } from '@/lib/format';
import type { UploadView } from '@/store/uploads';

/**
 * Pulsing cloud badge on the streamer card while upload job(s) for this
 * streamer are in flight, with real-time percent from the WS upload events.
 * Presence-only in the card layout — the download ProgressIndicator keeps
 * the card's progress-bar real estate.
 */
export function UploadIndicator({ uploads }: { uploads: UploadView[] }) {
  const { i18n } = useLingui();

  if (uploads.length === 0) return null;

  // With several concurrent upload jobs, surface the one with the most
  // recent progress in the compact percent label.
  const latest = uploads.reduce((a, b) =>
    b.lastEventAtMs > a.lastEventAtMs ? b : a,
  );

  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <div className="flex items-center gap-1 rounded-full border border-sky-500/20 bg-sky-500/10 px-2 py-0.5 text-sky-600 dark:text-sky-400">
          <CloudUpload className="h-3 w-3 animate-pulse" />
          {latest.percent != null && (
            <span className="font-mono text-[10px] font-semibold tabular-nums">
              {Math.min(latest.percent, 100).toFixed(0)}%
            </span>
          )}
        </div>
      </TooltipTrigger>
      <TooltipContent className="space-y-1.5">
        <div className="text-xs font-medium">
          {i18n._(
            plural(uploads.length, {
              one: '# active upload',
              other: '# active uploads',
            }),
          )}
        </div>
        {uploads.map((upload) => (
          <div key={upload.jobId} className="text-xs space-y-0.5">
            <div className="flex items-center justify-between gap-4">
              <span className="opacity-70">
                {upload.uploader || <Trans>upload</Trans>}
                {upload.filesTotal > 0 && (
                  <>
                    {' · '}
                    {i18n._(
                      plural(upload.filesTotal, {
                        one: '# file',
                        other: '# files',
                      }),
                    )}
                  </>
                )}
              </span>
              {upload.percent != null && (
                <span className="font-mono font-semibold">
                  {Math.min(upload.percent, 100).toFixed(1)}%
                </span>
              )}
            </div>
            {(upload.bytesDone != null || upload.speedBytesPerSec != null) && (
              <div className="flex items-center justify-between gap-4 font-mono opacity-60">
                <span>
                  {upload.bytesDone != null && formatBytes(upload.bytesDone)}
                  {upload.bytesTotal != null &&
                    ` / ${formatBytes(upload.bytesTotal)}`}
                </span>
                {upload.speedBytesPerSec != null && (
                  <span>{formatSpeed(upload.speedBytesPerSec)}</span>
                )}
              </div>
            )}
          </div>
        ))}
      </TooltipContent>
    </Tooltip>
  );
}
