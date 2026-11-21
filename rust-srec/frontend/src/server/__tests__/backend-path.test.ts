import {
  backendPath,
  InvalidBackendPathError,
  withQuery,
} from '../backend-path';

describe('backendPath', () => {
  it('leaves a template without parameters untouched', () => {
    expect(backendPath`/streamers`).toBe('/streamers');
  });

  it('produces the plain path for identifiers that need no encoding', () => {
    const id = '0f6b2f7e-6c2f-4c1a-9e3f-2f0a1b8c7d55';
    expect(backendPath`/streamers/${id}`).toBe(`/streamers/${id}`);
    expect(backendPath`/config/platforms/${'BILIBILI'}`).toBe(
      '/config/platforms/BILIBILI',
    );
  });

  it('encodes every parameter of a multi-segment template', () => {
    expect(backendPath`/streamers/${'a b'}/filters/${'c/d'}`).toBe(
      '/streamers/a%20b/filters/c%2Fd',
    );
  });

  it.each([
    ['../x', '/streamers/..%2Fx'],
    ['a/b', '/streamers/a%2Fb'],
    ['?limit=', '/streamers/%3Flimit%3D'],
    ['#frag', '/streamers/%23frag'],
    ['a b', '/streamers/a%20b'],
    ['直播', '/streamers/%E7%9B%B4%E6%92%AD'],
  ])('confines %s to a single segment', (id, expected) => {
    expect(backendPath`/streamers/${id}`).toBe(expected);
  });

  it.each([
    ['an empty string', ''],
    ['a dot segment', '.'],
    ['a parent segment', '..'],
    ['undefined', undefined],
    ['null', null],
    ['a number', 42],
    ['an object', { toString: () => 'x' }],
  ])('rejects %s', (_label, value) => {
    expect(() => backendPath`/streamers/${value as unknown as string}`).toThrow(
      InvalidBackendPathError,
    );
  });

  it('rejects a parameter longer than a single segment may be', () => {
    expect(() => backendPath`/streamers/${'a'.repeat(513)}`).toThrow(
      InvalidBackendPathError,
    );
    expect(backendPath`/streamers/${'a'.repeat(512)}`).toHaveLength(
      '/streamers/'.length + 512,
    );
  });
});

describe('withQuery', () => {
  it('omits the separator when there is nothing to send', () => {
    expect(withQuery('/logging/files', new URLSearchParams())).toBe(
      '/logging/files',
    );
  });
});
