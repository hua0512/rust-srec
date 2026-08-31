import { RefreshCw, Trash2, XCircle } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';

import type { BatchDagAction } from '@/api/schemas';
import { Button } from '@/components/ui/button';
import { BatchActionBar } from '@/components/shared/batch-action-bar';
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from '@/components/ui/alert-dialog';
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from '@/components/ui/tooltip';

interface PipelineBatchActionBarProps {
  selectedCount: number;
  pageCount: number;
  allPageSelected: boolean;
  /** Selected pipelines that are PENDING or PROCESSING, so cancellable. */
  cancellableCount: number;
  /** Selected pipelines that are FAILED or CANCELLED, so retryable. */
  retryableCount: number;
  /** Selected pipelines that are still running and will be cancelled by a delete. */
  runningCount: number;
  isPending: boolean;
  onSelectPage: () => void;
  onClearSelection: () => void;
  onAction: (action: BatchDagAction) => void;
  onExit: () => void;
}

export function PipelineBatchActionBar({
  selectedCount,
  pageCount,
  allPageSelected,
  cancellableCount,
  retryableCount,
  runningCount,
  isPending,
  onSelectPage,
  onClearSelection,
  onAction,
  onExit,
}: PipelineBatchActionBarProps) {
  const { i18n } = useLingui();

  // Cancel only applies to non-terminal pipelines and retry only to terminal
  // failed/cancelled ones, so a mixed selection would otherwise send IDs the
  // backend is guaranteed to reject. The counts show how much of the selection
  // each action will actually touch.
  const cancelDisabled = cancellableCount === 0 || isPending;
  const retryDisabled = retryableCount === 0 || isPending;
  const deleteDisabled = selectedCount === 0 || isPending;

  return (
    <BatchActionBar
      selectedCount={selectedCount}
      pageCount={pageCount}
      allPageSelected={allPageSelected}
      isPending={isPending}
      onSelectPage={onSelectPage}
      onClearSelection={onClearSelection}
      onExit={onExit}
    >
      <Tooltip>
        <TooltipTrigger asChild>
          <Button
            variant="ghost"
            size="sm"
            disabled={retryDisabled}
            aria-label={i18n._(msg`Retry selected`)}
            onClick={() => onAction({ type: 'retry' })}
            className="rounded-full text-blue-600 hover:bg-blue-500/10 hover:text-blue-700 dark:text-blue-400"
          >
            <RefreshCw />
            <span className="hidden sm:inline">
              <Trans>Retry</Trans>
            </span>
            {retryableCount > 0 && retryableCount < selectedCount && (
              <span className="tabular-nums opacity-70">{retryableCount}</span>
            )}
          </Button>
        </TooltipTrigger>
        <TooltipContent side="top">
          {retryableCount === 0 ? (
            <Trans>No failed or cancelled pipelines selected</Trans>
          ) : (
            <Trans>Retry {retryableCount} failed or cancelled pipelines</Trans>
          )}
        </TooltipContent>
      </Tooltip>

      <Tooltip>
        <TooltipTrigger asChild>
          <Button
            variant="ghost"
            size="sm"
            disabled={cancelDisabled}
            aria-label={i18n._(msg`Cancel selected`)}
            onClick={() => onAction({ type: 'cancel' })}
            className="rounded-full text-amber-600 hover:bg-amber-500/10 hover:text-amber-700 dark:text-amber-400"
          >
            <XCircle />
            <span className="hidden sm:inline">
              <Trans>Cancel</Trans>
            </span>
            {cancellableCount > 0 && cancellableCount < selectedCount && (
              <span className="tabular-nums opacity-70">
                {cancellableCount}
              </span>
            )}
          </Button>
        </TooltipTrigger>
        <TooltipContent side="top">
          {cancellableCount === 0 ? (
            <Trans>No running pipelines selected</Trans>
          ) : (
            <Trans>Cancel {cancellableCount} running pipelines</Trans>
          )}
        </TooltipContent>
      </Tooltip>

      <AlertDialog>
        <Tooltip>
          <TooltipTrigger asChild>
            <AlertDialogTrigger asChild>
              <Button
                variant="ghost"
                size="icon-sm"
                disabled={deleteDisabled}
                className="rounded-full text-destructive hover:bg-destructive/10 hover:text-destructive"
                aria-label={i18n._(msg`Delete selected`)}
              >
                <Trash2 />
              </Button>
            </AlertDialogTrigger>
          </TooltipTrigger>
          <TooltipContent side="top">
            <Trans>Delete selected</Trans>
          </TooltipContent>
        </Tooltip>
        <AlertDialogContent className="rounded-2xl">
          <AlertDialogHeader>
            <AlertDialogTitle>
              <Trans>Delete selected pipelines?</Trans>
            </AlertDialogTitle>
            <AlertDialogDescription>
              <Trans>
                This will permanently delete {selectedCount} pipelines with
                their jobs and logs. This action cannot be undone.
              </Trans>{' '}
              {runningCount > 0 && (
                <Trans>
                  {runningCount} of them are still running and will be cancelled
                  first.
                </Trans>
              )}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel className="rounded-full">
              <Trans>Cancel</Trans>
            </AlertDialogCancel>
            <AlertDialogAction
              onClick={() => onAction({ type: 'delete' })}
              className="rounded-full bg-destructive text-white hover:bg-destructive/90"
            >
              <Trans>Delete</Trans>
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </BatchActionBar>
  );
}
