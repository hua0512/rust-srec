import { act, renderHook } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { useBatchSelection } from '../use-batch-selection';

describe('useBatchSelection', () => {
  it('adds and removes ids', () => {
    const { result } = renderHook(() =>
      useBatchSelection({ pageIds: ['a', 'b'], scope: 'page-1' }),
    );

    act(() => result.current.handleSelectionChange('a', true));
    expect([...result.current.selectedIds]).toEqual(['a']);

    act(() => result.current.handleSelectionChange('a', false));
    expect(result.current.selectedIds.size).toBe(0);
  });

  it('reports when every id on the page is selected', () => {
    const { result } = renderHook(() =>
      useBatchSelection({ pageIds: ['a', 'b'], scope: 'page-1' }),
    );

    expect(result.current.allPageSelected).toBe(false);
    act(() => result.current.selectPage());
    expect([...result.current.selectedIds]).toEqual(['a', 'b']);
    expect(result.current.allPageSelected).toBe(true);
  });

  // The reason the scope exists: a batch action must never reach rows the
  // person can no longer see, so paging or refiltering drops the selection
  // rather than carrying hidden ids into the next request.
  it('clears the selection when the scope changes', () => {
    const { result, rerender } = renderHook(
      ({ pageIds, scope }: { pageIds: string[]; scope: string }) =>
        useBatchSelection({ pageIds, scope }),
      { initialProps: { pageIds: ['a', 'b'], scope: 'page-1' } },
    );

    act(() => result.current.selectPage());
    expect(result.current.selectedIds.size).toBe(2);

    rerender({ pageIds: ['c', 'd'], scope: 'page-2' });
    expect(result.current.selectedIds.size).toBe(0);
  });

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

  it('leaving selection mode discards the selection', () => {
    const { result } = renderHook(() =>
      useBatchSelection({ pageIds: ['a'], scope: 'page-1' }),
    );

    act(() => result.current.toggleSelectionMode());
    act(() => result.current.selectPage());

    act(() => result.current.exitSelectionMode());
    expect(result.current.selectionMode).toBe(false);
    expect(result.current.selectedIds.size).toBe(0);
  });
});
