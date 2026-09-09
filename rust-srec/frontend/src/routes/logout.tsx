import { redirect, createFileRoute } from '@tanstack/react-router';

import { logoutFn } from '@/server/functions';
import { sessionQueryOptions } from '@/api/session';
import { useDownloadStore } from '@/store/downloads';
import { useUploadStore } from '@/store/uploads';

export const Route = createFileRoute('/logout')({
  preload: false,
  loader: async ({ context }) => {
    await logoutFn();

    // Query results and the live download/upload state outlive the session
    // cookie, so without this the next person to sign in on the same tab would
    // briefly see the previous account's streamers, sessions and transfers.
    // Polling observers are still mounted until the redirect commits, so
    // in-flight fetches are cancelled first rather than left to write a round
    // of results back into the cache that was just emptied.
    await context.queryClient.cancelQueries();
    context.queryClient.clear();
    // Emptying the cache is not enough on its own: the session query is read
    // with an `initialData` user, so an absent entry is immediately re-seeded
    // with the account that just signed out and the authenticated tree would
    // accept it. Recording an explicit "signed out" instead makes that seed
    // inert and forces the next navigation to ask the server.
    context.queryClient.setQueryData(sessionQueryOptions.queryKey, null);
    useDownloadStore.getState().clearAll();
    useUploadStore.getState().clearAll();

    throw redirect({
      to: '/login',
    });
  },
});
