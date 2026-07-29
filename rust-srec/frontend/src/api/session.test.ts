import { QueryClient } from '@tanstack/react-query';

import { sessionQueryOptions } from './session';

const checkAuthFnMock = vi.hoisted(() => vi.fn());

vi.mock('@/server/functions', () => ({
  checkAuthFn: checkAuthFnMock,
}));

describe('sessionQueryOptions', () => {
  it('retains the last session when an authentication check rejects', async () => {
    const queryClient = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    const session = {
      username: 'user',
      token: {
        access_token: 'access-token',
        expires_in: Date.now() + 60_000,
        refresh_expires_in: Date.now() + 120_000,
      },
      roles: [],
      mustChangePassword: false,
    };
    queryClient.setQueryData(sessionQueryOptions.queryKey, session);
    checkAuthFnMock.mockRejectedValueOnce(new Error('temporary failure'));

    await expect(queryClient.fetchQuery(sessionQueryOptions)).rejects.toThrow(
      'temporary failure',
    );
    expect(queryClient.getQueryData(sessionQueryOptions.queryKey)).toEqual(
      session,
    );
  });
});
