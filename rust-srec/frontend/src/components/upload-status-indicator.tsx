import { useState, type ReactNode } from 'react';
import { Link } from '@tanstack/react-router';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { CloudAlert, CloudUpload, Square, X } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { msg, plural, t } from '@lingui/core/macro';
import type { I18n } from '@lingui/core';
import { useLingui } from '@lingui/react';
import { useShallow } from 'zustand/react/shallow';
import { toast } from 'sonner';

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog';
import { Avatar, AvatarFallback, AvatarImage } from '@/components/ui/avatar';
import { Button, buttonVariants } from '@/components/ui/button';
import { NotificationBadge } from '@/components/ui/notification-badge';
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from '@/components/ui/popover';
import { Progress } from '@/components/ui/progress';
import { formatLocalizedDuration } from '@/lib/date-utils';
import { formatSpeed } from '@/lib/format';
import {
  activeUploadsLabel,
  formatUploadBytes,
  uploaderLabel,
  uploadPercent,
} from '@/lib/upload-format';
import { usePresence } from '@/hooks/use-presence';
import {
  HEADER_BADGE_DOT,
  HEADER_POPOVER,
  HEADER_TRIGGER_EXIT_MS,
  headerTriggerClass,
} from '@/components/header-indicator';
import { cn, getProxiedUrl } from '@/lib/utils';
import { cancelActivePipelineJob } from '@/server/functions/pipeline';
import {
  useUploadStore,
  type FailedUploadView,
  type UploadView,
} from '@/store/uploads';

export interface UploadSummary {
  // Undefined until at least one upload reports progress.
  percent?: number;
  speedBytesPerSec?: number;
  // The longest remaining time: when every upload is expected to be done.
  etaSecs?: number;
}

/**
 * Overall progress across uploads. Weighted by bytes when any upload
 * reports sizes, so a small file finishing doesn't make a large batch
 * look nearly done; uploads that only report a percent are averaged in
 * when no sizes are known at all.
 */
export function summarizeUploads(uploads: UploadView[]): UploadSummary {
  let bytesDone = 0n;
  let bytesTotal = 0n;
  let percentSum = 0;
  let percentCount = 0;
  let speed: number | undefined;
  let eta: number | undefined;

  for (const upload of uploads) {
    if (
      upload.bytesDone != null &&
      upload.bytesTotal != null &&
      upload.bytesTotal > 0n
    ) {
      bytesDone += upload.bytesDone;
      bytesTotal += upload.bytesTotal;
    }
    if (upload.percent != null) {
      percentSum += Math.min(upload.percent, 100);
      percentCount += 1;
    }
    if (upload.speedBytesPerSec != null) {
      speed = (speed ?? 0) + upload.speedBytesPerSec;
    }
    if (upload.etaSecs != null && upload.etaSecs > 0) {
      eta = Math.max(eta ?? 0, upload.etaSecs);
    }
  }

  let percent: number | undefined;
  if (bytesTotal > 0n) {
    percent = Math.min((Number(bytesDone) / Number(bytesTotal)) * 100, 100);
  } else if (percentCount > 0) {
    percent = percentSum / percentCount;
  }
  return { percent, speedBytesPerSec: speed, etaSecs: eta };
}

const RING_RADIUS = 15;
const RING_CIRCUMFERENCE = 2 * Math.PI * RING_RADIUS;

/**
 * Header entry for upload jobs across every streamer: running, queued, and
 * failed during this session. Uploads usually run after a stream has ended,
 * when the streamer's card is no longer on the dashboard, so this is the one
 * place they stay visible. Renders nothing while there is nothing to report.
 */
