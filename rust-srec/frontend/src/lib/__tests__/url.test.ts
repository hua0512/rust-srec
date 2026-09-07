import { describe, expect, it } from 'vitest';

import { safeRedirectPath } from '../url';

describe('safeRedirectPath', () => {
  it('keeps a path on this application, search string included', () => {
    expect(safeRedirectPath('/sessions')).toBe('/sessions');
    expect(safeRedirectPath('/sessions?page=2&status=active')).toBe(
      '/sessions?page=2&status=active',
    );
    expect(safeRedirectPath('/pipeline/jobs/abc#logs')).toBe(
      '/pipeline/jobs/abc#logs',
    );
  });

  it.each([
    ['an absolute URL', 'https://evil.example/steal'],
    ['a protocol-relative URL', '//evil.example/steal'],
    ['a backslash-escaped origin', '/\\evil.example/steal'],
    ['a javascript URL', 'javascript:alert(1)'],
    ['a relative path', 'dashboard'],
    ['an empty string', ''],
    ['a non-string', 42],
    ['nothing at all', undefined],
  ])('rejects %s', (_label, value) => {
    expect(safeRedirectPath(value)).toBeNull();
  });

  it('rejects a path carrying characters browsers strip from URLs', () => {
    // `/\t/evil.example` reads as `//evil.example` once the tab is removed.
    expect(safeRedirectPath('/\t/evil.example')).toBeNull();
    expect(safeRedirectPath('/\n/evil.example')).toBeNull();
    expect(safeRedirectPath('/\r/evil.example')).toBeNull();
  });
});
