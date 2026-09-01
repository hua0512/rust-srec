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

/** True when an error body carries the backend's `code` discriminator. */
export function hasErrorCode(body: unknown, code: string): boolean {
  return (
    typeof body === 'object' &&
    body !== null &&
    (body as { code?: unknown }).code === code
  );
}

// Body code the backend attaches to 403 responses on every authenticated
// endpoint while the account's must_change_password flag is set (only
// /auth/change-password and /auth/logout-all are exempt).
export const PASSWORD_CHANGE_REQUIRED_CODE = 'PASSWORD_CHANGE_REQUIRED';

export function hasPasswordChangeRequiredCode(body: unknown): boolean {
  return hasErrorCode(body, PASSWORD_CHANGE_REQUIRED_CODE);
}

// Server functions rethrow BackendApiError across the serialization boundary
// as a plain Error that keeps own properties (status/body) but not the
// subclass prototype, so match by shape instead of instanceof.
export function isPasswordChangeRequiredError(error: unknown): boolean {
  if (!(error instanceof Error)) return false;
  const { status, body } = error as Partial<BackendApiError>;
  return status === 403 && hasPasswordChangeRequiredCode(body);
}

// Body code the backend returns when a cancel or retry targets a DAG that
// already reached a terminal status. Mirrors DAG_ALREADY_TERMINAL_CODE in
// `api/error.rs`.
export const DAG_ALREADY_TERMINAL_CODE = 'DAG_ALREADY_TERMINAL';

/**
 * True when the backend rejected the action because the DAG had already
 * finished. Callers treat this as a no-op rather than a failure: a cancel
 * racing a DAG that completed a moment earlier got the outcome it wanted.
 *
 * Shape-matched for the same reason as `isPasswordChangeRequiredError`.
 */
export function isDagAlreadyTerminalError(error: unknown): boolean {
  if (!(error instanceof Error)) return false;
  const { body } = error as Partial<BackendApiError>;
  return hasErrorCode(body, DAG_ALREADY_TERMINAL_CODE);
}
