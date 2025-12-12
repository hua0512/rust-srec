import { createServerFn } from '@tanstack/react-start';
import { useAppSession } from '../../utils/session';
import { fetchBackend, BASE_URL } from '../api';
import {
    LoginRequestSchema,
    LoginResponseSchema,
    ChangePasswordRequestSchema
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

export const loginFn = createServerFn({ method: "POST" })
    .inputValidator((data: z.infer<typeof LoginRequestSchema>) => data)
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

            return userData;
        } catch (error) {
            console.error('Login failed:', error);
            // Re-throw so the UI knows it failed
            throw error;
        }
    });

export const logoutFn = createServerFn({ method: "POST" })
    .handler(async () => {
        const session = await useAppSession();
        const refreshToken = session.data.token?.refresh_token;

        if (refreshToken) {
            try {
                // Best effort logout on backend
                await authClient.post('auth/logout', {
                    json: { refresh_token: refreshToken }
                });
            } catch (e) {
                console.error('Backend logout failed (ignoring):', e);
            }
        }

        await session.clear();
        return { success: true };
    });

export const changePassword = createServerFn({ method: "POST" })
    .inputValidator((data: z.infer<typeof ChangePasswordRequestSchema>) => data)
    .handler(async ({ data }) => {
        // changePassword requires authentication, so we use fetchBackend 
        // which injects the current token.
        await fetchBackend('/auth/change-password', {
            method: 'POST',
            body: JSON.stringify(data)
        });
    });

export const checkAuthFn = createServerFn({ method: "POST" })
    .handler(async () => {
        const session = await useAppSession();
        const refreshToken = session.data.token?.refresh_token;

        if (!refreshToken) {
            return null;
        }

        const now = Date.now();
        const refreshExpiry = session.data.token?.refresh_expires_in ?? 0;
        if (now >= refreshExpiry) {
            await session.clear();
            return null;
        }

        try {
            const json = await authClient.post('auth/refresh', {
                json: { refresh_token: refreshToken }
            }).json();

            const parsed = LoginResponseSchema.parse(json);

            const userData = {
                username: session.data.username,
                token: {
                    access_token: parsed.access_token,
                    refresh_token: parsed.refresh_token,
                    expires_in: computeExpiryTimestamp(
                        parsed.expires_in,
                        session.data.token?.expires_in
                    ),
                    refresh_expires_in: computeExpiryTimestamp(
                        parsed.refresh_expires_in,
                        session.data.token?.refresh_expires_in
                    ),
                },
                roles: parsed.roles,
                mustChangePassword: parsed.must_change_password,
            };

            await session.update(userData);

            return userData;
        } catch (error) {
            console.warn('Token refresh failed, clearing session:', error);
            await session.clear();
            return null;
        }
    });
