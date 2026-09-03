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
