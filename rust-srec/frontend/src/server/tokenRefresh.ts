/**
 * Global token refresh coordination module.
 * 
 * This module provides a centralized mechanism for refreshing authentication tokens
 * to prevent race conditions when multiple concurrent requests detect an expired token.
 * 
 * The key insight is that token rotation on the backend immediately revokes the old
 * refresh token, so we must ensure only ONE refresh attempt happens at a time globally.
 */

import { useAppSession, SessionData } from '../utils/session';
import { BASE_URL } from './api';

// Global singleton promise to coordinate ALL refresh attempts across the application
let globalRefreshPromise: Promise<string | null> | null = null;

// Track the last refresh token we successfully used, to detect if session was updated by another refresh
let lastKnownRefreshToken: string | null = null;

/**
 * Attempt to refresh the authentication token.
 * 
 * This function ensures that only one refresh request is in flight at any time,
 * preventing race conditions that could occur when:
 * 1. Multiple API calls fail with 401 simultaneously
 * 2. checkAuthFn runs concurrently with API calls
 * 
 * @returns The new access token if successful, null if refresh failed
 */
export async function refreshAuthTokenGlobal(): Promise<string | null> {
    const session = await useAppSession();
    const currentRefreshToken = session.data.token?.refresh_token;

    if (!currentRefreshToken) {
        console.log('[TokenRefresh] No refresh token available in session.');
        return null;
    }

    // If a refresh is already in progress, wait for it
    if (globalRefreshPromise) {
        console.log('[TokenRefresh] Refresh already in progress, waiting...');
        const result = await globalRefreshPromise;

        // After waiting, check if the session was updated with a new token
        const updatedSession = await useAppSession();
        const newAccessToken = updatedSession.data.token?.access_token;

        if (newAccessToken && updatedSession.data.token?.refresh_token !== currentRefreshToken) {
            console.log('[TokenRefresh] Session was updated by concurrent refresh, using new token.');
            return newAccessToken;
        }

        return result;
    }

    // Check if we're trying to refresh with a token we already know was replaced
    if (lastKnownRefreshToken && currentRefreshToken === lastKnownRefreshToken) {
        // The session might not have been updated yet, re-read it
        const freshSession = await useAppSession();
        if (freshSession.data.token?.refresh_token !== currentRefreshToken) {
            console.log('[TokenRefresh] Token was already rotated, using new access token.');
            return freshSession.data.token?.access_token ?? null;
        }
    }

    // Start a new refresh
    globalRefreshPromise = performRefresh(session, currentRefreshToken);

    try {
        const result = await globalRefreshPromise;
        return result;
    } finally {
        globalRefreshPromise = null;
    }
}

/**
 * Perform the actual token refresh.
 */
async function performRefresh(session: any, refreshToken: string): Promise<string | null> {
    try {
        console.log('[TokenRefresh] Calling refresh endpoint...');

        const baseUrl = BASE_URL.endsWith('/') ? BASE_URL.slice(0, -1) : BASE_URL;
        const response = await fetch(`${baseUrl}/auth/refresh`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ refresh_token: refreshToken }),
        });

        if (!response.ok) {
            // Check if this might be because another refresh already happened
            const freshSession = await useAppSession();
            if (freshSession.data.token?.refresh_token !== refreshToken) {
                console.log('[TokenRefresh] Token was rotated by another request, using new token.');
                return freshSession.data.token?.access_token ?? null;
            }

            console.error(`[TokenRefresh] Refresh failed with status: ${response.status}`);
            throw new Error(`Refresh failed: ${response.status}`);
        }

        const json = await response.json();
        const now = Date.now();

        const computedAccessExpiry =
            typeof json.expires_in === 'number' && Number.isFinite(json.expires_in)
                ? now + json.expires_in * 1000
                : session.data.token?.expires_in;

        const computedRefreshExpiry =
            typeof json.refresh_expires_in === 'number' && Number.isFinite(json.refresh_expires_in)
                ? now + json.refresh_expires_in * 1000
                : session.data.token?.refresh_expires_in;

        const userData: SessionData = {
            username: session.data.username,
            token: {
                access_token: json.access_token,
                refresh_token: json.refresh_token || refreshToken,
                expires_in: computedAccessExpiry ?? now,
                refresh_expires_in: computedRefreshExpiry ?? now,
            },
            roles: json.roles || session.data.roles,
            mustChangePassword:
                json.must_change_password !== undefined
                    ? json.must_change_password
                    : session.data.mustChangePassword,
        };

        await session.update(userData);

        // Track the old refresh token so we can detect if it was used again
        lastKnownRefreshToken = refreshToken;

        console.log('[TokenRefresh] Token refreshed successfully.');
        return json.access_token;
    } catch (error) {
        console.error('[TokenRefresh] Failed to refresh token:', error);

        // Before clearing session, check if another refresh succeeded
        const freshSession = await useAppSession();
        if (freshSession.data.token?.refresh_token !== refreshToken) {
            console.log('[TokenRefresh] Token was rotated by another request during error, using new token.');
            return freshSession.data.token?.access_token ?? null;
        }

        // Clear session on fatal refresh error
        await session.clear();
        return null;
    }
}

/**
 * Check if a valid access token exists and is not expired.
 * If expired, attempt to refresh.
 * 
 * @returns SessionData if authenticated, null if not authenticated
 */
export async function ensureValidToken(): Promise<SessionData | null> {
    const session = await useAppSession();
    const token = session.data.token;
    const username = session.data.username;

    // Check if we have the minimum required data
    if (!token?.refresh_token || !username) {
        return null;
    }

    const now = Date.now();

    // Check if refresh token is expired
    const refreshExpiry = token.refresh_expires_in ?? 0;
    if (now >= refreshExpiry) {
        console.log('[TokenRefresh] Refresh token expired, clearing session.');
        await session.clear();
        return null;
    }

    // Check if access token is expired or about to expire (with 30s buffer)
    const accessExpiry = token.expires_in ?? 0;
    if (now >= accessExpiry - 30000) {
        console.log('[TokenRefresh] Access token expired or expiring soon, refreshing...');
        const newAccessToken = await refreshAuthTokenGlobal();

        if (!newAccessToken) {
            return null;
        }

        // Re-read session to get updated data
        const updatedSession = await useAppSession();
        const updatedData = updatedSession.data;

        // Type guard: ensure we have all required fields
        if (!updatedData.username || !updatedData.token || !updatedData.roles) {
            return null;
        }

        return {
            username: updatedData.username,
            token: updatedData.token,
            roles: updatedData.roles,
            mustChangePassword: updatedData.mustChangePassword ?? false,
        };
    }

    // Type guard: ensure we have all required fields
    if (!session.data.roles) {
        return null;
    }

    return {
        username: username,
        token: token,
        roles: session.data.roles,
        mustChangePassword: session.data.mustChangePassword ?? false,
    };
}

