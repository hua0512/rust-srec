import { describe, expect, it } from 'vitest';

import { parseStoredSession } from './session-storage';

// `backendScope` is absent on sessions stored before scoping existed.
const stored = (backendScope?: string) =>
  JSON.stringify({ token: { access_token: 'token-a' }, backendScope });

describe('parseStoredSession', () => {
  it.each([
    [
      'drops a session from another remote server',
      stored('remote:https://a.example'),
      'remote:https://b.example',
      undefined,
    ],
    [
      'drops an unscoped legacy session on a remote server',
      stored(),
      'remote:https://a.example',
      undefined,
    ],
    [
      'keeps an unscoped legacy session on the local backend',
      stored(),
      'local',
      'token-a',
    ],
  ])('%s', (_, raw, scope, token) => {
    expect(parseStoredSession(raw, scope).token?.access_token).toBe(token);
  });
});
