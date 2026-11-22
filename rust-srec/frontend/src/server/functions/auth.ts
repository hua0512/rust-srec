import { createServerFn } from '@/server/createServerFn';
import { validateWith } from '../validate';
import { fetchBackend } from '../api';
import { BASE_URL } from '../../utils/env';
import { ensureValidToken } from '../tokenRefresh';
import {
  sanitizeClientSession,
  isValidSession,
  type ClientSessionData,
  type SessionData,
} from '../../utils/session';
import { useAppSession } from '../../utils/session.server';
import { isDesktopBuild } from '../../utils/desktop';
import { isTauriRuntime } from '../../utils/tauri';
import {
  clearExplicitDesktopLogout,
  hasExplicitDesktopLogout,
  markExplicitDesktopLogout,
  requestDesktopAutoLogin,
} from '../../desktop/desktop-auth';
import {
  LoginRequestSchema,
  LoginResponseSchema,
  ChangePasswordRequestSchema,
} from '../../api/schemas/auth';
import ky, { isHTTPError } from 'ky';
import { BackendApiError, LOGIN_FAILED_CODE } from '../../lib/api-error';

// Dedicated ky instance for auth calls that might not need the bearer token from session (like login)
// or need to handle session updates manually.
const authClient = ky.create({
  prefix: BASE_URL,
  timeout: 10000,
});

/**
 * ky's HTTPError keeps the request options, so logging one prints the JSON
 * body: the password on login, the refresh token on logout. Log its status
 * only. Other errors (network, timeout) carry no body, so their message and
 * cause are safe and needed to diagnose an unreachable backend.
 */
function describeAuthError(error: unknown): string {
  if (isHTTPError(error)) return `HTTP ${error.response.status}`;
  if (!(error instanceof Error)) return 'unknown error';
  const cause = error.cause instanceof Error ? ` (${error.cause.message})` : '';
  return `${error.name}: ${error.message}${cause}`;
}

/**
 * The ky message names the internal backend URL; surface the backend's own
 * message for HTTP failures and `LOGIN_FAILED_CODE` for everything else.
 */
function toLoginError(error: unknown): Error {
  if (isHTTPError(error)) {
    return new BackendApiError(
      error.response.status,
      error.response.statusText,
      error.data,
    );
  }
  // A backend error body, so the code reaches the browser through the
  // server function's error serialization like any backend refusal.
  return new BackendApiError(502, 'Bad Gateway', {
    code: LOGIN_FAILED_CODE,
    message: 'Login failed',
  });
}

function computeExpiryTimestamp(seconds?: number, fallback?: number): number {
  if (typeof seconds === 'number' && Number.isFinite(seconds)) {
    return Date.now() + seconds * 1000;
  }
  return fallback ?? Date.now();
}

export const loginFn = createServerFn({ method: 'POST' })
  .validator(validateWith(LoginRequestSchema))
  .handler(async ({ data }) => {
    try {
      const json = await authClient.post('auth/login', { json: data }).json();
      const parsed = LoginResponseSchema.parse(json);
      const session = await useAppSession();

      const userData = {
        username: data.username,
        token: {
          access_token: parsed.access_token,
          refresh_token: parsed.refresh_token,
          expires_in: computeExpiryTimestamp(parsed.expires_in),
          refresh_expires_in: computeExpiryTimestamp(parsed.refresh_expires_in),
        },
        roles: parsed.roles,
        mustChangePassword: parsed.must_change_password,
      };
      await session.update(userData);

      // Do not leak refresh token back to the browser; return a sanitized view.
      if (!isValidSession(session.data)) {
        throw new Error('Session update failed or incomplete');
      }
      clearExplicitDesktopLogout();
      return sanitizeClientSession(session.data);
    } catch (error) {
      console.error('Login failed:', describeAuthError(error));
      throw toLoginError(error);
    }
  });

export const logoutFn = createServerFn({ method: 'POST' }).handler(async () => {
  const session = await useAppSession();
  const refreshToken = session.data.token?.refresh_token;

  if (refreshToken) {
    try {
      // Best effort logout on backend
      await authClient.post('auth/logout', {
        json: { refresh_token: refreshToken },
      });
    } catch (e) {
      console.error('Backend logout failed (ignoring):', describeAuthError(e));
    }
  }

  await session.clear();
  if (isDesktopBuild() && isTauriRuntime()) {
    markExplicitDesktopLogout();
  }
  return { success: true };
});

