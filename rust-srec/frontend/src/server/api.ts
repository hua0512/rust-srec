import { refreshAuthTokenGlobal } from './tokenRefresh';

import { BASE_URL } from '../utils/env';
import { joinUrlPath } from '../lib/url';
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
 * browser that closes or reloads the tab, or leaves a page while it is being
 * server-rendered, also cancels the backend call. Moving between pages in the
 * app cancels nothing, because no server function call passes on the query's
 * signal. Desktop builds call the backend from the browser and have none.
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

/** Functions that release what a fallback signal holds once the call is done. */
type Cleanups = Array<() => void>;

function timeoutSignal(ms: number, cleanups: Cleanups): AbortSignal {
  if (typeof AbortSignal.timeout === 'function') return AbortSignal.timeout(ms);
  const controller = new AbortController();
  const timer = setTimeout(
    () => controller.abort(new DOMException('Timed out', 'TimeoutError')),
    ms,
  );
  cleanups.push(() => clearTimeout(timer));
  return controller.signal;
}

/**
 * One signal that aborts with the first of `signals`. The desktop app runs
 * this in the system WebView, and `AbortSignal.any` needs Safari 17.4 there.
 */
function anySignal(signals: AbortSignal[], cleanups: Cleanups): AbortSignal {
  if (signals.length === 1) return signals[0];
  if (typeof AbortSignal.any === 'function') return AbortSignal.any(signals);
  const controller = new AbortController();
  for (const signal of signals) {
    if (signal.aborted) {
      controller.abort(signal.reason);
      break;
    }
    // Removed when the call is done, so a long-lived caller signal does not
    // keep one listener per finished call.
    const onAbort = () => controller.abort(signal.reason);
    signal.addEventListener('abort', onAbort, { once: true });
    cleanups.push(() => signal.removeEventListener('abort', onAbort));
  }
  return controller.signal;
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
  const url = joinUrlPath(BASE_URL, endpoint);

  const clientSignal = await incomingRequestSignal();
  const cleanups: Cleanups = [];
  const requestSignal = () =>
    anySignal(
      [timeoutSignal(timeoutMs, cleanups), clientSignal, init.signal].filter(
        (signal): signal is AbortSignal => signal != null,
      ),
      cleanups,
    );

  try {
    return await sendWithRefresh();
  } finally {
    for (const cleanup of cleanups) cleanup();
  }

  async function sendWithRefresh(): Promise<T> {
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
      let failed = response;
      if (response.status === 401) {
        console.log(`[API] 401 Unauthorized for ${url}. Attempting refresh...`);
        // Read the 401 now: its signal's timeout keeps running during the
        // refresh and would otherwise fail the body read reported below.
        failed = new Response(await response.arrayBuffer(), {
          status: response.status,
          statusText: response.statusText,
          headers: response.headers,
        });

        let refresh: Awaited<ReturnType<typeof refreshAuthTokenGlobal>> | null =
          null;
        try {
          // Use the global refresh mechanism to prevent race conditions
          refresh = await refreshAuthTokenGlobal();
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

        if (refresh?.status === 'refreshed') {
          // Failures of the retry itself, an abort or timeout included, are
          // its own and propagate instead of being reported as the first 401.
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
        } else if (refresh) {
          // No usable token, so the request is not retried and the original
          // 401 is reported below. Session handling belongs to
          // refreshAuthTokenGlobal, which cleared it for 'rejected' and left
          // it intact for 'transient' or 'superseded'. In particular, an old
          // request must never be replayed as a newly signed-in user.
          console.log(
            `[API] Refresh ${refresh.status} for retry of ${endpoint}.`,
          );
        }
      }

      await throwBackendError(failed);
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
  }
};
