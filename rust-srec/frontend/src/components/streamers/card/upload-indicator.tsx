import { CloudUpload } from 'lucide-react';
import { plural, t } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';

import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from '@/components/ui/tooltip';
import { formatSpeed } from '@/lib/format';
import {
  activeUploadsLabel,
  formatUploadBytes,
  uploaderLabel,
  uploadPercent,
} from '@/lib/upload-format';
import { useShallow } from 'zustand/react/shallow';
import { useUploadStore } from '@/store/uploads';

/**
 * Pulsing cloud badge on the streamer card while upload job(s) for this
 * streamer are in flight, with real-time percent from the WS upload events.
 * Presence-only in the card layout — the download ProgressIndicator keeps
 * the card's progress-bar real estate.
 */
export function UploadIndicator({ streamerId }: { streamerId: string }) {
  const { i18n } = useLingui();
  const uploads = useUploadStore(
    useShallow((state) => state.getActiveUploadsByStreamer(streamerId)),
  );

  if (uploads.length === 0) return null;

  const singlePercent =
    uploads.length === 1 ? uploadPercent(uploads[0]) : undefined;

  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <div className="flex h-6 items-center gap-1 rounded-full border border-sky-500/20 bg-sky-500/10 px-2 text-sky-600 dark:text-sky-400">
          <CloudUpload className="h-3 w-3 animate-pulse" />
          {singlePercent != null ? (
            <span className="font-mono text-[10px] font-semibold tabular-nums">
              {singlePercent.toFixed(0)}%
            </span>
          ) : uploads.length > 1 ? (
            <span className="font-mono text-[10px] font-semibold tabular-nums">
              {uploads.length}
            </span>
          ) : null}
        </div>
      </TooltipTrigger>
      <TooltipContent className="space-y-1.5">
        <div className="text-xs font-medium">
          {activeUploadsLabel(uploads.length, i18n)}
        </div>
        {uploads.map((upload) => {
          const percent = uploadPercent(upload);
          const bytes = formatUploadBytes(upload);
          return (
            <div key={upload.jobId} className="text-xs space-y-0.5">
              <div className="flex items-center justify-between gap-4">
                <span className="opacity-70">
                  {uploaderLabel(upload, i18n)}
                  {upload.filesTotal > 0 && (
                    <>
                      {' · '}
                      {t(
                        i18n,
                      )`${plural(upload.filesTotal, { one: '# file', other: '# files' })}`}
                    </>
                  )}
                </span>
                {percent != null && (
                  <span className="font-mono font-semibold">
                    {percent.toFixed(1)}%
                  </span>
                )}
              </div>
              {(bytes != null || upload.speedBytesPerSec != null) && (
                <div className="flex items-center justify-between gap-4 font-mono opacity-60">
                  <span>{bytes}</span>
                  {upload.speedBytesPerSec != null && (
                    <span>{formatSpeed(upload.speedBytesPerSec)}</span>
                  )}
                </div>
              )}
            </div>
          );
        })}
      </TooltipContent>
    </Tooltip>
  );
}
