/**
 * What a route loader returns for data it prefetches into the query cache.
 *
 * During server rendering the loader waits, so the page is rendered with its
 * data and the results travel to the browser with the HTML (see
 * integrations/tanstack-query/router-hydration.ts). In the browser it does not
 * wait: the page's own queries pick up the requests in flight, so a slow
 * backend shows the page's loading state instead of holding the navigation
 * (whose pending UI is the full-screen startup screen), while a hover preload
 * still gets the data in ahead of the click.
 *
 * Pair it with `prefetchQuery`, which never throws: a failed fetch is left to
 * the page's query, which retries and shows its own error.
 */
export function awaitOnServer(
  prefetch: Promise<unknown>,
): Promise<unknown> | undefined {
  return typeof window === 'undefined' ? prefetch : undefined;
}
