import { QueryClient } from '@tanstack/react-query';
import { isRedirect } from '@tanstack/react-router';

import { sessionQueryOptions } from '@/api/session';
import { Route } from '../_authed';
import { Route as LogoutRoute } from '../logout';

const checkAuthFnMock = vi.hoisted(() => vi.fn());
const logoutFnMock = vi.hoisted(() => vi.fn());

vi.mock('@/server/functions', () => ({
  checkAuthFn: checkAuthFnMock,
  logoutFn: logoutFnMock,
}));

type RedirectOptions = { to?: string; search?: { redirect?: string } };

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

function runGuard(queryClient: QueryClient, href: string) {
  const url = new URL(href, 'https://srec.test');
  const beforeLoad = Route.options.beforeLoad as (args: {
    context: { queryClient: QueryClient };
    location: { pathname: string; href: string };
  }) => Promise<{ user: unknown }>;

  return beforeLoad({
    context: { queryClient },
    location: { pathname: url.pathname, href },
  });
}

function runLogout(queryClient: QueryClient) {
  const loader = LogoutRoute.options.loader as (args: {
    context: { queryClient: QueryClient };
  }) => Promise<unknown>;

  return loader({ context: { queryClient } });
}

/**
 * What WebSocketProvider's `useQuery({ ...sessionQueryOptions, initialData })`
 * does on every render: seed the session from route context, but only while no
 * entry exists — an entry holding `null` is still an entry.
 */
function seedLikeTheProvider(
  queryClient: QueryClient,
  user: ReturnType<typeof session>,
) {
  const existing = queryClient
    .getQueryCache()
    .find({ queryKey: sessionQueryOptions.queryKey });
  if (existing) return;
  queryClient.setQueryData(sessionQueryOptions.queryKey, user);
}

async function captureRedirect(promise: Promise<unknown>) {
  try {
    await promise;
  } catch (error) {
    if (isRedirect(error))
      return (error as Response & { options: RedirectOptions }).options;
    throw error;
  }
  throw new Error('expected the guard to redirect');
}

describe('/_authed guard', () => {
  let queryClient: QueryClient;

  beforeEach(() => {
    checkAuthFnMock.mockReset();
    logoutFnMock.mockReset();
    queryClient = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
  });

  it('sends an unauthenticated visitor to login with the page they wanted', async () => {
    checkAuthFnMock.mockResolvedValue(null);

    const options = await captureRedirect(
      runGuard(queryClient, '/streamers?page=3&q=abc'),
    );

    expect(options.to).toBe('/login');
    expect(options.search?.redirect).toBe('/streamers?page=3&q=abc');
  });

  it('sends a visitor owing a password change to the change-password page', async () => {
    checkAuthFnMock.mockResolvedValue(session({ mustChangePassword: true }));

    const options = await captureRedirect(runGuard(queryClient, '/dashboard'));

    expect(options.to).toBe('/change-password');
  });

  it('lets that visitor stay on the change-password page', async () => {
    checkAuthFnMock.mockResolvedValue(session({ mustChangePassword: true }));

    await expect(
      runGuard(queryClient, '/change-password'),
    ).resolves.toMatchObject({ user: { mustChangePassword: true } });
  });

  it('checks the session once for a run of navigations', async () => {
    checkAuthFnMock.mockResolvedValue(session());

    await runGuard(queryClient, '/dashboard');
    await runGuard(queryClient, '/streamers');
    await runGuard(queryClient, '/pipeline/jobs');

    expect(checkAuthFnMock).toHaveBeenCalledOnce();
  });

  it('checks again rather than trusting a remembered signed-out answer', async () => {
    checkAuthFnMock.mockResolvedValueOnce(null);
    await captureRedirect(runGuard(queryClient, '/dashboard'));

    checkAuthFnMock.mockResolvedValueOnce(session());
    await expect(runGuard(queryClient, '/dashboard')).resolves.toMatchObject({
      user: { username: 'user' },
    });
  });

  it('checks again once the cached access token is about to expire', async () => {
    checkAuthFnMock.mockResolvedValue(
      session({
        token: {
          access_token: 'about-to-expire',
          expires_in: Date.now() + 5_000,
          refresh_expires_in: Date.now() + 120_000,
        },
      }),
    );

    await runGuard(queryClient, '/dashboard');
    await runGuard(queryClient, '/streamers');

    expect(checkAuthFnMock).toHaveBeenCalledTimes(2);
  });

  it('asks the server again after signing out, even with a seeded session', async () => {
    const previous = session();
    seedLikeTheProvider(queryClient, previous);
    logoutFnMock.mockResolvedValue(undefined);

    await captureRedirect(runLogout(queryClient));
    // The provider re-renders while the redirect to /login commits.
    seedLikeTheProvider(queryClient, previous);

    checkAuthFnMock.mockResolvedValue(null);
    const options = await captureRedirect(runGuard(queryClient, '/dashboard'));

    expect(options.to).toBe('/login');
    expect(checkAuthFnMock).toHaveBeenCalledOnce();
  });

  it('shares its result with the session query the provider reads', async () => {
    checkAuthFnMock.mockResolvedValue(session());

    await runGuard(queryClient, '/dashboard');

    expect(
      queryClient.getQueryData(sessionQueryOptions.queryKey),
    ).toMatchObject({ username: 'user' });
  });
});
