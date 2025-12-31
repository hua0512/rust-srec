import { refreshAuthTokenGlobal } from './tokenRefresh';

import { BASE_URL } from '../utils/env';

export { BASE_URL };

export class BackendApiError extends Error {
  constructor(
    public status: number,
    public statusText: string,
    public body: any,
  ) {
    // Extract detailed message from body if available
    const detail =
      typeof body === 'object' && body !== null
        ? body.message || body.detail || body.error || JSON.stringify(body)
        : typeof body === 'string' && body.length > 0
          ? body
          : `${status} ${statusText}`;
    super(detail);
    this.name = 'BackendApiError';
  }
}

/**
 * Generic fetch wrapper for server-side calls to the backend.
 * Automatically injects the access token from the session.
 * On 401, attempts to refresh the token using the global refresh mechanism.
 */
export const fetchBackend = async <T = any>(
  endpoint: string,
  init?: RequestInit,
): Promise<T> => {
  const { useAppSession } = await import('../utils/session');
  const session = await useAppSession();
  const token = session.data.token?.access_token;

  const headers = new Headers(init?.headers);
  if (token) {
    headers.set('Authorization', `Bearer ${token}`);
    console.log(`[API] ${init?.method || 'GET'} ${endpoint} - Token present: ${token.slice(0, 10)}...`);
  } else {
    console.log(`[API] ${init?.method || 'GET'} ${endpoint} - No token found in session.`);
  }

  // Ensure Content-Type is set for JSON bodies if not already present
  if (
    init?.body &&
    !headers.has('Content-Type') &&
    typeof init.body === 'string'
  ) {
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

  console.log(`[API] ${init?.method || 'GET'} ${endpoint} - Status: ${response.status}`);

  // Handle errors
  if (!response.ok) {
    if (response.status === 401) {
      console.log(`[API] 401 Unauthorized for ${url}. Attempting refresh...`);
      try {
        // Use the global refresh mechanism to prevent race conditions
        const newToken = await refreshAuthTokenGlobal();
        if (newToken) {
          console.log(`[API] Token refreshed. Retrying ${url}...`);
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
          console.log(
            `[API] Retry failed with status: ${retryResponse.status}`,
          );
          // If retry failed, throw error from retry response
          let errorBody;
          const errorText = await retryResponse.text();
          try {
            errorBody = JSON.parse(errorText);
          } catch {
            errorBody = errorText;
          }
          throw new BackendApiError(
            retryResponse.status,
            retryResponse.statusText,
            errorBody,
          );
        } else {
          console.log(`[API] Refresh failed or returned no token for retry of ${endpoint}.`);
        }
      } catch (refreshError) {
        console.error(
          '[API] Token refresh failed during interceptor:',
          refreshError,
        );
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
