import type { z } from 'zod';

export class BackendApiError extends Error {
  constructor(
    public status: number,
    public statusText: string,
    public body: unknown,
  ) {
    // Extract detailed message from body if available
    let detail: unknown;
    if (typeof body === 'object' && body !== null) {
      const fields = body as {
        message?: unknown;
        detail?: unknown;
        error?: unknown;
      };
      detail =
        fields.message || fields.detail || fields.error || JSON.stringify(body);
    } else {
      detail =
        typeof body === 'string' && body.length > 0
          ? body
          : `${status} ${statusText}`;
    }
    super(String(detail));
    this.name = 'BackendApiError';
  }
}

/** True when an error body carries the backend's `code` discriminator. */
export function hasErrorCode(body: unknown, code: string): boolean {
  return (
    typeof body === 'object' &&
    body !== null &&
    (body as { code?: unknown }).code === code
  );
}

/**
 * The body of a backend error. Read by shape, so any `Error` carrying a `body`
 * counts, not only a `BackendApiError` instance.
 */
export function errorBody(error: unknown): unknown {
  return error instanceof Error
    ? (error as { body?: unknown }).body
    : undefined;
}

/** The `details` object of a backend error body, when it has one. */
export function errorDetails(
  error: unknown,
): Record<string, unknown> | undefined {
  const body = errorBody(error);
  const value =
    typeof body === 'object' && body !== null
      ? (body as { details?: unknown }).details
      : undefined;
  return typeof value === 'object' && value !== null
    ? (value as Record<string, unknown>)
    : undefined;
}

/**
 * The `details.references` of a refused deletion, when the backend refused it
 * with `code` and they match `schema`.
 */
export function referencesFromConflict<T>(
  error: unknown,
  code: string,
  schema: z.ZodType<T>,
): T | undefined {
  if (!hasErrorCode(errorBody(error), code)) return undefined;
  const parsed = schema.safeParse(errorDetails(error)?.references);
  return parsed.success ? parsed.data : undefined;
}

/**
 * `code` of the error `loginFn` throws when the failure has no backend answer
 * to show (unreachable backend, unreadable response). The server cannot know
 * the browser's language, so the sign-in page supplies the message.
 */
export const LOGIN_FAILED_CODE = 'LOGIN_FAILED';

export function isLoginFailedError(error: unknown): boolean {
  return hasErrorCode(error, LOGIN_FAILED_CODE);
}

// Body code the backend attaches to 403 responses on every authenticated
// endpoint while the account's must_change_password flag is set (only
// /auth/change-password and /auth/logout-all are exempt).
const PASSWORD_CHANGE_REQUIRED_CODE = 'PASSWORD_CHANGE_REQUIRED';

export function hasPasswordChangeRequiredCode(body: unknown): boolean {
  return hasErrorCode(body, PASSWORD_CHANGE_REQUIRED_CODE);
}

// Matched by shape, like `errorBody`.
export function isPasswordChangeRequiredError(error: unknown): boolean {
  if (!(error instanceof Error)) return false;
  const { status, body } = error as Partial<BackendApiError>;
  return status === 403 && hasPasswordChangeRequiredCode(body);
}

/** True when the backend answered with the given HTTP status. */
function isBackendStatus(error: unknown, status: number): boolean {
  if (!(error instanceof Error)) return false;
  return (error as Partial<BackendApiError>).status === status;
}

/** True when the backend reported the requested resource does not exist. */
export function isNotFoundError(error: unknown): boolean {
  return isBackendStatus(error, 404);
}

// Body code the backend attaches to the 403 it returns once the account behind
// a credential has been deactivated. It is the only 403 `/auth/refresh` itself
// produces, which is how server/tokenRefresh.ts tells it apart from a 403
// injected by a proxy in front of the backend.
export const ACCOUNT_DISABLED_CODE = 'ACCOUNT_DISABLED';

// Body code the backend returns when a cancel or retry targets a DAG that
// already reached a terminal status. Mirrors DAG_ALREADY_TERMINAL_CODE in
// `api/error.rs`.
export const DAG_ALREADY_TERMINAL_CODE = 'DAG_ALREADY_TERMINAL';

/**
 * True when the backend rejected the action because the DAG had already
 * finished. Callers treat this as a no-op rather than a failure: a cancel
 * racing a DAG that completed a moment earlier got the outcome it wanted.
 */
export function isDagAlreadyTerminalError(error: unknown): boolean {
  if (!(error instanceof Error)) return false;
  const { body } = error as Partial<BackendApiError>;
  return hasErrorCode(body, DAG_ALREADY_TERMINAL_CODE);
}

/** The message of a thrown error, or `fallback` when it carries none. */
export function errorMessage(error: unknown, fallback: string): string {
  return error instanceof Error && error.message ? error.message : fallback;
}
