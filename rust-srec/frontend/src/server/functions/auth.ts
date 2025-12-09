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
                    expires_in: parsed.expires_in,
                    refresh_expires_in: parsed.refresh_expires_in,
                },
                roles: parsed.roles,
                mustChangePassword: parsed.must_change_password
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

        // check if the refresh token is expired
        // In a real app we might check local time vs expires_in, 
        // but here we just try to refresh if present? 
        // Logic from original auth.ts:

        // refresh_expires_in is likely seconds from issue, or absolute timestamp?
        // In rust code it's usually relative seconds? 
        // Original code: `const expiresAt = session.data.token?.refresh_expires_in || 0`
        // If it was stored as expiration timestamp it's fine. If relative, needs calculation.
        // Assuming original logic was correct or we should verify. 
        // Let's assume it works as is if we persisted it correctly.
        // But in `loginFn` above: `expires_in: parsed.expires_in`. 
        // If the backend returns relative seconds, we should theoretically convert to absolute time for storage if we want to check `now > expiresAt`.
        // However, `utils/session.ts` defines `expires_in: number`.
        // For now, let's keep the logic close to original but maybe trust the backend refresh call more.

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
                    expires_in: parsed.expires_in,
                    refresh_expires_in: parsed.refresh_expires_in,
                },
                roles: parsed.roles,
                mustChangePassword: parsed.must_change_password
            };

            await session.update(userData);

            return userData;
        } catch (error) {
            console.warn('Token refresh failed, clearing session:', error);
            await session.clear();
            return null;
        }
    });

