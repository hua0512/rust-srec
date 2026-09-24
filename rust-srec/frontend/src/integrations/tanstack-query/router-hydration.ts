import {
  dehydrate,
  hydrate,
  type DehydratedState,
  type QueryClient,
} from '@tanstack/react-query';
import type { AnyRouter } from '@tanstack/react-router';

/**
 * Sends the query results that route loaders filled during server rendering
 * along with the page, and puts them in the browser's cache before the first
 * render, so a page whose data was loaded on the server shows it straight away
 * instead of fetching it again after hydration.
 */
export function routerWithQueryHydration<TRouter extends AnyRouter>(
  router: TRouter,
  queryClient: QueryClient,
): TRouter {
  if (router.isServer) {
    const ogDehydrate = router.options.dehydrate;
    router.options.dehydrate = async () => ({
      ...(await ogDehydrate?.()),
      queryClientState: dehydrate(queryClient),
    });
  } else {
    const ogHydrate = router.options.hydrate;
    router.options.hydrate = async (dehydrated: any) => {
      const state = dehydrated?.queryClientState as DehydratedState | undefined;
      if (state) hydrate(queryClient, state);
      await ogHydrate?.(dehydrated);
    };
  }
  return router;
}
