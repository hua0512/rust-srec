import type { QueryClient } from '@tanstack/react-query';
import { isRedirect } from '@tanstack/react-router';

import { sessionQueryOptions } from '@/api/session';
import { Route } from '../_public/login';
import { newQueryClient } from '@/test/render';
import { testSession } from '@/test/session';

const checkAuthFnMock = vi.hoisted(() => vi.fn());

vi.mock('@/server/functions/auth', () => ({
  checkAuthFn: checkAuthFnMock,
}));

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
    checkAuthFnMock.mockResolvedValue(testSession());

    await expect(redirectTarget(runGuard(newQueryClient()))).resolves.toBe(
      '/dashboard',
    );
  });

  it('keeps a visitor who still has to change their password', async () => {
    checkAuthFnMock.mockResolvedValue(
      testSession({ mustChangePassword: true }),
    );

    await expect(
      redirectTarget(runGuard(newQueryClient())),
    ).resolves.toBeUndefined();
  });

  it('re-checks instead of trusting a cached signed-out result', async () => {
    const queryClient = newQueryClient();
    queryClient.setQueryData(sessionQueryOptions.queryKey, null);
    checkAuthFnMock.mockResolvedValue(testSession());

    await expect(redirectTarget(runGuard(queryClient))).resolves.toBe(
      '/dashboard',
    );
    expect(checkAuthFnMock).toHaveBeenCalledTimes(1);
  });
});
