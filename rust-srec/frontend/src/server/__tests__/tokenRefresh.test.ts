import type { SessionData } from '@/utils/session';

const useAppSessionMock = vi.hoisted(() => vi.fn());

vi.mock('@/utils/session.server', () => ({
  useAppSession: useAppSessionMock,
}));

vi.mock('@/utils/env', () => ({
  BASE_URL: 'http://backend.test',
}));

import { ensureValidToken, refreshAuthTokenGlobal } from '../tokenRefresh';

// The module keeps per-refresh-token state (in-flight promises, recently
// rotated outcomes), so every test uses its own refresh token.
let refreshTokenCounter = 0;

function createSession(overrides: Partial<SessionData['token']> = {}) {
  refreshTokenCounter += 1;
  const now = Date.now();
  let current: Partial<SessionData> = {
    username: 'alice',
    roles: ['admin'],
    mustChangePassword: false,
    token: {
      access_token: 'access-1',
      refresh_token: `refresh-${refreshTokenCounter}`,
      // Inside ensureValidToken's 30 s buffer, so it refreshes.
      expires_in: now + 10_000,
      refresh_expires_in: now + 3_600_000,
      ...overrides,
    },
  };

  const session = {
    get data() {
      return current;
    },
    update: vi.fn(async (next: SessionData) => {
      current = next;
    }),
    clear: vi.fn(async () => {
      current = {};
    }),
  };

  useAppSessionMock.mockResolvedValue(session);
  return session;
}

