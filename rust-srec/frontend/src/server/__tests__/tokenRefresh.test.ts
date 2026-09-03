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

  // A caller that starts while an earlier successful refresh is still inside
  // session.update sees an empty rotation cache and an empty in-flight map, so
  // it issues its own request for a token the backend has already rotated. Its
  // own answer is worthless; the rotation recorded a moment later is not.
  it.each([
    ['is answered 401', new Response(null, { status: 401 })],
    ['cannot reach the backend', new TypeError('fetch failed')],
    ['gets a body that is not JSON', new Response('<html></html>')],
    ['gets a body without an access token', Response.json({ expires_in: 900 })],
  ])(
    'adopts a concurrent rotation when its own attempt %s',
    async (_label, secondAnswer) => {
      const now = Date.now();
      refreshTokenCounter += 1;
      const token = {
        access_token: 'access-1',
        refresh_token: `refresh-${refreshTokenCounter}`,
        expires_in: now + 10_000,
        refresh_expires_in: now + 3_600_000,
      };

      let releaseUpdate!: () => void;
      const updateGate = new Promise<void>((resolve) => {
        releaseUpdate = resolve;
      });
      const first = {
        data: { username: 'alice', roles: ['admin'], token },
        update: vi.fn(() => updateGate),
        clear: vi.fn(),
      };
      const second = {
        data: { username: 'alice', roles: ['admin'], token },
        update: vi.fn(),
        clear: vi.fn(),
      };

      let deliverSecond!: () => void;
      const secondPending = new Promise<Response>((resolve, reject) => {
        deliverSecond = () =>
          secondAnswer instanceof Response
            ? resolve(secondAnswer)
            : reject(secondAnswer);
      });
      const fetchMock = vi
        .fn()
        .mockResolvedValueOnce(
          Response.json({
            access_token: 'access-2',
            refresh_token: 'refresh-rotated',
            expires_in: 900,
            refresh_expires_in: 86_400,
          }),
        )
        .mockReturnValueOnce(secondPending);
      vi.stubGlobal('fetch', fetchMock);

      useAppSessionMock.mockResolvedValue(first);
      const firstCall = refreshAuthTokenGlobal();
      await vi.waitFor(() => expect(first.update).toHaveBeenCalled());

      useAppSessionMock.mockResolvedValue(second);
      const secondCall = refreshAuthTokenGlobal();
      await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(2));

      releaseUpdate();
      await firstCall;
      deliverSecond();

      await expect(secondCall).resolves.toEqual({
        status: 'refreshed',
        accessToken: 'access-2',
      });
      expect(second.clear).not.toHaveBeenCalled();
      expect(second.update).toHaveBeenCalledWith(
        expect.objectContaining({
          token: expect.objectContaining({
            access_token: 'access-2',
            refresh_token: 'refresh-rotated',
          }),
        }),
      );
    },
  );

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
