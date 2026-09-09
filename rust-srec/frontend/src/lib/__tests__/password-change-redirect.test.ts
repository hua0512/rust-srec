import { QueryClient } from '@tanstack/react-query';
import type { AnyRouter } from '@tanstack/react-router';
import type { I18n } from '@lingui/core';

import { sessionQueryOptions } from '@/api/session';
import {
  registerPasswordChangeRedirect,
  redirectToChangePasswordOnError,
} from '../password-change-redirect';

const toastWarning = vi.hoisted(() => vi.fn());

vi.mock('sonner', () => ({
  toast: { warning: toastWarning },
}));

vi.mock('@/server/functions', () => ({
  checkAuthFn: vi.fn(),
}));

function passwordChangeRequiredError() {
  return Object.assign(new Error('forbidden'), {
    status: 403,
    body: { code: 'PASSWORD_CHANGE_REQUIRED' },
  });
}

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

/** Records the order in which the session and the router are refreshed. */
function harness(pathname = '/dashboard') {
  const calls: string[] = [];
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const invalidateQueries = vi
    .spyOn(queryClient, 'invalidateQueries')
    .mockImplementation(async () => {
      calls.push('session');
    });
  const router = {
    state: { location: { pathname } },
    invalidate: vi.fn(async () => {
      calls.push('router');
    }),
    navigate: vi.fn(async () => {
      calls.push('navigate');
    }),
  } as unknown as AnyRouter;
  const i18n = { _: (message: unknown) => String(message) } as unknown as I18n;

  registerPasswordChangeRedirect(router, i18n, queryClient);
  return { calls, queryClient, invalidateQueries, router };
}

/** The redirect runs detached from the caller; let its promise chain drain. */
async function settle() {
  for (let i = 0; i < 5; i++) await Promise.resolve();
}

describe('redirectToChangePasswordOnError', () => {
  beforeEach(() => {
    toastWarning.mockReset();
  });

  it('sends an account owing a password change to the change-password page', async () => {
    const { router } = harness();

    redirectToChangePasswordOnError(passwordChangeRequiredError());
    await settle();

    expect(router.navigate).toHaveBeenCalledWith({
      to: '/change-password',
      replace: true,
    });
    expect(toastWarning).toHaveBeenCalled();
  });

  it('drops the cached session before the router re-reads it', async () => {
    const { calls, invalidateQueries } = harness();

    redirectToChangePasswordOnError(passwordChangeRequiredError());
    await settle();

    expect(invalidateQueries).toHaveBeenCalledWith({
      queryKey: sessionQueryOptions.queryKey,
    });
    expect(calls).toEqual(['session', 'router', 'navigate']);
  });

  it('marks a session cached before the requirement was raised as stale', async () => {
    const { queryClient, invalidateQueries } = harness();
    invalidateQueries.mockRestore();
    queryClient.setQueryData(sessionQueryOptions.queryKey, session());

    redirectToChangePasswordOnError(passwordChangeRequiredError());
    await settle();

    expect(
      queryClient
        .getQueryCache()
        .find({ queryKey: sessionQueryOptions.queryKey })
        ?.isStale(),
    ).toBe(true);
  });

  it('ignores errors that are not a password-change requirement', async () => {
    const { router } = harness();

    redirectToChangePasswordOnError(new Error('network down'));
    await settle();

    expect(router.invalidate).not.toHaveBeenCalled();
    expect(router.navigate).not.toHaveBeenCalled();
  });

  it('does not navigate again from the change-password page', async () => {
    const { router } = harness('/change-password');

    redirectToChangePasswordOnError(passwordChangeRequiredError());
    await settle();

    expect(router.navigate).not.toHaveBeenCalled();
  });
});
