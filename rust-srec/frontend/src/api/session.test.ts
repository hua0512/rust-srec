import { QueryClient } from '@tanstack/react-query';

import { sessionQueryOptions } from './session';
import { testSession } from '@/test/session';
import { newQueryClient } from '@/test/render';

const checkAuthFnMock = vi.hoisted(() => vi.fn());

vi.mock('@/server/functions/auth', () => ({
  checkAuthFn: checkAuthFnMock,
}));

describe('sessionQueryOptions', () => {
  beforeEach(() => {
    checkAuthFnMock.mockReset();
  });

  it('retains the last session when an authentication check rejects', async () => {
    const queryClient = newQueryClient();
    // Inside the renewal window, so the check actually runs.
    const session = testSession({ expiresInMs: 5_000 });
    queryClient.setQueryData(sessionQueryOptions.queryKey, session);
    checkAuthFnMock.mockRejectedValueOnce(new Error('temporary failure'));

    await expect(queryClient.fetchQuery(sessionQueryOptions)).rejects.toThrow(
      'temporary failure',
    );
    expect(queryClient.getQueryData(sessionQueryOptions.queryKey)).toEqual(
      session,
    );
  });

  it('measures its reuse window from when the check ran, not from the call', async () => {
    vi.useFakeTimers();
    try {
      const queryClient = newQueryClient();
      checkAuthFnMock.mockResolvedValue(testSession({ expiresInMs: 70_000 }));

      await queryClient.fetchQuery(sessionQueryOptions);
      // Still inside the window the first check earned, even though the token
      // is now closer to its renewal than it was.
      await vi.advanceTimersByTimeAsync(20_000);
      await queryClient.fetchQuery(sessionQueryOptions);

      expect(checkAuthFnMock).toHaveBeenCalledOnce();
    } finally {
      vi.useRealTimers();
    }
  });

  it('fails a check straight away instead of retrying behind the guard', async () => {
    const queryClient = new QueryClient();
    checkAuthFnMock.mockRejectedValue(new Error('endpoint down'));

    await expect(queryClient.fetchQuery(sessionQueryOptions)).rejects.toThrow(
      'endpoint down',
    );
    expect(checkAuthFnMock).toHaveBeenCalledOnce();
  });
});
