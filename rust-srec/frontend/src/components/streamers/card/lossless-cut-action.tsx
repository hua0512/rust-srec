import { useEffect, useRef, useState } from 'react';
import { useMutation } from '@tanstack/react-query';
import { msg } from '@lingui/core/macro';
import { Trans, useLingui } from '@lingui/react/macro';
import type { I18n } from '@lingui/core';
import { Loader2, Scissors } from 'lucide-react';
import { toast } from 'sonner';
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
 * Returns the menu item to render, or `null` without an active download.
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

  if (downloadId === undefined) return null;

  const pending =
    (mutation.isPending && mutation.variables === downloadId) ||
    split?.status === 'pending' ||
    split?.status === 'finalizing' ||
    (requestId !== undefined && (!split || split.requestId < requestId));
  const explanation = !connected
    ? t`Reconnect to request a cut.`
    : !split?.supported
      ? t`This recording mode does not support lossless cutting.`
      : t`Finish this file and continue recording at the next safe boundary.`;

  return (
    <div title={explanation}>
      <DropdownMenuItem
        disabled={!connected || !split?.supported || pending}
        onSelect={() => mutation.mutate(downloadId)}
        aria-description={explanation}
        className="cursor-pointer"
      >
        {pending ? (
          <Loader2 className="mr-2 h-4 w-4 animate-spin" />
        ) : (
          <Scissors className="mr-2 h-4 w-4" />
        )}
        <span aria-live="polite">
          {split?.status === 'finalizing' ? (
            <Trans>Finalizing recording file...</Trans>
          ) : pending ? (
            <Trans>Waiting for a safe cut...</Trans>
          ) : (
            <Trans>Lossless cutting</Trans>
          )}
        </span>
      </DropdownMenuItem>
    </div>
  );
}
