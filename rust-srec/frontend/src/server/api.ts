import { useAppSession, SessionData } from '../utils/session';

// Determine the base URL for the backend API.
// Priority:
// 1. process.env.API_BASE_URL (Runtime env var, ideal for Docker)
// 2. import.meta.env.VITE_API_BASE_URL (Build time env var)
// 3. Fallback to localhost default
const getBaseUrl = () => {
    if (typeof process !== 'undefined' && process.env.API_BASE_URL) {
        return process.env.API_BASE_URL;
    }
    return import.meta.env.VITE_API_BASE_URL || 'http://127.0.0.1:12555';
};

export const BASE_URL = getBaseUrl();

export class BackendApiError extends Error {
    constructor(public status: number, public statusText: string, public body: any) {
        super(`Backend API Error: ${status} ${statusText}`);
        this.name = 'BackendApiError';
    }
}

/**
 * Generic fetch wrapper for server-side calls to the backend.
 * Automatically injects the access token from the session.
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
    // We ensure BASE_URL has a trailing slash for the URL constructor if we treat endpoint as relative
    // But a safer way is string concatenation if we are sure about the format.
    // Let's rely on string ops to be explicit.
    const baseUrl = BASE_URL.endsWith('/') ? BASE_URL.slice(0, -1) : BASE_URL;
    const path = endpoint.startsWith('/') ? endpoint : `/${endpoint}`;
    const url = `${baseUrl}${path}`;

    // console.log(`[API] Fetching ${url} with token: ${token ? 'PRESENT' : 'MISSING'}`);

    const response = await fetch(url, {
        ...init,
        headers,
    });

    // Handle errors
    if (!response.ok) {
        if (response.status === 401) {
            console.log(`[API] 401 Unauthorized for ${url}. Attempting refresh...`);
            try {
                const newToken = await refreshAuthToken(session);
                if (newToken) {
                    console.log(`[API] Token refreshed. Retrying ${url}...`);
                    // Retry original request with new token
                    const headers = new Headers(init?.headers);
                    headers.set('Authorization', `Bearer ${newToken}`);
                    const retryResponse = await fetch(url, {
                        ...init,
                        headers,
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
                console.error("Token refresh failed during interceptor:", refreshError);
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

// Singleton promise to handle concurrent refreshes
let refreshPromise: Promise<string | null> | null = null;

async function refreshAuthToken(session: any): Promise<string | null> {
    const refreshToken = session.data.token?.refresh_token;
    if (!refreshToken) {
        console.log(`[API] No refresh token available in session.`);
        return null;
    }

    if (refreshPromise) {
        return refreshPromise;
    }

    refreshPromise = (async () => {
        try {
            console.log(`[API] Calling refresh endpoint...`);
            // We use fetch directly to avoid infinite loops if fetchBackend calls itself
            const baseUrl = BASE_URL.endsWith('/') ? BASE_URL.slice(0, -1) : BASE_URL;
            const response = await fetch(`${baseUrl}/auth/refresh`, {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ refresh_token: refreshToken }),
            });

            if (!response.ok) {
                throw new Error('Refresh failed');
            }

            const json = await response.json();
            const userData: SessionData = {
                username: session.data.username,
                token: {
                    access_token: json.access_token,
                    refresh_token: json.refresh_token || refreshToken, // Fallback to existing refresh token if not allowed to rotate
                    expires_in: json.expires_in,
                    refresh_expires_in: json.refresh_expires_in || session.data.token.refresh_expires_in,
                },
                roles: json.roles || session.data.roles,
                mustChangePassword: json.must_change_password !== undefined ? json.must_change_password : session.data.mustChangePassword
            };

            await session.update(userData);
            return json.access_token;
        } catch (error) {
            console.error('Failed to refresh token:', error);
            // Clear session on fatal refresh error
            await session.clear();
            return null;
        } finally {
            refreshPromise = null;
        }
    })();

    return refreshPromise;
}
