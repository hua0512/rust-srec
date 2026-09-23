import { parseStoredSession } from './session-storage';

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
