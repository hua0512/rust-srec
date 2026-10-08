import {
  ATTENTION_REFRESH_FAILURES,
  VALIDITY_STYLES,
  attentionReason,
  validityStyle,
} from '../account-health';
import { contextualAction } from '../account-row';
import { accountDetail, profile } from './credential-test-utils';

describe('attentionReason', () => {
  it('mirrors what the header button lists', () => {
    expect(attentionReason({ validity: 'invalid' })).toBe('login_required');
    expect(
      attentionReason({
        validity: 'needs_refresh',
        refresh_failure_count: ATTENTION_REFRESH_FAILURES,
      }),
    ).toBe('refresh_failing');
    expect(
      attentionReason({
        validity: 'needs_refresh',
        refresh_failure_count: ATTENTION_REFRESH_FAILURES - 1,
      }),
    ).toBeUndefined();
    expect(attentionReason({ validity: 'needs_refresh' })).toBeUndefined();
    expect(attentionReason({ validity: 'valid' })).toBeUndefined();
    expect(attentionReason(null)).toBeUndefined();
  });
});

describe('contextualAction', () => {
  const invalid = { validity: 'invalid' as const };
  const withQr = { validate: true, refresh: true, qr_login: true };
  const withoutQr = { validate: true, refresh: true, qr_login: false };

  it('logs in again by QR code where the platform supports it', () => {
    expect(
      contextualAction(
        accountDetail({ health: invalid, capabilities: withQr }),
      ),
    ).toBe('qr_login');
  });

  it('edits the credentials where it does not', () => {
    expect(
      contextualAction(
        accountDetail({ health: invalid, capabilities: withoutQr }),
      ),
    ).toBe('edit');
  });

  it('offers nothing for a healthy or disabled account', () => {
    expect(
      contextualAction(
        accountDetail({
          health: { validity: 'valid' },
          capabilities: withQr,
        }),
      ),
    ).toBeUndefined();
    expect(
      contextualAction(
        accountDetail({
          profile: { ...profile, enabled: false },
          health: invalid,
          capabilities: withQr,
        }),
      ),
    ).toBeUndefined();
  });
});

describe('validityStyle', () => {
  it('colours each state like the badges and treats no health as unknown', () => {
    expect(validityStyle('valid').dot).toBe('bg-green-500');
    expect(validityStyle('needs_refresh').dot).toBe('bg-amber-500');
    expect(validityStyle('invalid').dot).toBe('bg-red-500');
    expect(validityStyle(undefined)).toBe(VALIDITY_STYLES.unknown);
    // The badge and the dot of a state share its hue.
    for (const style of Object.values(VALIDITY_STYLES)) {
      const hue = style.dot.match(/bg-(\w+)-/)?.[1];
      if (hue) expect(style.badge).toContain(hue);
    }
  });
});
