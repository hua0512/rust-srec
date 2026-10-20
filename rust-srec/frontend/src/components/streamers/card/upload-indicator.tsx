import { CloudUpload } from 'lucide-react';
import { msg, plural, t } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';

import { JobProgressDetails } from '@/components/job-progress-details';
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from '@/components/ui/tooltip';
import { describeJobProgress } from '@/lib/job-progress';
import {
  activeUploadsLabel,
  uploaderLabel,
  uploadPercent,
} from '@/lib/upload-format';
import { useShallow } from 'zustand/react/shallow';
import { useUploadStore, type UploadView } from '@/store/uploads';

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
      <TooltipContent className="max-w-xs space-y-1.5">
        <div className="text-xs font-medium">
          {activeUploadsLabel(uploads.length, i18n)}
        </div>
        {uploads.map((upload) => (
          <UploadTooltipRow key={upload.jobId} upload={upload} />
        ))}
      </TooltipContent>
    </Tooltip>
  );
}

function UploadTooltipRow({ upload }: { upload: UploadView }) {
  const { i18n } = useLingui();
  // Keeps each upload's speed, which the header's rows leave out: the card
  // shows no total speed anywhere else.
  const { percent, details } = describeJobProgress(upload, i18n);
  // Before the first progress line the uploader is still setting up:
  // checking the destination, hashing, or waiting on the remote.
  if (percent == null && !upload.phase) {
    details.push(i18n._(msg`Preparing…`));
  }

  return (
    <div className="space-y-0.5 text-xs">
      <div className="flex items-center justify-between gap-4">
        <span className="min-w-0 truncate opacity-70">
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
          <span className="shrink-0 font-mono font-semibold">
            {percent.toFixed(1)}%
          </span>
        )}
      </div>
      {details.length > 0 && (
        <JobProgressDetails details={details} className="opacity-60" />
      )}
    </div>
  );
}