export function UploadStatusIndicator() {
  const { i18n } = useLingui();
  const [open, setOpen] = useState(false);
  const [cancelTarget, setCancelTarget] = useState<UploadView>();
  const uploads = useUploadStore(
    useShallow((state) => state.getActiveUploads()),
  );
  const failed = useUploadStore(
    useShallow((state) => state.getFailedUploads()),
  );
  const pending = useUploadStore((state) => state.pendingCount);
  const dismissFailed = useUploadStore((state) => state.dismissFailed);

  const hasContent = uploads.length > 0 || failed.length > 0 || pending > 0;
  // Keeps the trigger mounted for its exit animation after the last upload
  // leaves.
  const { mounted, exiting } = usePresence(hasContent, HEADER_TRIGGER_EXIT_MS);

  if (!mounted) return null;

  const summary = summarizeUploads(uploads);
  const activeLabel = activeUploadsLabel(uploads.length, i18n);
  const failedLabel = t(
    i18n,
  )`${plural(failed.length, { one: '# failed upload', other: '# failed uploads' })}`;
  const queuedLabel = t(
    i18n,
  )`${plural(pending, { one: '# queued upload', other: '# queued uploads' })}`;
  const queuedShortLabel = t(i18n)`${pending} queued`;
  const percentLabel =
    summary.percent != null ? `${summary.percent.toFixed(0)}%` : undefined;
  const eta =
    summary.etaSecs != null
      ? formatLocalizedDuration(summary.etaSecs, i18n.locale)
      : undefined;

  const title =
    uploads.length > 0
      ? activeLabel
      : failed.length > 0
        ? failedLabel
        : queuedLabel;
  const triggerLabel = [
    uploads.length > 0 && activeLabel,
    percentLabel,
    pending > 0 && queuedLabel,
    failed.length > 0 && failedLabel,
  ]
    .filter(Boolean)
    .join(', ');

  const tone =
    uploads.length > 0 ? 'active' : failed.length > 0 ? 'failed' : 'queued';
  const badge =
    failed.length > 0
      ? { label: '!', className: 'bg-red-500' }
      : uploads.length > 1
        ? { label: String(uploads.length), className: 'bg-sky-500' }
        : undefined;

  return (
    <>
      <Popover open={open && hasContent} onOpenChange={setOpen}>
        <PopoverTrigger
          aria-label={triggerLabel || title}
          className={cn(
            headerTriggerClass(exiting),
            tone === 'active' &&
              'text-sky-600 hover:text-sky-600 dark:text-sky-400 dark:hover:text-sky-400',
            tone === 'failed' &&
              'text-red-600 hover:text-red-600 dark:text-red-400 dark:hover:text-red-400',
            tone === 'queued' && 'text-muted-foreground',
          )}
        >
          <svg
            viewBox="0 0 36 36"
            className="absolute inset-0 size-full -rotate-90"
            aria-hidden="true"
          >
            <circle
              cx="18"
              cy="18"
              r={RING_RADIUS}
              fill="none"
              strokeWidth="2"
              className={
                tone === 'failed' ? 'stroke-red-500/25' : 'stroke-sky-500/20'
              }
            />
            {summary.percent != null && (
              <circle
                cx="18"
                cy="18"
                r={RING_RADIUS}
                fill="none"
                strokeWidth="2"
                strokeLinecap="round"
                strokeDasharray={RING_CIRCUMFERENCE}
                strokeDashoffset={
                  RING_CIRCUMFERENCE * (1 - summary.percent / 100)
                }
                className="stroke-sky-500 transition-[stroke-dashoffset] duration-500 ease-[cubic-bezier(0.22,1,0.36,1)] motion-reduce:transition-none"
              />
            )}
          </svg>
          {tone === 'failed' ? (
            <CloudAlert className="size-4" />
          ) : (
            <CloudUpload className="size-4" />
          )}
          <NotificationBadge
            open={badge != null}
            entering={badge != null}
            className="-top-0.5 -right-0.5"
            dotClassName={cn(
              HEADER_BADGE_DOT,
              badge?.className ?? 'bg-sky-500',
            )}
          >
            {badge?.label}
          </NotificationBadge>
        </PopoverTrigger>
        <PopoverContent align="end" sideOffset={8} className={HEADER_POPOVER}>
          <div className="space-y-2 px-4 py-3">
            <div className="flex items-baseline justify-between gap-3">
              <span className="text-sm font-semibold">
                {title}
                {uploads.length > 0 && pending > 0 && (
                  <span className="font-normal text-muted-foreground">
                    {' · '}
                    {queuedShortLabel}
                  </span>
                )}
              </span>
              {summary.speedBytesPerSec != null && (
                <span className="text-xs text-muted-foreground tabular-nums">
                  {formatSpeed(summary.speedBytesPerSec)}
                </span>
              )}
            </div>
            {summary.percent != null && (
              <div className="flex items-center gap-2">
                <Progress
                  value={summary.percent}
                  className="h-1.5 flex-1 bg-sky-500/15"
                  indicatorClassName="bg-sky-500"
                />
                <span className="text-xs font-semibold tabular-nums">
                  {percentLabel}
                </span>
                {eta != null && (
                  <span className="text-xs text-muted-foreground">
                    <Trans>~{eta} left</Trans>
                  </span>
                )}
              </div>
            )}
          </div>
          <div className="max-h-80 overflow-y-auto border-t border-border/60 py-1">
            {failed.length > 0 && (
              <ul aria-label={failedLabel}>
                {failed.map((upload) => (
                  <li key={upload.jobId}>
                    <FailedUploadRow
                      upload={upload}
                      onNavigate={() => setOpen(false)}
                      onDismiss={() => dismissFailed(upload.jobId)}
                    />
                  </li>
                ))}
              </ul>
            )}
            {uploads.length > 0 && (
              <ul aria-label={activeLabel}>
                {uploads.map((upload) => (
                  <li key={upload.jobId}>
                    <ActiveUploadRow
                      upload={upload}
                      onNavigate={() => setOpen(false)}
                      onCancel={() => {
                        setOpen(false);
                        setCancelTarget(upload);
                      }}
                    />
                  </li>
                ))}
              </ul>
            )}
            {uploads.length === 0 && failed.length === 0 && (
              <p className="px-4 py-2 text-xs text-muted-foreground">
                <Trans>Waiting for a free upload worker.</Trans>
              </p>
            )}
          </div>
          <div className="border-t border-border/60 px-4 py-2 text-right">
            <Link
              to="/pipeline/jobs"
              search={{ status: 'PROCESSING' }}
              onClick={() => setOpen(false)}
              className="text-xs text-muted-foreground hover:text-foreground"
            >
              <Trans>View processing jobs</Trans>
            </Link>
          </div>
        </PopoverContent>
      </Popover>
      {/* Outside the popover, which closes (unmounting its content) as the
          dialog takes focus. */}
      <CancelUploadDialog
        upload={cancelTarget}
        onClose={() => setCancelTarget(undefined)}
      />
    </>
  );
}

