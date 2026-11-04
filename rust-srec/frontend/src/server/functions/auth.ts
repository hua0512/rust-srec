import { createServerFn } from '@/server/createServerFn';
import { parseInput } from '../validate';
import { fetchBackend } from '../api';
import { BASE_URL } from '../../utils/env';
import { ensureValidToken } from '../tokenRefresh';
import { sanitizeClientSession, isValidSession } from '../../utils/session';
import { useAppSession } from '../../utils/session.server';
import {
  LoginRequestSchema,
  LoginResponseSchema,
  ChangePasswordRequestSchema,
} from '../../api/schemas/auth';
import { z } from 'zod';
import ky, { isHTTPError } from 'ky';
import { BackendApiError } from '../../lib/api-error';

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
 * message for HTTP failures and a fixed one for everything else.
 */
function toLoginError(error: unknown): Error {
  if (isHTTPError(error)) {
    return new BackendApiError(
      error.response.status,
      error.response.statusText,
      error.data,
    );
  }
  return new Error('Login failed');
}

function computeExpiryTimestamp(seconds?: number, fallback?: number): number {
  if (typeof seconds === 'number' && Number.isFinite(seconds)) {
    return Date.now() + seconds * 1000;
  }
  return fallback ?? Date.now();
}

export const loginFn = createServerFn({ method: 'POST' })
  .validator((data: z.infer<typeof LoginRequestSchema>) =>
    parseInput(LoginRequestSchema, data),
  )
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
  return { success: true };
});

export const changePassword = createServerFn({ method: 'POST' })
  .validator((data: z.infer<typeof ChangePasswordRequestSchema>) =>
    parseInput(ChangePasswordRequestSchema, data),
  )
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
  });

export const checkAuthFn = createServerFn({ method: 'POST' }).handler(
  async () => {
    // Use the global token refresh mechanism to ensure proper coordination
    // with other concurrent refresh attempts (e.g., from API 401 handling)
    const sessionData = await ensureValidToken();
    console.log(
      `[Auth] checkAuth result: ${sessionData ? `Authenticated as ${sessionData.username}` : 'Not authenticated'}`,
    );
    return sessionData;
  },
);
