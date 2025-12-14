import { createServerFn } from '@tanstack/react-start';
import { fetchBackend } from '../api';
import { BASE_URL } from '../../utils/env';
import { ensureValidToken } from '../tokenRefresh';
import {
  LoginRequestSchema,
  LoginResponseSchema,
  ChangePasswordRequestSchema,
} from '../../api/schemas';
import { z } from 'zod';
import ky from 'ky';

// Dedicated ky instance for auth calls that might not need the bearer token from session (like login)
// or need to handle session updates manually.
const authClient = ky.create({
  prefixUrl: BASE_URL,
  timeout: 10000,
});

function computeExpiryTimestamp(seconds?: number, fallback?: number): number {
  if (typeof seconds === 'number' && Number.isFinite(seconds)) {
    return Date.now() + seconds * 1000;
  }
  return fallback ?? Date.now();
}

export const loginFn = createServerFn({ method: 'POST' })
  .inputValidator((data: z.infer<typeof LoginRequestSchema>) => data)
  .handler(async ({ data }) => {
    try {
      const json = await authClient.post('auth/login', { json: data }).json();
      const parsed = LoginResponseSchema.parse(json);

      const { useAppSession } = await import('../../utils/session');
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

      return userData;
    } catch (error) {
      console.error('Login failed:', error);
      // Re-throw so the UI knows it failed
      throw error;
    }
  });

export const logoutFn = createServerFn({ method: 'POST' }).handler(async () => {
  const { useAppSession } = await import('../../utils/session');
  const session = await useAppSession();
  const refreshToken = session.data.token?.refresh_token;

  if (refreshToken) {
    try {
      // Best effort logout on backend
      await authClient.post('auth/logout', {
        json: { refresh_token: refreshToken },
      });
    } catch (e) {
      console.error('Backend logout failed (ignoring):', e);
    }
  }

  await session.clear();
  return { success: true };
});

export const changePassword = createServerFn({ method: 'POST' })
  .inputValidator((data: z.infer<typeof ChangePasswordRequestSchema>) => data)
  .handler(async ({ data }) => {
    // changePassword requires authentication, so we use fetchBackend
    // which injects the current token.
    await fetchBackend('/auth/change-password', {
      method: 'POST',
      body: JSON.stringify(data),
    });

    // Update the server session to clear mustChangePassword flag
    // This ensures the _authed layout check uses the updated value
    // This ensures the _authed layout check uses the updated value
    const { useAppSession } = await import('../../utils/session');
    const session = await useAppSession();
    if (session.data) {
      await session.update({
        ...session.data,
        mustChangePassword: false,
      });
    }
  });

export const checkAuthFn = createServerFn({ method: 'POST' }).handler(
  async () => {
    // Use the global token refresh mechanism to ensure proper coordination
    // with other concurrent refresh attempts (e.g., from API 401 handling)
    const sessionData = await ensureValidToken();
    return sessionData;
  },
);
