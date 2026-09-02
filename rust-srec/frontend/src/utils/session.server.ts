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

// Use TanStack Start's server session in SSR deployments, but fall back
// to localStorage for pure-client builds (e.g. Tauri desktop).
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

  const { useSession, getRequestHeader } =
    await import('@tanstack/react-start/server');

  // getRequestHeader throws outside of a request context (e.g. prerendering),
  // which leaves resolveCookieSecurity on its NODE_ENV fallback.
  let forwardedProto: string | undefined;
  try {
    forwardedProto = getRequestHeader('x-forwarded-proto');
  } catch {
    forwardedProto = undefined;
  }

  const { secure, warnInsecure } = resolveCookieSecurity({
    cookieSecure: process.env.COOKIE_SECURE,
    forwardedProto,
    nodeEnv: process.env.NODE_ENV,
  });
  if (warnInsecure) warnInsecureCookieOnce();

  const session = await useSession<SessionData>({
    name: 'srec_session',
    password: resolveSessionSecret(),
    cookie: {
      secure,
      sameSite: 'lax',
      httpOnly: true,
      maxAge: 30 * 24 * 60 * 60, // 30 days
    },
  });

  return {
    get data() {
      return session.data;
    },
    async update(next: SessionData) {
      await session.update(next);
    },
    async clear() {
      await session.clear();
    },
  };
}