function displayName(upload: UploadView, i18n: I18n) {
  return upload.streamerName || upload.streamerId || i18n._(msg`Manual job`);
}

function ActiveUploadRow({
  upload,
  onNavigate,
  onCancel,
}: {
  upload: UploadView;
  onNavigate: () => void;
  onCancel: () => void;
}) {
  const { i18n } = useLingui();
  const name = displayName(upload, i18n);
  const percent = uploadPercent(upload);
  const eta =
    upload.etaSecs != null && upload.etaSecs > 0
      ? formatLocalizedDuration(upload.etaSecs, i18n.locale)
      : undefined;
  const bytes = formatUploadBytes(upload);
  const filesDone = upload.progressFilesDone;
  const filesTotal = upload.progressFilesTotal;
  const details = [uploaderLabel(upload, i18n)];
  if (filesDone != null && filesTotal != null && filesTotal > 1) {
    details.push(i18n._(msg`${filesDone}/${filesTotal} files`));
  }
  if (bytes != null) details.push(bytes);
  if (eta != null) details.push(i18n._(msg`${eta} left`));
  // Before the first progress line the uploader is still setting up:
  // checking the destination, hashing, or waiting on the remote.
  if (percent == null) details.push(i18n._(msg`Preparing…`));

  return (
    <div className="group/row flex items-center hover:bg-accent/60">
      <Link
        to="/pipeline/jobs/$jobId"
        params={{ jobId: upload.jobId }}
        onClick={onNavigate}
        className="flex min-w-0 flex-1 items-center gap-3 py-2 pl-4"
      >
        <StreamerAvatar upload={upload} name={name} />
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-3">
            <span className="w-24 shrink-0 truncate text-sm font-medium">
              {name}
            </span>
            <Progress
              value={percent ?? 0}
              className="h-1 flex-1 bg-sky-500/15"
              indicatorClassName="bg-sky-500"
            />
            <span className="w-9 shrink-0 text-right text-xs font-semibold tabular-nums">
              {percent != null ? `${percent.toFixed(0)}%` : '—'}
            </span>
          </div>
          <div className="mt-0.5 truncate text-[11px] text-muted-foreground">
            {details.join(' · ')}
          </div>
        </div>
      </Link>
      <RowAction label={i18n._(msg`Cancel upload`)} onClick={onCancel}>
        <Square className="size-2.5 fill-current" />
      </RowAction>
    </div>
  );
}

