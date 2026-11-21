import { createElement, type ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, renderHook, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import {
  useBatchActionMutation,
  useBatchSelection,
} from '../use-batch-selection';

vi.mock('sonner', () => ({
  toast: { success: vi.fn(), warning: vi.fn(), error: vi.fn() },
}));

describe('useBatchSelection', () => {
  it('keeps the selection while the scope is unchanged', () => {
    const { result, rerender } = renderHook(
      ({ pageIds, scope }: { pageIds: string[]; scope: string }) =>
        useBatchSelection({ pageIds, scope }),
      { initialProps: { pageIds: ['a', 'b'], scope: 'page-1' } },
    );

    act(() => result.current.handleSelectionChange('a', true));
    // A refetch hands back an equal-but-new array; that is not a scope change.
    rerender({ pageIds: ['a', 'b'], scope: 'page-1' });
    expect([...result.current.selectedIds]).toEqual(['a']);
  });

  it('keeps selection mode but empties it on scope change', () => {
    const { result, rerender } = renderHook(
      ({ scope }: { scope: string }) =>
        useBatchSelection({ pageIds: ['a'], scope }),
      { initialProps: { scope: 'page-1' } },
    );

    act(() => result.current.toggleSelectionMode());
    act(() => result.current.selectPage());
    expect(result.current.selectionMode).toBe(true);

    rerender({ scope: 'page-2' });
    expect(result.current.selectionMode).toBe(true);
    expect(result.current.selectedIds.size).toBe(0);
  });
});

describe('useBatchActionMutation', () => {
  // A partial batch is how a person retries: only the rows that did not go
  // through stay selected, and selection mode stays open for the retry.
  it('keeps only the failed rows selected after a partial batch', async () => {
    const queryClient = new QueryClient();
    const wrapper = ({ children }: { children: ReactNode }) =>
      createElement(QueryClientProvider, { client: queryClient }, children);
    const { result } = renderHook(
      () => {
        const selection = useBatchSelection({
          pageIds: ['a', 'b', 'c'],
          scope: 'page-1',
        });
        const mutation = useBatchActionMutation({
          mutationFn: async (ids: string[]) => ({
            requested: ids.length,
            succeeded: 1,
            failed: 2,
            results: [
              { id: 'a', success: true },
              { id: 'b', success: false, error: 'busy' },
              { id: 'c', success: false, error: 'gone' },
            ],
          }),
          invalidate: () => {},
          selection,
          successMessage: () => '',
          partialMessage: () => '',
          errorMessage: '',
        });
        return { selection, mutation };
      },
      { wrapper },
    );

    act(() => result.current.selection.toggleSelectionMode());
    act(() => result.current.selection.selectPage());
    act(() => result.current.mutation.mutate(['a', 'b', 'c']));

    await waitFor(() => expect(result.current.mutation.isSuccess).toBe(true));
    expect(result.current.selection.selectionMode).toBe(true);
    expect([...result.current.selection.selectedIds]).toEqual(['b', 'c']);
  });
});
