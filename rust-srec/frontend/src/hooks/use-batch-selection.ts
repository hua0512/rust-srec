import { useCallback, useEffect, useMemo, useState } from 'react';
import { useMutation } from '@tanstack/react-query';
import { toast } from 'sonner';
import type { BatchResponse } from '@/api/schemas';

interface UseBatchSelectionOptions {
  /**
   * IDs rendered on the current page, in render order. Used by `selectPage` and
   * to derive `allPageSelected`.
   */
  pageIds: string[];
  /**
   * A value that changes whenever the visible set of rows changes — page,
   * page size, search text, filters, sort. Selection is cleared when it changes
   * so IDs from a previous filter cannot stay selected out of view and get
   * swept up by the next batch action.
   */
  scope: string;
}

/**
 * Selection-mode state for a list page's batch actions.
 *
 * Selection is intentionally local rather than URL-persisted: unlike the
 * filters carried by `useUpdateSearch`, a pending selection should not survive
 * a reload or a shared link.
 */
export function useBatchSelection({
  pageIds,
  scope,
}: UseBatchSelectionOptions) {
  const [selectionMode, setSelectionMode] = useState(false);
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());

  useEffect(() => {
    setSelectedIds(new Set());
  }, [scope]);

  const allPageSelected = useMemo(
    () => pageIds.length > 0 && pageIds.every((id) => selectedIds.has(id)),
    [pageIds, selectedIds],
  );

  const handleSelectionChange = useCallback((id: string, selected: boolean) => {
    setSelectedIds((current) => {
      const next = new Set(current);
      if (selected) {
        next.add(id);
      } else {
        next.delete(id);
      }
      return next;
    });
  }, []);

  const selectPage = useCallback(() => {
    setSelectedIds(new Set(pageIds));
  }, [pageIds]);

  const clearSelection = useCallback(() => {
    setSelectedIds(new Set());
  }, []);

  const toggleSelectionMode = useCallback(() => {
    setSelectionMode((current) => {
      if (current) setSelectedIds(new Set());
      return !current;
    });
  }, []);

  const exitSelectionMode = useCallback(() => {
    setSelectionMode(false);
    setSelectedIds(new Set());
  }, []);

  return {
    selectionMode,
    selectedIds,
    setSelectedIds,
    allPageSelected,
    handleSelectionChange,
    selectPage,
    clearSelection,
    toggleSelectionMode,
    exitSelectionMode,
  };
}

interface UseBatchActionMutationOptions<TVariables> {
  mutationFn: (variables: TVariables) => Promise<BatchResponse>;
  /** Refetches what the batch changed; runs after every response, partial or not. */
  invalidate: () => void;
  selection: Pick<
    ReturnType<typeof useBatchSelection>,
    'setSelectedIds' | 'exitSelectionMode'
  >;
  /** Toast text for a batch where every item went through. */
  successMessage: (succeeded: number) => string;
  /** Toast text for a batch where some items failed. */
  partialMessage: (succeeded: number, failed: number) => string;
  /** Toast text when the request itself fails and the error has no message. */
  errorMessage: string;
}

/** Item errors listed under a partial-failure toast; the rest stay selected. */
const LISTED_ERRORS = 3;

/**
 * The mutation behind a list page's batch action bar.
 *
 * Batch endpoints apply items one by one without rolling back, so a response
 * can be partial. A full success leaves selection mode; a partial one keeps
 * only the failed rows selected, so repeating the action targets exactly the
 * rows that did not go through.
 */
export function useBatchActionMutation<TVariables>({
  mutationFn,
  invalidate,
  selection: { setSelectedIds, exitSelectionMode },
  successMessage,
  partialMessage,
  errorMessage,
}: UseBatchActionMutationOptions<TVariables>) {
  return useMutation({
    mutationFn,
    onSuccess: (result) => {
      invalidate();

      if (result.failed === 0) {
        toast.success(successMessage(result.succeeded));
        exitSelectionMode();
        return;
      }

      const failedResults = result.results.filter((item) => !item.success);
      setSelectedIds(new Set(failedResults.map((item) => item.id)));
      toast.warning(partialMessage(result.succeeded, result.failed), {
        description: failedResults
          .slice(0, LISTED_ERRORS)
          .map((item) => item.error)
          .filter(Boolean)
          .join('; '),
      });
    },
    onError: (error) => {
      toast.error(
        error instanceof Error && error.message ? error.message : errorMessage,
      );
    },
  });
}