export const changePassword = createServerFn({ method: 'POST' })
  .validator(validateWith(ChangePasswordRequestSchema))
  .handler(async ({ data }) => {
    // changePassword requires authentication, so we use fetchBackend
    // which injects the current token.
    await fetchBackend('/auth/change-password', {
      method: 'POST',
      body: JSON.stringify(data),
    });

    // Clear the session since the backend revokes all tokens on password change
    // This forces the user to re-login with their new password
    const session = await useAppSession();
    await session.clear();
    if (isDesktopBuild() && isTauriRuntime()) {
      markExplicitDesktopLogout();
    }
  });

let inFlightDesktopAutoLogin: Promise<ClientSessionData | null> | null = null;

async function performDesktopAutoLogin(options?: {
  ignoreExplicitLogout?: boolean;
}): Promise<ClientSessionData | null> {
  try {
    const autoLogin = await requestDesktopAutoLogin();
    if (
      !autoLogin ||
      (!options?.ignoreExplicitLogout && hasExplicitDesktopLogout())
    ) {
      return null;
    }

    const session = await useAppSession();
    const userData: SessionData = {
      username: autoLogin.username,
      token: {
        access_token: autoLogin.accessToken,
        refresh_token: autoLogin.refreshToken,
        expires_in: computeExpiryTimestamp(autoLogin.expiresIn),
        refresh_expires_in: computeExpiryTimestamp(autoLogin.refreshExpiresIn),
      },
      roles: autoLogin.roles,
      mustChangePassword: autoLogin.mustChangePassword,
      autoLogin: true,
    };

    await session.update(userData);
    if (!isValidSession(session.data)) {
      return null;
    }
    return sanitizeClientSession(session.data);
  } catch (error) {
    console.error(
      'Desktop auto-login failed (falling back to login screen):',
      describeAuthError(error),
    );
    return null;
  }
}

function tryDesktopAutoLogin(options?: {
  ignoreExplicitLogout?: boolean;
}): Promise<ClientSessionData | null> {
  if (!isDesktopBuild() || !isTauriRuntime()) {
    return Promise.resolve(null);
  }

  if (inFlightDesktopAutoLogin) {
    return inFlightDesktopAutoLogin;
  }

  inFlightDesktopAutoLogin = performDesktopAutoLogin(options).finally(() => {
    inFlightDesktopAutoLogin = null;
  });
  return inFlightDesktopAutoLogin;
}

export const checkAuthFn = createServerFn({ method: 'POST' }).handler(
  async () => {
    // Use the global token refresh mechanism to ensure proper coordination
    // with other concurrent refresh attempts (e.g., from API 401 handling)
    let sessionData = await ensureValidToken();

    if (
      !sessionData &&
      isDesktopBuild() &&
      isTauriRuntime() &&
      !hasExplicitDesktopLogout()
    ) {
      sessionData = await tryDesktopAutoLogin();
    }

    console.log(
      `[Auth] checkAuth result: ${sessionData ? `Authenticated as ${sessionData.username}` : 'Not authenticated'}`,
    );
    return sessionData;
  },
);

export const desktopAutoLoginFn = createServerFn({ method: 'POST' }).handler(
  async () => {
    if (!isDesktopBuild() || !isTauriRuntime()) {
      return null;
    }

    const sessionData =
      (await ensureValidToken()) ??
      (await tryDesktopAutoLogin({ ignoreExplicitLogout: true }));
    if (sessionData) {
      clearExplicitDesktopLogout();
    }
    return sessionData;
  },
);

export const clearDesktopClientSessionFn = createServerFn({
  method: 'POST',
}).handler(async () => {
  if (!isDesktopBuild()) {
    return;
  }
  const session = await useAppSession();
  await session.clear();
});

export const clearDesktopAutoLoginSessionFn = createServerFn({
  method: 'POST',
}).handler(async () => {
  if (!isDesktopBuild()) {
    return false;
  }
  const session = await useAppSession();
  if (!session.data.autoLogin) {
    return false;
  }
  await session.clear();
  markExplicitDesktopLogout();
  return true;
});
