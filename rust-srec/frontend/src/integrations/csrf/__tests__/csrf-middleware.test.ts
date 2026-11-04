import { isRequestHostOrigin } from '../csrf-middleware';

describe('isRequestHostOrigin', () => {
  it.each([
    ['same host and port', 'http://nas:15275', { host: 'nas:15275' }, true],
    ['another port', 'http://nas:9000', { host: 'nas:15275' }, false],
    [
      'the client-facing X-Forwarded-Host',
      'http://nas:15275',
      { host: 'internal:3000', 'x-forwarded-host': 'nas:15275, proxy' },
      true,
    ],
    ['an opaque origin', 'null', { host: 'nas' }, false],
    ['a default port written out', 'https://nas', { host: 'nas:443' }, true],
    [
      'another scheme on the default port',
      'https://nas',
      { host: 'nas', 'x-forwarded-proto': 'http' },
      false,
    ],
  ])('%s', (_label, origin, headers, expected) => {
    const request = new Request('http://internal:3000/_serverFn/x', {
      headers,
    });
    expect(isRequestHostOrigin(origin, request)).toBe(expected);
  });
});
