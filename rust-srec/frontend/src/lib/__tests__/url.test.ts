import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { getMediaDownloadUrl, isSameOriginUrl, safeRedirectPath } from '../url';

// jsdom serves the tests from http://localhost:3000 by default.
describe('isSameOriginUrl', () => {
  it('accepts a relative media path', () => {
    expect(isSameOriginUrl('/api/media/abc/content?token=t')).toBe(true);
  });

  it('accepts an absolute URL on the page origin', () => {
    expect(
      isSameOriginUrl(`${window.location.origin}/api/media/abc/content`),
    ).toBe(true);
  });

  // The desktop build reaches the backend on its own origin, where an anchor's
  // `download` attribute is ignored and clicking would navigate away from the
  // application instead of saving the file.
  it.each([
    ['a different port', 'http://127.0.0.1:12555/api/media/abc/content'],
    ['a different host', 'https://media.example/api/media/abc/content'],
    ['a different scheme', 'https://localhost:3000/api/media/abc/content'],
  ])('rejects %s', (_label, url) => {
    expect(isSameOriginUrl(url)).toBe(false);
  });

  it('rejects a URL it cannot resolve', () => {
    expect(isSameOriginUrl('http://')).toBe(false);
  });
});

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

describe('getMediaDownloadUrl', () => {
  // The same-origin cases need a relative API base; a developer's `.env` or
  // shell may point the build at another origin.
  beforeEach(() => {
    vi.stubEnv('VITE_API_BASE_URL', '');
    vi.stubEnv('API_BASE_URL', '');
    vi.stubEnv('BACKEND_URL', '');
  });

  afterEach(() => {
    vi.unstubAllEnvs();
    delete (globalThis as { __RUST_SREC_BACKEND_URL__?: unknown })
      .__RUST_SREC_BACKEND_URL__;
  });

  it('leaves a same-origin URL to the anchor download attribute', () => {
    expect(getMediaDownloadUrl('abc', 'tok')).toBe(
      '/api/media/abc/content?token=tok',
    );
  });

  // An absolute API base — the desktop build and any split deployment — puts
  // the media route on another origin, where only the attachment header makes
  // the browser save the response instead of navigating to it.
  it('asks for an attachment when the media route is cross-origin', () => {
    (
      globalThis as { __RUST_SREC_BACKEND_URL__?: unknown }
    ).__RUST_SREC_BACKEND_URL__ = 'http://127.0.0.1:12555';

    expect(getMediaDownloadUrl('abc', 'tok')).toBe(
      'http://127.0.0.1:12555/api/media/abc/content?token=tok&download=1',
    );
  });

  it('escapes the output id', () => {
    expect(getMediaDownloadUrl('a/b')).toBe('/api/media/a%2Fb/content');
  });
});
