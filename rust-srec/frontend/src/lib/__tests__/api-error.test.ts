import { z } from 'zod';
import {
  BackendApiError,
  DAG_ALREADY_TERMINAL_CODE,
  errorBody,
  errorDetails,
  hasErrorCode,
  isDagAlreadyTerminalError,
  isNotFoundError,
  isPasswordChangeRequiredError,
  referencesFromConflict,
} from '../api-error';

function backendError(status: number, body: unknown): BackendApiError {
  return new BackendApiError(status, 'Error', body);
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

  it('does not match other validation failures', () => {
    const error = backendError(422, {
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

describe('isNotFoundError', () => {
  it('matches a BackendApiError with a 404 status', () => {
    expect(
      isNotFoundError(new BackendApiError(404, 'Not Found', { message: 'no' })),
    ).toBe(true);
  });

  it('does not match other failures', () => {
    expect(isNotFoundError(backendError(500, {}))).toBe(false);
    expect(isNotFoundError(new Error('Network request failed'))).toBe(false);
    expect(isNotFoundError(undefined)).toBe(false);
    expect(isNotFoundError({ status: 404 })).toBe(false);
  });
});

describe('isPasswordChangeRequiredError', () => {
  it('still requires both the 403 status and the code', () => {
    expect(
      isPasswordChangeRequiredError(
        backendError(403, { code: 'PASSWORD_CHANGE_REQUIRED' }),
      ),
    ).toBe(true);
    expect(
      isPasswordChangeRequiredError(
        backendError(422, { code: 'PASSWORD_CHANGE_REQUIRED' }),
      ),
    ).toBe(false);
  });
});

describe('error body helpers', () => {
  it('reads the body and its details from a backend error', () => {
    const body = { code: 'X', details: { name: 'proxy-a' } };
    const error = backendError(409, body);
    expect(errorBody(error)).toBe(body);
    expect(errorDetails(error)).toEqual({ name: 'proxy-a' });
  });

  it('has no details without an error or an object body', () => {
    expect(errorBody('not an error')).toBeUndefined();
    expect(errorDetails(backendError(500, 'text'))).toBeUndefined();
    expect(
      errorDetails(backendError(409, { details: 'text' })),
    ).toBeUndefined();
  });
});

describe('referencesFromConflict', () => {
  const schema = z.object({ names: z.array(z.string()) });
  const refused = (code: string, references: unknown) =>
    backendError(409, { code, details: { references } });

  it('parses the references of the matching code', () => {
    expect(
      referencesFromConflict(
        refused('IN_USE', { names: ['a'] }),
        'IN_USE',
        schema,
      ),
    ).toEqual({ names: ['a'] });
  });

  it('ignores another code or references of another shape', () => {
    expect(
      referencesFromConflict(
        refused('OTHER', { names: ['a'] }),
        'IN_USE',
        schema,
      ),
    ).toBeUndefined();
    expect(
      referencesFromConflict(
        refused('IN_USE', { names: 'a' }),
        'IN_USE',
        schema,
      ),
    ).toBeUndefined();
  });
});
