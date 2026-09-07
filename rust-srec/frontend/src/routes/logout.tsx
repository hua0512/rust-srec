import { redirect, createFileRoute } from '@tanstack/react-router';

import { logoutFn } from '@/server/functions';
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
    useDownloadStore.getState().clearAll();
    useUploadStore.getState().clearAll();

    throw redirect({
      to: '/login',
    });
  },
});
