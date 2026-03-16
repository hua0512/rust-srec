import type { SessionData } from './session';
import { isDesktopBuild } from '@/utils/desktop';

type SessionLike<T> = {
  data: Partial<T>;
  update: (data: T) => Promise<void>;
  clear: () => Promise<void>;
};

const BROWSER_SESSION_STORAGE_KEY = 'rust_srec_session_v1';

function isBrowserRuntime(): boolean {
  return (
    typeof window !== 'undefined' && typeof window.localStorage !== 'undefined'
  );
}

function parseStoredSession(raw: string | null): Partial<SessionData> {
  if (!raw) return {};

  try {
    const parsed = JSON.parse(raw) as unknown;
    if (typeof parsed !== 'object' || parsed === null) return {};
    return parsed as Partial<SessionData>;
  } catch {
    return {};
  }
}

let browserSessionSingleton: SessionLike<SessionData> | null = null;

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

  // Determine if cookies should use the secure flag
  // Priority:
  // 1. COOKIE_SECURE env var: 'true' = always secure, 'false' = never secure
  // 2. X-Forwarded-Proto header (when behind a reverse proxy)
  // 3. Default: secure only in production
  const isSecureCookie = (): boolean => {
    const cookieSecure = process.env.COOKIE_SECURE?.toLowerCase();
    if (cookieSecure === 'true') return true;
    if (cookieSecure === 'false') return false;

    // Check X-Forwarded-Proto header for reverse proxy HTTPS detection
    try {
      const forwardedProto = getRequestHeader('x-forwarded-proto');
      if (forwardedProto) {
        const isHttps = forwardedProto.toLowerCase() === 'https';
        console.log(
          `[Session] X-Forwarded-Proto: ${forwardedProto}, secure cookie: ${isHttps}`,
        );
        return isHttps;
      }
    } catch {
      // getRequestHeader may throw if called outside of a request context
      // Fall through to default behavior
    }

    // Default: secure only in production
    return process.env.NODE_ENV === 'production';
  };

  const session = await useSession<SessionData>({
    name: 'srec_session',
    password:
      process.env.SESSION_SECRET ||
      'dev_secret_must_be_at_least_32_chars_long_and_random',
    cookie: {
      secure: isSecureCookie(),
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
