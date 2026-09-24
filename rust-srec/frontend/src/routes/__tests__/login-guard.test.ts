import { QueryClient } from '@tanstack/react-query';
import { isRedirect } from '@tanstack/react-router';

import { sessionQueryOptions } from '@/api/session';
import { Route } from '../_public/login';

const checkAuthFnMock = vi.hoisted(() => vi.fn());

vi.mock('@/server/functions/auth', () => ({
  checkAuthFn: checkAuthFnMock,
}));

function session(overrides: Record<string, unknown> = {}) {
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

function runGuard(queryClient: QueryClient) {
  const beforeLoad = Route.options.beforeLoad as (args: {
    context: { queryClient: QueryClient };
  }) => Promise<unknown>;
  return beforeLoad({ context: { queryClient } });
}

async function redirectTarget(promise: Promise<unknown>) {
  try {
    await promise;
  } catch (error) {
    if (isRedirect(error))
      return (error as Response & { options: { to?: string } }).options.to;
    throw error;
  }
  return undefined;
}

describe('/login guard', () => {
  beforeEach(() => {
    checkAuthFnMock.mockReset();
  });

  it('sends a signed-in visitor to the dashboard', async () => {
    checkAuthFnMock.mockResolvedValue(session());

    await expect(redirectTarget(runGuard(new QueryClient()))).resolves.toBe(
      '/dashboard',
    );
  });

  it('keeps a visitor who still has to change their password', async () => {
    checkAuthFnMock.mockResolvedValue(session({ mustChangePassword: true }));

    await expect(
      redirectTarget(runGuard(new QueryClient())),
    ).resolves.toBeUndefined();
  });

  it('re-checks instead of trusting a cached signed-out result', async () => {
    const queryClient = new QueryClient();
    queryClient.setQueryData(sessionQueryOptions.queryKey, null);
    checkAuthFnMock.mockResolvedValue(session());

    await expect(redirectTarget(runGuard(queryClient))).resolves.toBe(
      '/dashboard',
    );
    expect(checkAuthFnMock).toHaveBeenCalledTimes(1);
  });

  it('shares the session check with the rest of the app', async () => {
    const queryClient = new QueryClient();
    checkAuthFnMock.mockResolvedValue(null);

    await expect(
      redirectTarget(runGuard(queryClient)),
    ).resolves.toBeUndefined();
    expect(queryClient.getQueryData(sessionQueryOptions.queryKey)).toBeNull();
  });
});
