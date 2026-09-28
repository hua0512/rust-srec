import { useEffect, useRef, useState } from 'react';
import { useMutation } from '@tanstack/react-query';
import { msg } from '@lingui/core/macro';
import { Trans, useLingui } from '@lingui/react/macro';
import type { I18n } from '@lingui/core';
import { Loader2, Scissors } from 'lucide-react';
import { toast } from 'sonner';
import { Badge } from '@/components/ui/badge';
import { DropdownMenuItem } from '@/components/ui/dropdown-menu';
import { useDownloadStore } from '@/store/downloads';
import { requestLosslessCut } from '@/server/functions/downloads';

function expiryMessage(i18n: I18n, reason: string | undefined): string {
  switch (reason) {
    case 'no_keyframe':
      return i18n._(
        msg`No keyframe arrived in time, so no safe cut boundary was found. Recording continues.`,
      );
    case 'no_independent_segment':
      return i18n._(
        msg`The stream's segments are not marked as independently playable, so no safe cut boundary was found. Recording continues.`,
      );
    case 'finalization_backlog':
      return i18n._(
        msg`Earlier files are still being finalized, so the cut was not made. Recording continues.`,
      );
    case 'no_media':
      return i18n._(
        msg`No new media arrived in time, so no safe cut boundary was found. Recording continues.`,
      );
    default:
      return i18n._(msg`No safe cut boundary was found. Recording continues.`);
  }
}

/**
 * Lossless-cut state for one card. It lives with the card rather than inside
 * the dropdown content, so a result still reports after the menu closes, and
 * it is keyed by `downloadId` internally so the menu root never remounts when
 * a recording starts, ends, or is replaced.
 *
 * Returns the menu item to render, or `null` when there is no active download
 * or the recording does not support lossless cutting.
 */
export function useLosslessCutItem(downloadId: string | undefined) {
  const { i18n, t } = useLingui();
  const split = useDownloadStore((state) =>
    downloadId === undefined
      ? undefined
      : state.manualSplitById.get(downloadId),
  );
  const connected = useDownloadStore(
    (state) => state.connectionStatus === 'connected',
  );
  const [requested, setRequested] = useState<{
    downloadId: string;
    requestId: bigint;
  }>();
  const reported = useRef<{ downloadId: string; requestId: bigint }>(undefined);
  const mutation = useMutation({
    mutationFn: (id: string) => requestLosslessCut({ data: id }),
    onSuccess: (response, id) =>
      setRequested({ downloadId: id, requestId: BigInt(response.request_id) }),
    onError: (error) =>
      toast.error(
        error.message ||
          t`Could not request lossless cutting. Refresh the recording status and try again.`,
      ),
  });
  const requestId =
    requested?.downloadId === downloadId ? requested?.requestId : undefined;

  useEffect(() => {
    if (
      downloadId === undefined ||
      requestId === undefined ||
      !split ||
      split.requestId !== requestId ||
      (reported.current?.downloadId === downloadId &&
        reported.current.requestId === requestId)
    )
      return;
    const report = () => {
      reported.current = { downloadId, requestId };
    };
    if (split.status === 'completed') {
      report();
      toast.success(t`Recording split successfully.`);
    } else if (split.status === 'expired') {
      report();
      toast.error(expiryMessage(i18n, split.expiryReason));
    } else if (split.status === 'cancelled') {
      report();
      toast.error(t`The recording ended before the cut completed.`);
    } else if (split.status === 'failed') {
      report();
      toast.error(t`The cut could not be completed.`);
    }
  }, [downloadId, requestId, split, i18n, t]);

  // Unsupported recordings never offer the action; the item is disabled only
  // for temporary reasons, which the second line explains on every device.
  // Only the label and icon are dimmed so that explanation stays readable.
  if (downloadId === undefined || !split?.supported) return null;

  const pending =
    (mutation.isPending && mutation.variables === downloadId) ||
    split.status === 'pending' ||
    split.status === 'finalizing' ||
    (requestId !== undefined && split.requestId < requestId);
  const disabledReason = !connected
    ? t`Reconnect to request a cut.`
    : pending
      ? t`The file is split at the next safe boundary.`
      : undefined;

  return (
    <DropdownMenuItem
      disabled={disabledReason !== undefined}
      onSelect={() => mutation.mutate(downloadId)}
      aria-description={
        disabledReason ??
        t`Finish this file and continue recording at the next safe boundary.`
      }
      className="cursor-pointer group items-start data-[disabled]:opacity-100"
    >
      {pending ? (
        <Loader2 className="mr-2 mt-0.5 h-4 w-4 animate-spin text-cyan-500 opacity-50 dark:text-cyan-400" />
      ) : (
        <Scissors className="mr-2 mt-0.5 h-4 w-4 text-cyan-500 group-hover:text-cyan-600 group-data-[disabled]:opacity-50 dark:text-cyan-400 dark:group-hover:text-cyan-300" />
      )}
      <span className="flex flex-col gap-0.5">
        <span
          aria-live="polite"
          className="text-cyan-600 group-hover:text-cyan-700 group-data-[disabled]:opacity-50 dark:text-cyan-400 dark:group-hover:text-cyan-300 transition-colors"
        >
          {split.status === 'finalizing' ? (
            <Trans>Finalizing recording file...</Trans>
          ) : pending ? (
            <Trans>Waiting for a safe cut...</Trans>
          ) : (
            <Trans>Split file now</Trans>
          )}
        </span>
        {disabledReason && (
          <span className="text-xs text-muted-foreground">
            {disabledReason}
          </span>
        )}
      </span>
    </DropdownMenuItem>
  );
}

/**
 * Card badge while this card's download has a cut in progress. Reads only the
 * split state, so progress updates re-render the badge alone.
 */
export function LosslessCutBadge({ downloadId }: { downloadId: string }) {
  const status = useDownloadStore(
    (state) => state.manualSplitById.get(downloadId)?.status,
  );
  if (status !== 'pending' && status !== 'finalizing') return null;
  return (
    <Badge
      variant="outline"
      className="gap-1 whitespace-nowrap border-cyan-500/30 bg-cyan-500/10 text-cyan-700 dark:text-cyan-300"
    >
      <Loader2 className="h-3 w-3 animate-spin" aria-hidden />
      {status === 'pending' ? (
        <Trans>Splitting...</Trans>
      ) : (
        <Trans>Finalizing...</Trans>
      )}
    </Badge>
  );
}
