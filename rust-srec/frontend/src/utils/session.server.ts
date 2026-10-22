import type { SessionData } from './session';
import { isDesktopBuild } from '@/utils/desktop';
import {
  BROWSER_SESSION_STORAGE_KEY,
  isBrowserRuntime,
  parseStoredSession,
} from './session-storage';

type SessionLike<T> = {
  data: Partial<T>;
  update: (data: T) => Promise<void>;
  clear: () => Promise<void>;
};

let browserSessionSingleton: SessionLike<SessionData> | null = null;

const SESSION_COOKIE_NAME = 'srec_session';
const SESSION_MAX_AGE_SECONDS = 30 * 24 * 60 * 60; // 30 days

type RequestSession = {
  data: Partial<SessionData>;
  // Bumped by every update and clear. A seal that finishes after a newer write
  // is stale and must not set the cookie, or a slower, older update (or one
  // made before a sign-out) would end up in the response.
  version: number;
};

// Server functions called during SSR run in the page's request, so one state
// per request keeps a write visible to later reads in it, such as the token
// re-read after a refresh, and decrypts the cookie only once.
const requestSessions = new WeakMap<Request, Promise<RequestSession>>();

export function resolveSessionSecret(
  sessionSecret = process.env.SESSION_SECRET,
  nodeEnv = process.env.NODE_ENV,
): string {
  if (sessionSecret) return sessionSecret;
  if (nodeEnv === 'production') {
    throw new Error('SESSION_SECRET must be set in production');
  }
  return 'dev_secret_must_be_at_least_32_chars_long_and_random';
}

export type CookieSecuritySignals = {
  /** Raw `COOKIE_SECURE` value, when the operator set one. */
  cookieSecure?: string;
  /** Raw `X-Forwarded-Proto` request header. */
  forwardedProto?: string;
  nodeEnv?: string;
};

export type CookieSecurity = {
  /** Value handed to the `secure` cookie attribute. */
  secure: boolean;
  /** True when a production deployment is about to issue a non-secure cookie. */
  warnInsecure: boolean;
};

// A proxy chain appends to `X-Forwarded-Proto`, so the scheme the client
// actually spoke is the first value.
function firstXForwardedProto(header: string | undefined): string | undefined {
  const first = header?.split(',')[0]?.trim().toLowerCase();
  return first || undefined;
}

/**
 * Decides the `secure` attribute of the `srec_session` cookie.
 *
 * Priority:
 * 1. `COOKIE_SECURE`: `true` = always secure, `false` = never secure.
 * 2. The scheme reported in `X-Forwarded-Proto`. Plain HTTP stays non-secure so
 *    LAN deployments without TLS can still sign in.
 * 3. `NODE_ENV`, i.e. secure in production when the header is absent or empty.
 */
export function resolveCookieSecurity({
  cookieSecure,
  forwardedProto,
  nodeEnv,
}: CookieSecuritySignals): CookieSecurity {
  const override = cookieSecure?.trim().toLowerCase();
  if (override === 'true') return { secure: true, warnInsecure: false };
  if (override === 'false') return { secure: false, warnInsecure: false };

  const proto = firstXForwardedProto(forwardedProto);
  const secure = proto ? proto === 'https' : nodeEnv === 'production';

  return { secure, warnInsecure: !secure && nodeEnv === 'production' };
}

let insecureCookieWarned = false;

/**
 * Logged from every request that would issue a non-secure production cookie,
 * but emitted only once per process so it does not flood the request log.
 */
export function warnInsecureCookieOnce(): void {
  if (insecureCookieWarned) return;
  insecureCookieWarned = true;
  console.warn(
    '[Session] Request arrived over plain HTTP, so the srec_session cookie is issued without the Secure attribute. ' +
      'Behind an HTTPS reverse proxy, make it send "X-Forwarded-Proto: https", or set COOKIE_SECURE=true.',
  );
}

