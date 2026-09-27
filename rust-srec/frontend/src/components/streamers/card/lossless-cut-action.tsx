import { useEffect, useRef, useState, type ReactNode } from 'react';
import { useMutation } from '@tanstack/react-query';
import { Trans, useLingui } from '@lingui/react/macro';
import { Loader2, Scissors } from 'lucide-react';
import { toast } from 'sonner';
import { DropdownMenuItem } from '@/components/ui/dropdown-menu';
import { useDownloadStore } from '@/store/downloads';
import { requestLosslessCut } from '@/server/functions/downloads';

export function LosslessCutAction({
  downloadId,
  children,
}: {
  downloadId: string;
  children: (action: ReactNode) => ReactNode;
}) {
  const { t } = useLingui();
  const split = useDownloadStore((state) =>
    state.manualSplitById.get(downloadId),
  );
  const connected = useDownloadStore(
    (state) => state.connectionStatus === 'connected',
  );
  const [requested, setRequested] = useState<bigint>();
  const reported = useRef<bigint | undefined>(undefined);
  const mutation = useMutation({
    mutationFn: () => requestLosslessCut({ data: downloadId }),
    onSuccess: (response) => setRequested(BigInt(response.request_id)),
    onError: () =>
      toast.error(
        t`Could not request lossless cutting. Refresh the recording status and try again.`,
      ),
  });

  useEffect(() => {
    if (
      requested === undefined ||
      !split ||
      split.requestId !== requested ||
      reported.current === requested
    )
      return;
    if (split.status === 'completed') {
      reported.current = requested;
      toast.success(t`Recording split successfully.`);
    } else if (split.status === 'expired') {
      reported.current = requested;
      toast.error(t`No safe cut boundary was found. Recording continues.`);
    } else if (split.status === 'cancelled') {
      reported.current = requested;
      toast.error(t`The recording ended before the cut completed.`);
    } else if (split.status === 'failed') {
      reported.current = requested;
      toast.error(t`The cut could not be completed.`);
    }
  }, [requested, split, t]);

  const pending =
    mutation.isPending ||
    split?.status === 'pending' ||
    split?.status === 'finalizing' ||
    (requested !== undefined && (!split || split.requestId < requested));
  const explanation = !connected
    ? t`Reconnect to request a cut.`
    : !split?.supported
      ? t`This recording mode does not support lossless cutting.`
      : t`Finish this file and continue recording at the next safe boundary.`;

  return children(
    <div title={explanation}>
      <DropdownMenuItem
        disabled={!connected || !split?.supported || pending}
        onSelect={() => mutation.mutate()}
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
    </div>,
  );
}
