import { act, cleanup, renderHook } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';

import { sessionQueryOptions } from '@/api/session';
import { useSignedOutRedirect } from '../use-signed-out-redirect';
import { testSession } from '@/test/session';
import { newQueryClient } from '@/test/render';

vi.mock('@/server/functions/auth', () => ({
  checkAuthFn: vi.fn(),
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

/**
 * Writes the session query the way a check result lands. React Query hands the
 * change to observers on a timer, so the write is awaited past that tick.
 */
async function setSession(
  queryClient: QueryClient,
  value: ReturnType<typeof testSession> | null,
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
    queryClient = newQueryClient();
    router.navigate.mockClear();
    router.state.status = 'idle';
  });

  afterEach(() => {
    cleanup();
    queryClient.clear();
  });

  it('leaves an unchecked or signed-in session alone', async () => {
    renderRedirect(queryClient);
    expect(router.navigate).not.toHaveBeenCalled();

    await setSession(queryClient, testSession());

    expect(router.navigate).not.toHaveBeenCalled();
  });

  it('sends a signed-out visitor to login with the page they were on', async () => {
    queryClient.setQueryData(sessionQueryOptions.queryKey, testSession());
    renderRedirect(queryClient);

    await setSession(queryClient, null);

    expect(router.navigate).toHaveBeenCalledOnce();
    expect(router.navigate).toHaveBeenCalledWith(loginRedirect);
  });

  it('lets a navigation in flight finish, then redirects if still signed out', async () => {
    queryClient.setQueryData(sessionQueryOptions.queryKey, testSession());
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
});
