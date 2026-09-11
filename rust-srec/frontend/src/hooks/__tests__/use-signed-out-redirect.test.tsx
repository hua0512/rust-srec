import { act, cleanup, renderHook } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';

import { sessionQueryOptions } from '@/api/session';
import { useSignedOutRedirect } from '../use-signed-out-redirect';

const checkAuthFnMock = vi.hoisted(() => vi.fn());

vi.mock('@/server/functions', () => ({
  checkAuthFn: checkAuthFnMock,
}));

const router = vi.hoisted(() => ({
  navigate: vi.fn(async () => {}),
  state: {
    status: 'idle' as 'idle' | 'pending',
    location: { pathname: '/streamers', href: '/streamers?page=3&q=abc' },
  },
}));

vi.mock('@tanstack/react-router', () => ({
  useRouter: () => router,
  useRouterState: ({
    select,
  }: {
    select: (state: typeof router.state) => unknown;
  }) => select(router.state),
}));

function session() {
  return {
    username: 'user',
    token: {
      access_token: 'access-token',
      expires_in: Date.now() + 10 * 60_000,
      refresh_expires_in: Date.now() + 60 * 60_000,
    },
    roles: [],
    mustChangePassword: false,
  };
}

/**
 * Writes the session query the way a check result lands. React Query hands the
 * change to observers on a timer, so the write is awaited past that tick.
 */
async function setSession(
  queryClient: QueryClient,
  value: ReturnType<typeof session> | null,
) {
  await act(async () => {
    queryClient.setQueryData(sessionQueryOptions.queryKey, value);
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
}

function renderRedirect(queryClient: QueryClient) {
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
  );
  return renderHook(() => useSignedOutRedirect(), { wrapper });
}

const loginRedirect = {
  to: '/login',
  search: { redirect: '/streamers?page=3&q=abc' },
  replace: true,
};

describe('useSignedOutRedirect', () => {
  let queryClient: QueryClient;

  beforeEach(() => {
    queryClient = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    router.navigate.mockClear();
    router.state.status = 'idle';
    checkAuthFnMock.mockReset();
  });

  afterEach(() => {
    cleanup();
    queryClient.clear();
  });

  it('leaves an unchecked or signed-in session alone', async () => {
    renderRedirect(queryClient);
    expect(router.navigate).not.toHaveBeenCalled();

    await setSession(queryClient, session());

    expect(router.navigate).not.toHaveBeenCalled();
  });

  it('never runs the session check itself', async () => {
    queryClient.setQueryData(sessionQueryOptions.queryKey, session());
    renderRedirect(queryClient);
    await act(async () => {
      await Promise.resolve();
    });

    expect(checkAuthFnMock).not.toHaveBeenCalled();
  });

  it('sends a signed-out visitor to login with the page they were on', async () => {
    queryClient.setQueryData(sessionQueryOptions.queryKey, session());
    renderRedirect(queryClient);

    await setSession(queryClient, null);

    expect(router.navigate).toHaveBeenCalledOnce();
    expect(router.navigate).toHaveBeenCalledWith(loginRedirect);
  });

  it('redirects once, not again while its own navigation is in flight', async () => {
    queryClient.setQueryData(sessionQueryOptions.queryKey, session());
    const { rerender } = renderRedirect(queryClient);

    await setSession(queryClient, null);
    router.state.status = 'pending';
    rerender();
    rerender();

    expect(router.navigate).toHaveBeenCalledOnce();
  });

  it('lets a navigation in flight finish, then redirects if still signed out', async () => {
    queryClient.setQueryData(sessionQueryOptions.queryKey, session());
    const { rerender } = renderRedirect(queryClient);

    // Sign-out and password change write `null` and navigate in the same
    // tick, so the hook must stay out of the way while the router is busy.
    router.state.status = 'pending';
    rerender();
    await setSession(queryClient, null);
    expect(router.navigate).not.toHaveBeenCalled();

    router.state.status = 'idle';
    rerender();

    expect(router.navigate).toHaveBeenCalledOnce();
    expect(router.navigate).toHaveBeenCalledWith(loginRedirect);
  });

  it('stands down once the visitor is signed in again', async () => {
    queryClient.setQueryData(sessionQueryOptions.queryKey, null);
    renderRedirect(queryClient);
    expect(router.navigate).toHaveBeenCalledOnce();

    await setSession(queryClient, session());
    await setSession(queryClient, session());

    expect(router.navigate).toHaveBeenCalledOnce();
  });
});
