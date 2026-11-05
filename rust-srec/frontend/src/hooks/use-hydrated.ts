import { useSyncExternalStore } from 'react';

function subscribe() {
  return () => {};
}

/**
 * False while server-rendered markup is being hydrated, true from then on and
 * on every client-only render. Gates output that would differ between the
 * server and the browser, such as times relative to now.
 */
export function useHydrated(): boolean {
  return useSyncExternalStore(
    subscribe,
    () => true,
    () => false,
  );
}