function getBrowserSession(): SessionLike<SessionData> {
  if (browserSessionSingleton) return browserSessionSingleton;

  let data = parseStoredSession(
    window.localStorage.getItem(BROWSER_SESSION_STORAGE_KEY),
  );

  browserSessionSingleton = {
    get data() {
      return data;
    },
    async update(next: SessionData) {
      data = next;
      window.localStorage.setItem(
        BROWSER_SESSION_STORAGE_KEY,
        JSON.stringify(next),
      );
    },
    async clear() {
      data = {};
      window.localStorage.removeItem(BROWSER_SESSION_STORAGE_KEY);
    },
  };

  return browserSessionSingleton;
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

async function openCookieSession(
  sealed: string | undefined,
  unsealCookie: (sealed: string) => Promise<unknown>,
): Promise<RequestSession> {
  const state: RequestSession = { data: {}, version: 0 };
  if (!sealed) return state;
  try {
    // The payload is `{ data }`. Cookies from rust-srec 0.6 and earlier hold
    // `{ id, createdAt, data }` in the same Iron seal, so they stay valid.
    const unsealed = await unsealCookie(sealed);
    if (isObject(unsealed) && isObject(unsealed.data)) {
      state.data = unsealed.data as Partial<SessionData>;
    }
  } catch {
    // Expired, tampered with, or sealed with another SESSION_SECRET: the
    // request starts signed out and the next sign-in replaces the cookie.
  }
  return state;
}

// Use an encrypted cookie session (an Iron seal over Start's cookie helpers) in
// SSR deployments, but fall back to localStorage for pure-client builds (e.g.
// Tauri desktop).
export async function useAppSession(): Promise<SessionLike<SessionData>> {
  // Desktop SPA builds never have a server runtime. Always use localStorage.
  if (isDesktopBuild()) {
    if (!isBrowserRuntime()) {
      throw new Error(
        'Desktop session is only available in the browser runtime',
      );
    }
    return getBrowserSession();
  }

  // Web/SSR build: session is server-only.
  if (isBrowserRuntime()) {
    throw new Error('useAppSession is server-only outside desktop builds');
  }

  const [
    { defaults, seal, unseal },
    { deleteCookie, getCookie, getRequest, setCookie },
  ] = await Promise.all([
    import('iron-webcrypto'),
    import('@tanstack/react-start/server'),
  ]);

  // Throws outside a request context, as Start's cookie helpers do.
  const request = getRequest();

  const { secure, warnInsecure } = resolveCookieSecurity({
    cookieSecure: process.env.COOKIE_SECURE,
    forwardedProto: request.headers.get('x-forwarded-proto') ?? undefined,
    nodeEnv: process.env.NODE_ENV,
  });
  if (warnInsecure) warnInsecureCookieOnce();

  const password = resolveSessionSecret();
  // The seal expires with the cookie. Every update re-seals, which renews both.
  const sealOptions = { ...defaults, ttl: SESSION_MAX_AGE_SECONDS * 1000 };
  const cookieOptions = {
    path: '/',
    httpOnly: true,
    sameSite: 'lax',
    secure,
  } as const;

  let pending = requestSessions.get(request);
  if (!pending) {
    pending = openCookieSession(getCookie(SESSION_COOKIE_NAME), (sealed) =>
      unseal(sealed, password, sealOptions),
    );
    requestSessions.set(request, pending);
  }
  const state = await pending;

  return {
    get data() {
      return state.data;
    },
    async update(next: SessionData) {
      state.data = next;
      const version = ++state.version;
      const sealed = await seal({ data: next }, password, sealOptions);
      if (state.version !== version) return;
      setCookie(SESSION_COOKIE_NAME, sealed, {
        ...cookieOptions,
        maxAge: SESSION_MAX_AGE_SECONDS,
      });
    },
    async clear() {
      state.data = {};
      state.version += 1;
      deleteCookie(SESSION_COOKIE_NAME, cookieOptions);
    },
  };
}
