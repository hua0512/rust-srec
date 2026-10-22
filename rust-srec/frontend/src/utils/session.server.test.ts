// @vitest-environment node
import type { SessionData } from './session';
import {
  resolveCookieSecurity,
  resolveSessionSecret,
  useAppSession,
} from './session.server';

// Models what Start's cookie helpers do per request: reads see the cookies the
// browser sent, and writes only queue Set-Cookie headers that the browser
// applies after the response. A queued `null` deletes the cookie.
const http = vi.hoisted(() => ({
  incoming: new Map<string, string>(),
  outgoing: new Map<string, string | null>(),
  request: undefined as Request | undefined,
}));

vi.mock('@tanstack/react-start/server', () => ({
  getCookie: (name: string) => http.incoming.get(name),
  setCookie: (name: string, value: string) => {
    http.outgoing.set(name, value);
  },
  deleteCookie: (name: string) => {
    http.outgoing.set(name, null);
  },
  getRequest: () => {
    if (!http.request) throw new Error('no request context');
    return http.request;
  },
}));

describe('resolveSessionSecret', () => {
  it('uses the configured secret', () => {
    expect(resolveSessionSecret('configured-secret', 'production')).toBe(
      'configured-secret',
    );
  });

  it('allows the built-in secret outside production', () => {
    expect(resolveSessionSecret(undefined, 'development')).toContain(
      'dev_secret',
    );
  });

  it('rejects a missing production secret', () => {
    expect(() => resolveSessionSecret(undefined, 'production')).toThrow(
      'SESSION_SECRET must be set in production',
    );
  });
});

describe('resolveCookieSecurity', () => {
  it.each([
    ['https', true],
    ['HTTPS', true],
    ['https, http', true],
    ['http, https', false],
    ['http', false],
    ['', false],
  ])('reads X-Forwarded-Proto %p', (forwardedProto, secure) => {
    expect(resolveCookieSecurity({ forwardedProto })).toEqual({
      secure,
      warnInsecure: false,
    });
  });

  it('assumes HTTPS in production when no proxy header is present', () => {
    expect(resolveCookieSecurity({ nodeEnv: 'production' })).toEqual({
      secure: true,
      warnInsecure: false,
    });
  });

  it('stays insecure outside production when no proxy header is present', () => {
    expect(resolveCookieSecurity({ nodeEnv: 'development' })).toEqual({
      secure: false,
      warnInsecure: false,
    });
  });

  it('flags a plain-HTTP production request', () => {
    expect(
      resolveCookieSecurity({ forwardedProto: 'http', nodeEnv: 'production' }),
    ).toEqual({ secure: false, warnInsecure: true });
  });

  it('lets COOKIE_SECURE=false win over an https proxy header', () => {
    expect(
      resolveCookieSecurity({
        cookieSecure: 'false',
        forwardedProto: 'https',
        nodeEnv: 'production',
      }),
    ).toEqual({ secure: false, warnInsecure: false });
  });

  it('lets COOKIE_SECURE=true win over a plain-HTTP request', () => {
    expect(
      resolveCookieSecurity({ cookieSecure: 'TRUE', forwardedProto: 'http' })
        .secure,
    ).toBe(true);
  });

  it('ignores an unrecognised COOKIE_SECURE value', () => {
    expect(
      resolveCookieSecurity({ cookieSecure: 'yes', forwardedProto: 'https' })
        .secure,
    ).toBe(true);
  });
});

describe('warnInsecureCookieOnce', () => {
  it('logs once per module instance', async () => {
    // The guard is module state, so load a fresh copy instead of inheriting
    // whatever earlier tests left behind.
    vi.resetModules();
    const { warnInsecureCookieOnce } = await import('./session.server');
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});

    warnInsecureCookieOnce();
    warnInsecureCookieOnce();

    expect(warn).toHaveBeenCalledTimes(1);
    warn.mockRestore();
  });
});

