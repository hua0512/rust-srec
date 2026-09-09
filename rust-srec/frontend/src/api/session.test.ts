import { QueryClient } from '@tanstack/react-query';

import { sessionQueryOptions } from './session';

const checkAuthFnMock = vi.hoisted(() => vi.fn());

vi.mock('@/server/functions', () => ({
  checkAuthFn: checkAuthFnMock,
}));

/** A session whose access token is comfortably clear of its renewal window. */
function freshSession(overrides: Record<string, unknown> = {}) {
  return {
    username: 'user',
    token: {
      access_token: 'access-token',
      expires_in: Date.now() + 10 * 60_000,
      refresh_expires_in: Date.now() + 60 * 60_000,
    },
    roles: [],
    mustChangePassword: false,
    ...overrides,
  };
}

function createQueryClient() {
  return new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
}

describe('sessionQueryOptions', () => {
  beforeEach(() => {
    checkAuthFnMock.mockReset();
  });

  it('retains the last session when an authentication check rejects', async () => {
    const queryClient = createQueryClient();
    // Inside the renewal window, so the check actually runs.
    const session = freshSession({
      token: {
        access_token: 'access-token',
        expires_in: Date.now() + 5_000,
        refresh_expires_in: Date.now() + 120_000,
      },
    });
    queryClient.setQueryData(sessionQueryOptions.queryKey, session);
    checkAuthFnMock.mockRejectedValueOnce(new Error('temporary failure'));

    await expect(queryClient.fetchQuery(sessionQueryOptions)).rejects.toThrow(
      'temporary failure',
    );
    expect(queryClient.getQueryData(sessionQueryOptions.queryKey)).toEqual(
      session,
    );
  });

  it('checks once and reuses the result for further navigations', async () => {
    const queryClient = createQueryClient();
    const session = freshSession();
    checkAuthFnMock.mockResolvedValue(session);

    await expect(queryClient.fetchQuery(sessionQueryOptions)).resolves.toEqual(
      session,
    );
    await expect(queryClient.fetchQuery(sessionQueryOptions)).resolves.toEqual(
      session,
    );

    expect(checkAuthFnMock).toHaveBeenCalledOnce();
  });

  it('checks again once the access token is inside its renewal window', async () => {
    const queryClient = createQueryClient();
    checkAuthFnMock.mockResolvedValue(
      freshSession({
        token: {
          access_token: 'about-to-expire',
          expires_in: Date.now() + 5_000,
          refresh_expires_in: Date.now() + 120_000,
        },
      }),
    );

    await queryClient.fetchQuery(sessionQueryOptions);
    await queryClient.fetchQuery(sessionQueryOptions);

    expect(checkAuthFnMock).toHaveBeenCalledTimes(2);
  });

  it('measures its reuse window from when the check ran, not from the call', async () => {
    vi.useFakeTimers();
    try {
      const queryClient = createQueryClient();
      checkAuthFnMock.mockResolvedValue(
        freshSession({
          token: {
            access_token: 'access-token',
            expires_in: Date.now() + 70_000,
            refresh_expires_in: Date.now() + 600_000,
          },
        }),
      );

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

  it('never reuses an unauthenticated result', async () => {
    const queryClient = createQueryClient();
    checkAuthFnMock.mockResolvedValue(null);

    await expect(
      queryClient.fetchQuery(sessionQueryOptions),
    ).resolves.toBeNull();
    await queryClient.fetchQuery(sessionQueryOptions);

    expect(checkAuthFnMock).toHaveBeenCalledTimes(2);
  });
});