function deferred<T>() {
  let resolve!: (value: T | PromiseLike<T>) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

function refreshResponse(refreshToken: string) {
  return Response.json({
    access_token: `access-${refreshToken}`,
    refresh_token: refreshToken,
    expires_in: 900,
    refresh_expires_in: 86_400,
  });
}

describe('tokenRefresh', () => {
  beforeEach(() => {
    vi.spyOn(console, 'error').mockImplementation(() => {});
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it('keeps the session and the current access token when the refresh endpoint returns 500', async () => {
    const session = createSession();
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue(new Response('boom', { status: 500 })),
    );

    const user = await ensureValidToken();

    expect(session.clear).not.toHaveBeenCalled();
    expect(user?.token.access_token).toBe('access-1');
    expect(session.data.token?.refresh_token).toMatch(/^refresh-/);
  });

  it('keeps the session when the refresh endpoint cannot be reached', async () => {
    const session = createSession();
    vi.stubGlobal(
      'fetch',
      vi.fn().mockRejectedValue(new TypeError('fetch failed')),
    );

    await expect(refreshAuthTokenGlobal()).resolves.toEqual({
      status: 'transient',
    });
    expect(session.clear).not.toHaveBeenCalled();
  });

  it('keeps the session when the request times out, and retries on the next call', async () => {
    const session = createSession();
    const fetchMock = vi
      .fn()
      .mockRejectedValue(
        new DOMException('The operation timed out', 'TimeoutError'),
      );
    vi.stubGlobal('fetch', fetchMock);

    await expect(refreshAuthTokenGlobal()).resolves.toEqual({
      status: 'transient',
    });
    // The in-flight entry is dropped once the attempt settles, so the next
    // caller issues its own request instead of replaying the failure.
    await expect(refreshAuthTokenGlobal()).resolves.toEqual({
      status: 'transient',
    });
    expect(fetchMock).toHaveBeenCalledTimes(2);
    expect(session.clear).not.toHaveBeenCalled();
  });

  it('does not turn one transient failure into a rejection for concurrent callers', async () => {
    const session = createSession();
    const fetchMock = vi
      .fn()
      .mockResolvedValue(new Response(null, { status: 503 }));
    vi.stubGlobal('fetch', fetchMock);

    const results = await Promise.all([
      refreshAuthTokenGlobal(),
      refreshAuthTokenGlobal(),
    ]);

    expect(results).toEqual([{ status: 'transient' }, { status: 'transient' }]);
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(session.clear).not.toHaveBeenCalled();
  });

  it('clears the session when the refresh endpoint returns 401', async () => {
    const session = createSession();
    vi.stubGlobal(
      'fetch',
      vi
        .fn()
        .mockResolvedValue(
          Response.json({ message: 'invalid refresh token' }, { status: 401 }),
        ),
    );

    const user = await ensureValidToken();

    expect(session.clear).toHaveBeenCalled();
    expect(user).toBeNull();
  });

  it('clears the session when the account behind the token is disabled', async () => {
    const session = createSession();
    vi.stubGlobal(
      'fetch',
      vi
        .fn()
        .mockResolvedValue(
          Response.json(
            { code: 'ACCOUNT_DISABLED', message: 'Account is disabled' },
            { status: 403 },
          ),
        ),
    );

    const user = await ensureValidToken();

    expect(session.clear).toHaveBeenCalled();
    expect(user).toBeNull();
  });

  it('keeps the session for a 403 that did not come from the backend', async () => {
    const session = createSession();
    vi.stubGlobal(
      'fetch',
      vi
        .fn()
        .mockResolvedValue(
          new Response('<html>Forbidden</html>', { status: 403 }),
        ),
    );

    await expect(refreshAuthTokenGlobal()).resolves.toEqual({
      status: 'transient',
    });
    expect(session.clear).not.toHaveBeenCalled();
  });

  it.each([404, 429, 500, 502, 503])(
    'keeps the session when the refresh endpoint answers %i',
    async (status) => {
      const session = createSession();
      vi.stubGlobal(
        'fetch',
        vi.fn().mockResolvedValue(new Response(null, { status })),
      );

      await expect(refreshAuthTokenGlobal()).resolves.toEqual({
        status: 'transient',
      });
      expect(session.clear).not.toHaveBeenCalled();
    },
  );

  it.each([400, 422])(
    'clears the session when the refresh endpoint answers %i',
    async (status) => {
      const session = createSession();
      vi.stubGlobal(
        'fetch',
        vi.fn().mockResolvedValue(new Response(null, { status })),
      );

      await expect(refreshAuthTokenGlobal()).resolves.toEqual({
        status: 'rejected',
      });
      expect(session.clear).toHaveBeenCalled();
    },
  );

  it('stores the rotated tokens when the refresh succeeds', async () => {
    const session = createSession();
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue(
        Response.json({
          access_token: 'access-2',
          refresh_token: 'refresh-rotated',
          expires_in: 900,
          refresh_expires_in: 86_400,
        }),
      ),
    );

    const user = await ensureValidToken();

    expect(session.clear).not.toHaveBeenCalled();
    expect(user?.token.access_token).toBe('access-2');
    expect(session.data.token?.refresh_token).toBe('refresh-rotated');
  });

  it.each(['pending', 'failed'])(
    'shares a rotation while the first cookie write is %s',
    async (writeState) => {
      const first = createSession();
      const original = structuredClone(first.data);
      const second = createSession(original.token);
      const gate = deferred<void>();
      const updateStarted = deferred<void>();
      first.update.mockImplementation(async () => {
        updateStarted.resolve();
        await gate.promise;
        if (writeState === 'failed') throw new Error('cookie write failed');
      });
      const rotated = `${original.token!.refresh_token}-rotated`;
      const fetchMock = vi
        .fn()
        .mockResolvedValueOnce(refreshResponse(rotated))
        .mockResolvedValue(new Response(null, { status: 401 }));
      vi.stubGlobal('fetch', fetchMock);
      useAppSessionMock.mockResolvedValue(first);
      const firstCall = refreshAuthTokenGlobal().catch(
        (error: unknown) => error,
      );
      await updateStarted.promise;
      if (writeState === 'failed') {
        gate.resolve();
        await firstCall;
      }
      useAppSessionMock.mockResolvedValue(second);
      try {
        await expect(refreshAuthTokenGlobal()).resolves.toEqual({
          status: 'refreshed',
          accessToken: `access-${rotated}`,
        });
        expect(second.data.token?.refresh_token).toBe(rotated);
        expect(second.clear).not.toHaveBeenCalled();
        expect(fetchMock).toHaveBeenCalledTimes(1);
      } finally {
        gate.resolve();
        await firstCall;
      }
    },
  );

  it('shares one successful refresh between callers using the desktop session', async () => {
    const session = createSession();
    const rotated = `${session.data.token!.refresh_token}-rotated`;
    const fetchMock = vi.fn().mockResolvedValue(refreshResponse(rotated));
    vi.stubGlobal('fetch', fetchMock);
    const results = await Promise.all([
      refreshAuthTokenGlobal(),
      refreshAuthTokenGlobal(),
    ]);
    expect(results).toEqual([
      { status: 'refreshed', accessToken: `access-${rotated}` },
      { status: 'refreshed', accessToken: `access-${rotated}` },
    ]);
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(session.update).toHaveBeenCalledTimes(1);
  });

  it.each([
    ['refreshed', false],
    ['rejected', false],
    ['transient', false],
    ['refreshed', true],
    ['rejected', true],
    ['transient', true],
  ] as const)(
    'ignores a %s refresh after sign-out (new login: %s)',
    async (outcome, loginAgain) => {
      const session = createSession();
      const original = structuredClone(session.data) as SessionData;
      const pending = deferred<Response>();
      const started = deferred<void>();
      vi.stubGlobal(
        'fetch',
        vi.fn(() => {
          started.resolve();
          return pending.promise;
        }),
      );
      const refresh = refreshAuthTokenGlobal();
      await started.promise;
      await session.clear();
      if (loginAgain) {
        await session.update({
          ...original,
          username: 'bob',
          token: {
            ...original.token,
            access_token: 'bob-access',
            refresh_token: 'bob-refresh',
          },
        });
      }
      const expected = structuredClone(session.data);
      session.update.mockClear();
      session.clear.mockClear();
      pending.resolve(
        outcome === 'refreshed'
          ? refreshResponse(`${original.token.refresh_token}-rotated`)
          : new Response(null, { status: outcome === 'rejected' ? 401 : 503 }),
      );
      await expect(refresh).resolves.toEqual({ status: 'superseded' });
      expect(session.data).toEqual(expected);
      expect(session.update).not.toHaveBeenCalled();
      expect(session.clear).not.toHaveBeenCalled();
    },
  );

  it('keeps the current login visible when a session check is superseded', async () => {
    const session = createSession();
    const original = structuredClone(session.data) as SessionData;
    const pending = deferred<Response>();
    const started = deferred<void>();
    vi.stubGlobal(
      'fetch',
      vi.fn(() => {
        started.resolve();
        return pending.promise;
      }),
    );
    const check = ensureValidToken();
    await started.promise;
    await session.update({
      ...original,
      username: 'bob',
      token: {
        ...original.token,
        access_token: 'bob-access',
        refresh_token: 'bob-refresh',
      },
    });
    pending.resolve(new Response(null, { status: 401 }));
    await expect(check).resolves.toMatchObject({
      username: 'bob',
      token: { access_token: 'bob-access' },
    });
    expect(session.clear).not.toHaveBeenCalled();
  });

  it('repairs every predecessor after successive rotations', async () => {
    const session = createSession();
    const original = structuredClone(session.data) as SessionData;
    const r1 = `${original.token.refresh_token}-r1`;
    const r2 = `${original.token.refresh_token}-r2`;
    const r3 = `${original.token.refresh_token}-r3`;
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(refreshResponse(r1))
      .mockResolvedValueOnce(refreshResponse(r2))
      .mockResolvedValueOnce(refreshResponse(r3));
    vi.stubGlobal('fetch', fetchMock);
    await refreshAuthTokenGlobal();
    const firstRotation = structuredClone(session.data) as SessionData;
    await refreshAuthTokenGlobal();
    await refreshAuthTokenGlobal();
    for (const stale of [original, firstRotation]) {
      const request = createSession(stale.token);
      await expect(refreshAuthTokenGlobal()).resolves.toEqual({
        status: 'refreshed',
        accessToken: `access-${r3}`,
      });
      expect(request.data.token?.refresh_token).toBe(r3);
    }
    expect(fetchMock).toHaveBeenCalledTimes(3);
  });

  it.each(['refreshed', 'rejected', 'transient'] as const)(
    'waits for a cached successor already rotating (%s)',
    async (outcome) => {
      const session = createSession();
      const original = structuredClone(session.data) as SessionData;
      const r1 = `${original.token.refresh_token}-r1`;
      const r2 = `${original.token.refresh_token}-r2`;
      const pending = deferred<Response>();
      const started = deferred<void>();
      const fetchMock = vi
        .fn()
        .mockResolvedValueOnce(refreshResponse(r1))
        .mockImplementationOnce(() => {
          started.resolve();
          return pending.promise;
        });
      vi.stubGlobal('fetch', fetchMock);
      await refreshAuthTokenGlobal();
      const successorCall = refreshAuthTokenGlobal();
      await started.promise;
      const staleRequest = createSession(original.token);
      const staleCall = refreshAuthTokenGlobal();
      await Promise.resolve();
      expect(staleRequest.update).not.toHaveBeenCalled();
      pending.resolve(
        outcome === 'refreshed'
          ? refreshResponse(r2)
          : new Response(null, { status: outcome === 'rejected' ? 401 : 503 }),
      );
      await successorCall;
      if (outcome === 'rejected') {
        await expect(staleCall).resolves.toEqual({ status: 'rejected' });
        expect(staleRequest.clear).toHaveBeenCalledOnce();
      } else {
        const latest = outcome === 'refreshed' ? r2 : r1;
        await expect(staleCall).resolves.toEqual({
          status: 'refreshed',
          accessToken: `access-${latest}`,
        });
        expect(staleRequest.data.token?.refresh_token).toBe(latest);
      }
      expect(fetchMock).toHaveBeenCalledTimes(2);
    },
  );

  it('does not renew a predecessor cache deadline when it is read', async () => {
    vi.useFakeTimers();
    try {
      const session = createSession();
      const original = structuredClone(session.data) as SessionData;
      const r1 = `${original.token.refresh_token}-r1`;
      const fetchMock = vi
        .fn()
        .mockResolvedValueOnce(refreshResponse(r1))
        .mockResolvedValueOnce(new Response(null, { status: 401 }));
      vi.stubGlobal('fetch', fetchMock);
      await refreshAuthTokenGlobal();
      vi.advanceTimersByTime(59_000);
      createSession(original.token);
      await refreshAuthTokenGlobal();
      expect(fetchMock).toHaveBeenCalledTimes(1);
      vi.advanceTimersByTime(1_001);
      createSession(original.token);
      await expect(refreshAuthTokenGlobal()).resolves.toEqual({
        status: 'rejected',
      });
      expect(fetchMock).toHaveBeenCalledTimes(2);
    } finally {
      vi.useRealTimers();
    }
  });

  it('treats a success response without an access token as transient', async () => {
    const session = createSession();
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue(Response.json({ expires_in: 900 })),
    );

    await expect(refreshAuthTokenGlobal()).resolves.toEqual({
      status: 'transient',
    });
    expect(session.update).not.toHaveBeenCalled();
    expect(session.clear).not.toHaveBeenCalled();
  });
});
