import { describe, expect, it } from 'vitest';

import { parseBootFailure } from './fatal-screen';

describe('parseBootFailure', () => {
  it('keeps legacy string errors recoverable', () => {
    const failure = parseBootFailure('legacy startup error');

    expect(failure).toMatchObject({
      origin: 'native',
      stage: 'backend',
      kind: 'unknown',
      details: 'legacy startup error',
    });
  });

  it('ignores missing and malformed payloads', () => {
    expect(parseBootFailure(null)).toBeNull();
    expect(parseBootFailure({ title: 'Incomplete' })).toBeNull();
  });
});
