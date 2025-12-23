import { useSession, getRequestHeader } from '@tanstack/react-start/server';

export type SessionData = {
  username: string;
  token: {
    access_token: string;
    refresh_token: string;
    // Stored as absolute timestamps (ms since epoch)
    expires_in: number;
    refresh_expires_in: number;
  };
  roles: string[];
  mustChangePassword: boolean;
};

// Determine if cookies should use the secure flag
// Priority:
// 1. COOKIE_SECURE env var: 'true' = always secure, 'false' = never secure
// 2. X-Forwarded-Proto header (when behind a reverse proxy)
// 3. Default: secure only in production
function isSecureCookie(): boolean {
  const cookieSecure = process.env.COOKIE_SECURE?.toLowerCase();
  if (cookieSecure === 'true') return true;
  if (cookieSecure === 'false') return false;

  // Check X-Forwarded-Proto header for reverse proxy HTTPS detection
  try {
    const forwardedProto = getRequestHeader('x-forwarded-proto');
    if (forwardedProto) {
      return forwardedProto.toLowerCase() === 'https';
    }
  } catch {
    // getRequestHeader may throw if called outside of a request context
    // Fall through to default behavior
  }

  // Default: secure only in production
  return process.env.NODE_ENV === 'production';
}

export function useAppSession() {
  return useSession<SessionData>({
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
}