describe('useAppSession', () => {
  const token = {
    access_token: 'access',
    refresh_token: 'refresh',
    expires_in: 1,
    refresh_expires_in: 2,
  };
  const login: SessionData = {
    username: 'alice',
    token,
    roles: ['admin'],
    mustChangePassword: false,
  };

  // Each call starts a new request: the browser applies the previous response's
  // Set-Cookie headers and sends them back, with a new Request object.
  const browserCookies = new Map<string, string>();
  function startRequest() {
    for (const [name, value] of http.outgoing) {
      if (value === null) browserCookies.delete(name);
      else browserCookies.set(name, value);
    }
    http.outgoing.clear();
    http.incoming = new Map(browserCookies);
    http.request = new Request('http://localhost/');
  }

  beforeEach(() => {
    browserCookies.clear();
    http.outgoing.clear();
    startRequest();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('shares an update with later reads in the same request', async () => {
    const session = await useAppSession();
    expect(session.data.username).toBeUndefined();

    await session.update(login);

    const reread = await useAppSession();
    expect(reread.data.username).toBe('alice');
    expect(reread.data.token?.refresh_token).toBe('refresh');
  });

  it('stores the latest update for the next request', async () => {
    const first = await useAppSession();
    await first.update(login);

    startRequest();
    const second = await useAppSession();
    await second.update({ ...login, mustChangePassword: true });

    startRequest();
    const third = await useAppSession();
    expect(third.data).toMatchObject({
      username: 'alice',
      mustChangePassword: true,
    });
  });

  it('clears the cookie-backed session', async () => {
    const first = await useAppSession();
    await first.update(login);

    startRequest();
    const logout = await useAppSession();
    await logout.clear();

    startRequest();
    const after = await useAppSession();
    expect(after.data.username).toBeUndefined();
  });

  it('signs in again after a sign-out in the same request', async () => {
    const first = await useAppSession();
    await first.update(login);

    startRequest();
    const session = await useAppSession();
    await session.clear();
    await session.update({ ...login, username: 'bob' });

    startRequest();
    const after = await useAppSession();
    expect(after.data.username).toBe('bob');
  });

  // Sealing is asynchronous, so a cookie write that finishes after a sign-out
  // must not put the signed-out session back.
  it('keeps a sign-out made while an update is still sealing', async () => {
    const session = await useAppSession();
    const pendingUpdate = session.update(login);
    await session.clear();
    await pendingUpdate;

    startRequest();
    const after = await useAppSession();
    expect(after.data.username).toBeUndefined();
  });

  it('reads a session cookie written by rust-srec 0.6', async () => {
    // Sealed by the TanStack Start/h3 session of rust-srec 0.6 with the
    // development secret, at LEGACY_SEALED_AT and valid for 30 days after it.
    const LEGACY_SEALED_AT = 1792657249070;
    const legacyCookie =
      'Fe26.2**d4c4d8738f172456b12a7ef7c3238c869847f9ae2728b220299669859db4a4e1*smfQXfMsM_-gbfnFYmWRNg*qIjXEKHqV2MqhPLEGocSedrLp5lyiPROfBFuTmvJ0M4-Kgo9IW47Cdqhd1mLZ7FV4UrkkGHDrpUDq9lDsJcJY8aOHJtaF9W9kgBcGnUpaDCfO1JUUtA94RpjDozUgnatoaVdwLhfvF1X86m5Z0ozW56AMMJ2k3RJYCdp-BAlzlpocBUdgqxuSuimKDKSyNFQ7c-ypiKaOidMJtxsKJhrVP73BKEHYMV0jYYLkXT3ynd7Bgsgz-1xvcKhHhBkQCqHTZZvlJ2G5BxI1OfwxSqA53QZfeTmSfcSgjbC4LJd9_4R3kvKQ4nDwVGNBDz5xehW**ea37f0991835e41e828d7a57d3fbf01c1271795f8648188b5d1a8cdca4d7671c*w6uOUOF_bX9kbF0WcgAYJDcnLN0_XIo-ZwVHbPGdBqw';
    vi.useFakeTimers({ toFake: ['Date'] });
    vi.setSystemTime(LEGACY_SEALED_AT + 24 * 60 * 60 * 1000);
    browserCookies.set('srec_session', legacyCookie);
    startRequest();

    const session = await useAppSession();
    expect(session.data).toEqual({
      username: 'alice',
      token: {
        access_token: 'at-1',
        refresh_token: 'rt-1',
        expires_in: 1,
        refresh_expires_in: 2,
      },
      roles: ['admin'],
      mustChangePassword: false,
    });
  });
});
