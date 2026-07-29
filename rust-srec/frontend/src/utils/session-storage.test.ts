import { parseStoredSession } from './session-storage';
import { resolveSessionSecret } from './session.server';

describe('parseStoredSession', () => {
  it('returns an object from valid JSON', () => {
    expect(parseStoredSession('{"username":"user"}')).toEqual({
      username: 'user',
    });
  });

  it.each([null, '', 'invalid', '[]', 'null'])(
    'rejects non-session input %p',
    (raw) => {
      expect(parseStoredSession(raw)).toEqual({});
    },
  );
});

describe('resolveSessionSecret', () => {
  it('uses the configured secret', () => {
    expect(resolveSessionSecret('configured-secret', 'production')).toBe(
      'configured-secret',
    );
  });

  it('allows the built-in secret outside production', () => {
    expect(resolveSessionSecret(undefined, 'development')).toContain(
      'dev_secret',
    );
  });

  it('rejects a missing production secret', () => {
    expect(() => resolveSessionSecret(undefined, 'production')).toThrow(
      'SESSION_SECRET must be set in production',
    );
  });
});