function FailedUploadRow({
  upload,
  onNavigate,
  onDismiss,
}: {
  upload: FailedUploadView;
  onNavigate: () => void;
  onDismiss: () => void;
}) {
  const { i18n } = useLingui();
  const name = displayName(upload, i18n);
  const outcome =
    upload.filesFailed > 0
      ? t(
          i18n,
        )`${plural(upload.filesFailed, { one: '# file failed', other: '# files failed' })}`
      : i18n._(msg`Failed`);

  return (
    <div className="group/row flex items-center hover:bg-accent/60">
      <Link
        to="/pipeline/jobs/$jobId"
        params={{ jobId: upload.jobId }}
        onClick={onNavigate}
        className="flex min-w-0 flex-1 items-center gap-3 py-2 pl-4"
      >
        <StreamerAvatar upload={upload} name={name} failed />
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <span className="min-w-0 truncate text-sm font-medium">{name}</span>
            <span className="ml-auto shrink-0 text-xs font-medium text-red-600 dark:text-red-400">
              {outcome}
            </span>
          </div>
          {upload.error && (
            <div
              title={upload.error}
              className="mt-0.5 truncate text-[11px] text-muted-foreground"
            >
              {upload.error}
            </div>
          )}
        </div>
      </Link>
      <RowAction label={i18n._(msg`Dismiss`)} onClick={onDismiss}>
        <X className="size-3.5" />
      </RowAction>
    </div>
  );
}

function StreamerAvatar({
  upload,
  name,
  failed = false,
}: {
  upload: UploadView;
  name: string;
  failed?: boolean;
}) {
  return (
    <span className="relative shrink-0">
      <Avatar className="size-7 border border-border/60">
        <AvatarImage
          src={getProxiedUrl(upload.streamerAvatar)}
          alt=""
          className="object-cover"
        />
        <AvatarFallback className="text-[10px] font-medium text-muted-foreground">
          {name.substring(0, 2).toUpperCase()}
        </AvatarFallback>
      </Avatar>
      {failed && (
        <span
          aria-hidden
          className="absolute -right-0.5 -bottom-0.5 size-2.5 rounded-full bg-red-500 ring-2 ring-popover"
        />
      )}
    </span>
  );
}

/** Revealed on row hover or keyboard focus; always shown without a fine pointer. */
function RowAction({
  label,
  onClick,
  children,
}: {
  label: string;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <Button
      variant="ghost"
      size="icon"
      aria-label={label}
      title={label}
      onClick={onClick}
      className="mx-2 size-6 shrink-0 rounded-full text-muted-foreground transition-opacity duration-150 focus-visible:opacity-100 pointer-fine:opacity-0 pointer-fine:group-hover/row:opacity-100"
    >
      {children}
    </Button>
  );
}

function CancelUploadDialog({
  upload,
  onClose,
}: {
  upload: UploadView | undefined;
  onClose: () => void;
}) {
  const { i18n } = useLingui();
  const queryClient = useQueryClient();
  const cancelMutation = useMutation({
    mutationFn: (id: string) => cancelActivePipelineJob({ data: id }),
    onSuccess: (_, id) => {
      toast.success(i18n._(msg`Upload cancelled`));
      void queryClient.invalidateQueries({ queryKey: ['pipeline', 'job', id] });
    },
    onError: () => toast.error(i18n._(msg`Failed to cancel upload`)),
  });
  const name = upload ? displayName(upload, i18n) : '';

  return (
    <AlertDialog
      open={upload != null}
      onOpenChange={(next) => {
        if (!next) onClose();
      }}
    >
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>
            <Trans>Cancel this upload?</Trans>
          </AlertDialogTitle>
          <AlertDialogDescription>
            <Trans>
              The upload for {name} stops now. Files that already finished stay
              at the destination, and you can retry the job later.
            </Trans>
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>
            <Trans>Keep uploading</Trans>
          </AlertDialogCancel>
          <AlertDialogAction
            className={buttonVariants({ variant: 'destructive' })}
            onClick={() => {
              if (upload) cancelMutation.mutate(upload.jobId);
            }}
          >
            <Trans>Cancel upload</Trans>
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
