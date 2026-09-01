import {
  BackendApiError,
  DAG_ALREADY_TERMINAL_CODE,
  hasErrorCode,
  isDagAlreadyTerminalError,
  isPasswordChangeRequiredError,
} from '../api-error';

/**
 * Server functions rethrow `BackendApiError` across the serialization
 * boundary as a plain `Error` that keeps `status`/`body` but loses the
 * subclass prototype, so every matcher has to work on this shape too.
 */
function rethrownAcrossBoundary(status: number, body: unknown): Error {
  const error = new Error('Request failed');
  return Object.assign(error, { status, body });
}

describe('hasErrorCode', () => {
  it('matches the backend code discriminator', () => {
    expect(hasErrorCode({ code: 'X' }, 'X')).toBe(true);
    expect(hasErrorCode({ code: 'Y' }, 'X')).toBe(false);
  });

  it('tolerates bodies that are not objects', () => {
    for (const body of [null, undefined, 'plain text', 42]) {
      expect(hasErrorCode(body, 'X')).toBe(false);
    }
  });
});

describe('isDagAlreadyTerminalError', () => {
  it('matches a BackendApiError carrying the code', () => {
    const error = new BackendApiError(422, 'Unprocessable Entity', {
      code: DAG_ALREADY_TERMINAL_CODE,
      message: 'DAG dag-1 is already in a terminal state',
    });

    expect(isDagAlreadyTerminalError(error)).toBe(true);
  });

  it('matches the shape that survives the server-function boundary', () => {
    const error = rethrownAcrossBoundary(422, {
      code: DAG_ALREADY_TERMINAL_CODE,
    });

    expect(error).not.toBeInstanceOf(BackendApiError);
    expect(isDagAlreadyTerminalError(error)).toBe(true);
  });

  it('does not match other validation failures', () => {
    const error = rethrownAcrossBoundary(422, {
      code: 'VALIDATION_ERROR',
      // Same status and wording family as the terminal-state rejection used
      // to have; only the code distinguishes them now.
      message: 'DAG dag-1 is already in a terminal state',
    });

    expect(isDagAlreadyTerminalError(error)).toBe(false);
  });

  it('does not match non-errors', () => {
    expect(isDagAlreadyTerminalError(undefined)).toBe(false);
    expect(isDagAlreadyTerminalError({ body: { code: 'X' } })).toBe(false);
  });
});

describe('isPasswordChangeRequiredError', () => {
  it('still requires both the 403 status and the code', () => {
    expect(
      isPasswordChangeRequiredError(
        rethrownAcrossBoundary(403, { code: 'PASSWORD_CHANGE_REQUIRED' }),
      ),
    ).toBe(true);
    expect(
      isPasswordChangeRequiredError(
        rethrownAcrossBoundary(422, { code: 'PASSWORD_CHANGE_REQUIRED' }),
      ),
    ).toBe(false);
  });
});
