import { useCallback, useEffect, useMemo, useState } from 'react';

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
