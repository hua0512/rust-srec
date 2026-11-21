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
});
