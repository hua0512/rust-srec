import { useAppSession } from '../utils/session';
import { refreshAuthTokenGlobal } from './tokenRefresh';

// Determine the base URL for the backend API.
// Priority:
// 1. process.env.API_BASE_URL (Runtime env var, ideal for Docker)
// 2. import.meta.env.VITE_API_BASE_URL (Build time env var)
// 3. Fallback to localhost default
const getBaseUrl = () => {
    if (typeof process !== 'undefined' && process.env.API_BASE_URL) {
        return process.env.API_BASE_URL;
    }
    return import.meta.env.VITE_API_BASE_URL || 'http://127.0.0.1:12555/api';
};

export const BASE_URL = getBaseUrl();

export class BackendApiError extends Error {
    constructor(public status: number, public statusText: string, public body: any) {
        // Extract detailed message from body if available
        const detail = typeof body === 'object' && body !== null
            ? (body.message || body.detail || body.error || JSON.stringify(body))
            : (typeof body === 'string' && body.length > 0 ? body : `${status} ${statusText}`);
        super(detail);
        this.name = 'BackendApiError';
    }
}

/**
 * Generic fetch wrapper for server-side calls to the backend.
 * Automatically injects the access token from the session.
 * On 401, attempts to refresh the token using the global refresh mechanism.
 */
export const fetchBackend = async <T = any>(endpoint: string, init?: RequestInit): Promise<T> => {
    const session = await useAppSession();
    const token = session.data.token?.access_token;

    const headers = new Headers(init?.headers);
    if (token) {
        headers.set('Authorization', `Bearer ${token}`);
    }

    // Ensure Content-Type is set for JSON bodies if not already present
    if (init?.body && !headers.has('Content-Type') && typeof init.body === 'string') {
        try {
            JSON.parse(init.body);
            headers.set('Content-Type', 'application/json');
        } catch {
            // not json
        }
    }

    // Construct URL
    const baseUrl = BASE_URL.endsWith('/') ? BASE_URL.slice(0, -1) : BASE_URL;
    const path = endpoint.startsWith('/') ? endpoint : `/${endpoint}`;
    const url = `${baseUrl}${path}`;

    const response = await fetch(url, {
        ...init,
        headers,
    });

    // Handle errors
    if (!response.ok) {
        if (response.status === 401) {
            console.log(`[API] 401 Unauthorized for ${url}. Attempting refresh...`);
            try {
                // Use the global refresh mechanism to prevent race conditions
                const newToken = await refreshAuthTokenGlobal();
                if (newToken) {
                    console.log(`[API] Token refreshed. Retrying ${url}...`);
                    // Retry original request with new token
                    const retryHeaders = new Headers(init?.headers);
                    retryHeaders.set('Authorization', `Bearer ${newToken}`);
                    const retryResponse = await fetch(url, {
                        ...init,
                        headers: retryHeaders,
                    });

                    if (retryResponse.ok) {
                        // Return JSON if possible from retry
                        const contentType = retryResponse.headers.get('content-type');
                        if (contentType && contentType.includes('application/json')) {
                            return retryResponse.json();
                        }
                        if (retryResponse.status === 204) {
                            return null as T;
                        }
                        return retryResponse.text() as unknown as T;
                    }
                    console.log(`[API] Retry failed with status: ${retryResponse.status}`);
                    // If retry failed, throw error from retry response
                    let errorBody;
                    const errorText = await retryResponse.text();
                    try {
                        errorBody = JSON.parse(errorText);
                    } catch {
                        errorBody = errorText;
                    }
                    throw new BackendApiError(retryResponse.status, retryResponse.statusText, errorBody);
                } else {
                    console.log(`[API] Refresh failed or returned no token.`);
                }
            } catch (refreshError) {
                console.error("[API] Token refresh failed during interceptor:", refreshError);
                // Fall through to throw original 401
            }
        }

        let errorBody;
        const errorText = await response.text();
        try {
            errorBody = JSON.parse(errorText);
        } catch {
            errorBody = errorText;
        }
        throw new BackendApiError(response.status, response.statusText, errorBody);
    }

    // Handle empty responses
    if (response.status === 204) {
        return null as T;
    }

    // Return JSON if possible
    const contentType = response.headers.get('content-type');
    if (contentType && contentType.includes('application/json')) {
        return response.json();
    }

    return response.text() as unknown as T;
};
