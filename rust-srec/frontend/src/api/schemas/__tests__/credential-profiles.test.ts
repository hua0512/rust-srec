import { describe, expect, it } from 'vitest';
import {
  CredentialSelectionSchema,
  CredentialProfileUpdateSchema,
  CredentialLoginTargetSchema,
  CredentialProfileSummarySchema,
  UnavailableReasonSchema,
} from '../credential-profiles';
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

describe('account selection boundary', () => {
  it('accepts every mode and preserves ordered one-member pools', () => {
    for (const selection of [
      { mode: 'inherit' },
      { mode: 'none' },
      { mode: 'fixed', credential_id: 'a' },
      ...['round_robin', 'priority'].map((strategy) => ({
        mode: 'pool',
        credential_ids: ['b', 'a'],
        strategy,
        failover: true,
        max_attempts: 3,
      })),
    ]) {
      expect(CredentialSelectionSchema.parse(selection)).toEqual(selection);
    }
    expect(
      CredentialSelectionSchema.parse({
        mode: 'pool',
        credential_ids: ['a'],
        strategy: 'priority',
        failover: false,
        max_attempts: 1,
      }).mode,
    ).toBe('pool');
  });
  it('rejects null, unknown fields, blank or duplicate IDs and invalid attempt budgets', () => {
    const base = {
      mode: 'pool',
      credential_ids: ['a'],
      strategy: 'priority',
      failover: true,
      max_attempts: 3,
    };
    for (const invalid of [
      null,
      { mode: 'inherit', cookies: 'secret' },
      { mode: 'fixed', credential_id: ' ' },
      { ...base, credential_ids: [] },
      { ...base, credential_ids: ['a', 'a'] },
      { ...base, max_attempts: 0 },
      { ...base, max_attempts: 11 },
      { ...base, strategy: 'random' },
    ]) {
      expect(CredentialSelectionSchema.safeParse(invalid).success).toBe(false);
    }
  });
  it('metadata changes omit replacement material', () => {
    expect(
      CredentialProfileUpdateSchema.parse({
        id: 'a',
        expected_version: 2,
        label: 'Renamed',
      }),
    ).toEqual({ id: 'a', expected_version: 2, label: 'Renamed' });
    expect(
      CredentialProfileUpdateSchema.safeParse({
        id: 'a',
        expected_version: 2,
        replacement: { cookies: '' },
      }).success,
    ).toBe(false);
  });
  it('requires a version for replacement login and strips secrets from summaries', () => {
    expect(
      CredentialLoginTargetSchema.safeParse({
        type: 'replace',
        profile_id: 'a',
      }).success,
    ).toBe(false);
    const summary = CredentialProfileSummarySchema.parse({
      id: 'a',
      label: 'A',
      enabled: true,
      version: 1,
      has_cookies: true,
      cookies: 'secret-sentinel',
    });
    expect(JSON.stringify(summary)).not.toContain('secret-sentinel');
  });
});

describe('unavailable reasons', () => {
  it('keeps the reasons the backend reports', () => {
    for (const reason of [
      'login_required',
      'profiles_disabled',
      'bound_profile_unavailable',
      'binding_policy_changed',
      'attempts_exhausted',
    ])
      expect(UnavailableReasonSchema.parse(reason)).toBe(reason);
  });

  it('reads a reason this build does not know as unknown', () => {
    expect(UnavailableReasonSchema.parse('quota_exceeded')).toBe('unknown');
    expect(UnavailableReasonSchema.nullable().parse(null)).toBeNull();
  });
});
