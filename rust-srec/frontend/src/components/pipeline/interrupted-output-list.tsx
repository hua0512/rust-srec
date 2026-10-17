import { useState } from 'react';
import { Link } from '@tanstack/react-router';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { Trans } from '@lingui/react/macro';
import { msg, plural, t } from '@lingui/core/macro';
import type { I18n } from '@lingui/core';
import { useLingui } from '@lingui/react';
import { toast } from 'sonner';

import { invalidateInterruptedOutputs } from '@/api/interrupted-outputs';
import {
  MAX_BATCH_IDS,
  type BatchResponse,
  type InterruptedOutput,
} from '@/api/schemas';
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
import { Button, buttonVariants } from '@/components/ui/button';
import { formatBytes } from '@/lib/format';
import { formatRelativeTime } from '@/lib/date-utils';
import { cn } from '@/lib/utils';
import {
  batchDeleteInterruptedOutputs,
  deleteInterruptedOutput,
} from '@/server/functions/pipeline';

/** Splits a server path on either separator; the server may run on Windows. */
export function splitPath(path: string): { name: string; folder: string } {
  const index = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'));
  return index < 0
    ? { name: path, folder: '' }
    : { name: path.slice(index + 1), folder: path.slice(0, index) };
}

/** "N partial files from interrupted jobs (size)". */
export function interruptedOutputsLabel(
  count: number,
  totalBytes: number,
  i18n: I18n,
) {
  const files = t(
    i18n,
  )`${plural(count, { one: '# partial file from interrupted jobs', other: '# partial files from interrupted jobs' })}`;
  return totalBytes > 0 ? `${files} (${formatBytes(totalBytes)})` : files;
}

function errorDescription(error: unknown) {
  return error instanceof Error ? error.message : undefined;
}

/** Deletes every listed file, in as many batch requests as the endpoint needs. */
async function deleteAll(ids: string[]): Promise<BatchResponse> {
  const results: BatchResponse['results'] = [];
  for (let start = 0; start < ids.length; start += MAX_BATCH_IDS) {
    const response = await batchDeleteInterruptedOutputs({
      data: { ids: ids.slice(start, start + MAX_BATCH_IDS), keep_files: false },
    });
    results.push(...response.results);
  }
  const succeeded = results.filter((item) => item.success).length;
  return {
    requested: results.length,
    succeeded,
    failed: results.length - succeeded,
    results,
  };
}

/** Deletes or keeps entries, then refreshes every listing. */
function useSettleInterruptedOutputs() {
  const { i18n } = useLingui();
  const queryClient = useQueryClient();

  const one = useMutation({
    mutationFn: (input: { id: string; keepFile: boolean }) =>
      deleteInterruptedOutput({ data: input }),
    onSuccess: (_, { keepFile }) =>
      toast.success(
        keepFile
          ? i18n._(msg`File kept and removed from the list`)
          : i18n._(msg`Partial file deleted`),
      ),
    onError: (error) =>
      toast.error(i18n._(msg`Could not delete the partial file`), {
        description: errorDescription(error),
      }),
    onSettled: () => invalidateInterruptedOutputs(queryClient),
  });

  const all = useMutation({
    mutationFn: deleteAll,
    onSuccess: (result) => {
      if (result.failed === 0) {
        toast.success(
          t(
            i18n,
          )`${plural(result.succeeded, { one: 'Deleted # partial file', other: 'Deleted # partial files' })}`,
        );
      } else {
        toast.error(
          t(
            i18n,
          )`${plural(result.failed, { one: '# partial file could not be deleted', other: '# partial files could not be deleted' })}`,
          { description: result.results.find((item) => !item.success)?.error },
        );
      }
    },
    onError: (error) =>
      toast.error(i18n._(msg`Could not delete the partial files`), {
        description: errorDescription(error),
      }),
    onSettled: () => invalidateInterruptedOutputs(queryClient),
  });

  return { one, all };
}

