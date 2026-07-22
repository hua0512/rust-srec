import { useNavigate } from '@tanstack/react-router';
import { useCallback } from 'react';

/**
 * Returns an `updateSearch(partial)` that merges `partial` into the current
 * route's URL search params and navigates with `replace: true`, so filter and
 * pagination changes update the URL without stacking browser-history entries.
 *
 * `undefined`, `null`, and `''` values are stripped so clearing a filter removes
 * it from the URL. To reset a page back to its first page or clear a filter,
 * pass `undefined` — the derived value then falls back to the caller's `?? default`.
 *
 * Type it with the route's search shape, e.g.
 * `const updateSearch = useUpdateSearch<typeof search>()` where
 * `search = Route.useSearch()`.
 */
export function useUpdateSearch<TSearch extends Record<string, unknown>>() {
  const navigate = useNavigate();
  return useCallback(
    (updates: Partial<TSearch>) => {
      // The standalone useNavigate() types `search` as the union of every
      // route's params, which a generic helper cannot satisfy; the reducer is
      // cast at this single boundary. Callers keep type safety via `updates`.
      const reduce = (prev: Record<string, unknown>) => {
        const next: Record<string, unknown> = { ...prev, ...updates };
        for (const key of Object.keys(next)) {
          const value = next[key];
          if (value === undefined || value === null || value === '') {
            delete next[key];
          }
        }
        return next;
      };
      void navigate({ search: reduce as never, replace: true });
    },
    [navigate],
  );
}
