import { expect, it } from 'vitest';
import { ParseUrlRequestSchema, ResolveUrlRequestSchema } from '../system';

it('managed parse rejects raw authentication and resolve accepts only client-owned media', () => {
  expect(
    ParseUrlRequestSchema.safeParse({
      url: 'https://example.invalid',
      credential_id: 'a',
      cookies: '',
    }).success,
  ).toBe(false);
  expect(
    ParseUrlRequestSchema.safeParse({
      url: 'https://example.invalid',
      credential_id: 'a',
    }).success,
  ).toBe(true);
  const clientMedia = { url: 'https://example.invalid', stream_info: {} };
  expect(ResolveUrlRequestSchema.parse(clientMedia)).toEqual(clientMedia);
  for (const extra of [{ playback_handle: 'opaque' }, { credential_id: 'b' }])
    expect(
      ResolveUrlRequestSchema.safeParse({ ...clientMedia, ...extra }).success,
    ).toBe(false);
});