/**
 * Rows for partial files with Delete and Keep actions. Delete removes the file;
 * Keep leaves it on disk and only drops it from the list.
 */
export function InterruptedOutputList({
  outputs,
  showJobLink = true,
  onNavigate,
  className,
}: {
  outputs: InterruptedOutput[];
  showJobLink?: boolean;
  onNavigate?: () => void;
  className?: string;
}) {
  const { i18n } = useLingui();
  const { one } = useSettleInterruptedOutputs();
  const busy = one.isPending ? one.variables?.id : undefined;

  return (
    <ul className={cn('divide-y divide-border/60', className)}>
      {outputs.map((output) => {
        const { name, folder } = splitPath(output.path);
        const size =
          output.size_bytes != null
            ? formatBytes(output.size_bytes)
            : output.exists == null
              ? i18n._(msg`Folder unavailable`)
              : undefined;
        return (
          <li key={output.id} className="px-4 py-2.5">
            <p className="truncate text-sm font-medium" title={output.path}>
              {name}
            </p>
            <p
              className="truncate text-[11px] text-muted-foreground"
              title={folder}
            >
              {folder}
            </p>
            <div className="mt-1.5 flex items-center gap-2 text-[11px] text-muted-foreground">
              {size && <span className="tabular-nums">{size}</span>}
              <span>
                {i18n._(
                  msg`Interrupted ${formatRelativeTime(output.interrupted_at, i18n.locale)}`,
                )}
              </span>
              {showJobLink && (
                <Link
                  to="/pipeline/jobs/$jobId"
                  params={{ jobId: output.job_id }}
                  onClick={onNavigate}
                  className="underline-offset-2 hover:underline"
                >
                  <Trans>View job</Trans>
                </Link>
              )}
              <span className="ml-auto flex gap-1">
                <Button
                  size="sm"
                  variant="ghost"
                  className="h-6 px-2 text-xs"
                  disabled={busy === output.id}
                  onClick={() => one.mutate({ id: output.id, keepFile: true })}
                >
                  <Trans>Keep</Trans>
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  className="h-6 px-2 text-xs text-destructive hover:text-destructive"
                  disabled={busy === output.id}
                  onClick={() => one.mutate({ id: output.id, keepFile: false })}
                >
                  <Trans>Delete</Trans>
                </Button>
              </span>
            </div>
          </li>
        );
      })}
    </ul>
  );
}

/** A "Delete all" button that confirms before deleting every listed file. */
export function DeleteAllInterruptedOutputs({
  outputs,
  totalBytes,
}: {
  outputs: InterruptedOutput[];
  totalBytes: number;
}) {
  const { all } = useSettleInterruptedOutputs();
  const [confirming, setConfirming] = useState(false);
  const count = outputs.length;
  const size = formatBytes(totalBytes);

  return (
    <>
      <Button
        size="sm"
        variant="outline"
        className="h-7 text-xs text-destructive hover:text-destructive"
        disabled={all.isPending || count === 0}
        onClick={() => setConfirming(true)}
      >
        <Trans>Delete all</Trans>
      </Button>
      <AlertDialog open={confirming} onOpenChange={setConfirming}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              <Trans>Delete every partial file?</Trans>
            </AlertDialogTitle>
            <AlertDialogDescription>
              {totalBytes > 0 ? (
                <Trans>
                  This deletes {count} unfinished temporary files ({size}) left
                  by interrupted jobs. Finished outputs and original recordings
                  are not touched.
                </Trans>
              ) : (
                <Trans>
                  This deletes {count} unfinished temporary files left by
                  interrupted jobs. Finished outputs and original recordings are
                  not touched.
                </Trans>
              )}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>
              <Trans>Cancel</Trans>
            </AlertDialogCancel>
            <AlertDialogAction
              className={buttonVariants({ variant: 'destructive' })}
              onClick={() => all.mutate(outputs.map((output) => output.id))}
            >
              <Trans>Delete all</Trans>
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}
