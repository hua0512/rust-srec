import { refreshAuthTokenGlobal } from './tokenRefresh';

import { BASE_URL } from '../utils/env';
import { useAppSession } from '../utils/session.server';

import {
  BackendApiError,
  hasPasswordChangeRequiredCode,
} from '../lib/api-error';
import { isValidSession } from '../utils/session';
import { isDesktopBuild } from '../utils/desktop';

export { BackendApiError };

/** Default bound on one backend request; the retry after a refresh gets its own. */
export const BACKEND_TIMEOUT_MS = 15_000;

/**
 * For calls that run external tools or move many rows or files: engine and
 * proxy tests, BaiduPCS login, configuration backups and batch deletes.
 */
export const LONG_BACKEND_TIMEOUT_MS = 120_000;

/**
 * For calls that reach a streaming platform or a notification service. The
 * backend bounds platform operations, including the wait for a free platform
 * slot, at 5 minutes, and a notification channel's timeout goes up to the
 * same, so this only adds a margin for the answer to arrive.
 */
export const PLATFORM_BACKEND_TIMEOUT_MS = 310_000;

export interface FetchBackendInit extends RequestInit {
  /** Replaces {@link BACKEND_TIMEOUT_MS} for a call known to take longer. */
  timeoutMs?: number;
}

/**
 * The signal of the request this server function is answering, so that a
 * browser that navigates away or cancels the query also cancels the backend
 * call. Desktop builds call the backend from the browser and have none.
 */
async function incomingRequestSignal(): Promise<AbortSignal | undefined> {
  if (isDesktopBuild()) return undefined;
  try {
    const { getRequest } = await import('@tanstack/react-start/server');
    return getRequest().signal;
  } catch {
    // Called outside a request context.
    return undefined;
  }
}

/**
 * Parses the failure body and throws it as a BackendApiError. For the
 * backend's blanket 403 while must_change_password is set, first persists
 * mustChangePassword on the session so ensureValidToken and the /_authed
 * beforeLoad guard route to /change-password exactly as loginFn does.
 */
async function throwBackendError(response: Response): Promise<never> {
  let errorBody;
  const errorText = await response.text();
  try {
    errorBody = JSON.parse(errorText);
  } catch {
    errorBody = errorText;
  }

  if (response.status === 403 && hasPasswordChangeRequiredCode(errorBody)) {
    const session = await useAppSession();
    const data = session.data;
    if (isValidSession(data) && !data.mustChangePassword) {
      await session.update({ ...data, mustChangePassword: true });
    }
  }

  throw new BackendApiError(response.status, response.statusText, errorBody);
}

/**
 * Generic fetch wrapper for server-side calls to the backend.
 * Automatically injects the access token from the session.
 * On 401, attempts to refresh the token using the global refresh mechanism.
 */
export const fetchBackend = async <T = any>(
  endpoint: string,
  { timeoutMs = BACKEND_TIMEOUT_MS, ...init }: FetchBackendInit = {},
): Promise<T> => {
  const session = await useAppSession();
  const token = session.data.token?.access_token;

  const headers = new Headers(init.headers);
  if (token) {
    headers.set('Authorization', `Bearer ${token}`);
  }
  // Every server function passes through here, including the 5 s pollers,
  // so the request trace is development-only.
  if (import.meta.env.DEV) {
    console.log(
      `[API] ${init.method || 'GET'} ${endpoint} - ${token ? 'token present' : 'no token in session'}`,
    );
  }

  // Ensure Content-Type is set for JSON bodies if not already present
  if (
    init.body &&
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

  const clientSignal = await incomingRequestSignal();
  const requestSignal = () =>
    AbortSignal.any(
      [AbortSignal.timeout(timeoutMs), clientSignal, init.signal].filter(
        (signal): signal is AbortSignal => signal != null,
      ),
    );

  const response = await fetch(url, {
    ...init,
    headers,
    signal: requestSignal(),
  });

  if (import.meta.env.DEV) {
    console.log(
      `[API] ${init.method || 'GET'} ${endpoint} - Status: ${response.status}`,
    );
  }

  // Handle errors
  if (!response.ok) {
    if (response.status === 401) {
      console.log(`[API] 401 Unauthorized for ${url}. Attempting refresh...`);
      try {
        // Use the global refresh mechanism to prevent race conditions
        const refresh = await refreshAuthTokenGlobal();
        if (refresh.status === 'refreshed') {
          console.log(`[API] Token refreshed. Retrying ${url}...`);
          headers.set('Authorization', `Bearer ${refresh.accessToken}`);
          const retryResponse = await fetch(url, {
            ...init,
            headers,
            signal: requestSignal(),
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
          await throwBackendError(retryResponse);
        } else {
          // No usable token, so the request is not retried and the original
          // 401 is reported below. Session handling belongs to
          // refreshAuthTokenGlobal, which cleared it for 'rejected' and left
          // it intact for 'transient' or 'superseded'. In particular, an old
          // request must never be replayed as a newly signed-in user.
          console.log(
            `[API] Refresh ${refresh.status} for retry of ${endpoint}.`,
          );
        }
      } catch (refreshError) {
        if (refreshError instanceof BackendApiError) {
          throw refreshError;
        }
        console.error(
          '[API] Token refresh failed during interceptor:',
          refreshError,
        );
        // Fall through to throw original 401
      }
    }

    await throwBackendError(response);
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
