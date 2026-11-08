import { useCallback } from 'react';
import { useCanGoBack, useNavigate, useRouter } from '@tanstack/react-router';
import type { LinkOptions } from '@tanstack/react-router';

/**
 * Returns to the previous in-app history entry, or navigates to `fallback`
 * when the page was opened directly (new tab, bookmark).
 *
 * List pages keep pagination, filters and sorting in their search params, so
 * stepping back restores the exact list view, whereas navigating to the bare
 * list route would reset it to the first page.
 *
 * Pass `ignoreBlocker` after a successful save: the history library checks
 * blockers for a back step only when `popstate` fires, by which time the save
 * is no longer pending and an unsaved-changes guard would still see a dirty
 * form.
 */
export function useGoBack(fallback: LinkOptions['to']) {
  const router = useRouter();
  const canGoBack = useCanGoBack();
  const navigate = useNavigate();

  return useCallback(
    ({ ignoreBlocker = false }: { ignoreBlocker?: boolean } = {}) => {
      if (canGoBack) {
        router.history.back({ ignoreBlocker });
      } else {
        void navigate({ to: fallback, ignoreBlocker });
      }
    },
    [canGoBack, router, navigate, fallback],
  );
}
