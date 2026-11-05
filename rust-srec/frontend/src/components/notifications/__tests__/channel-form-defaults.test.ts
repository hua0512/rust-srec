import { describe, expect, it } from 'vitest';
import { DEFAULT_SETTINGS } from '../channel-form';
import { EmailSettingsSchema } from '@/api/schemas/notifications';

describe('channel form defaults', () => {
  it('seeds Email with the keys EmailChannelSettings requires', () => {
    // `smtp_host`, `from_address` and `to_addresses` are the user's to fill in, so the seed is
    // intentionally invalid until then; the rest must already round-trip.
    const result = EmailSettingsSchema.safeParse({
      ...DEFAULT_SETTINGS.Email,
      smtp_host: 'smtp.example.com',
      from_address: 'notifier@example.com',
      to_addresses: ['ops@example.com'],
    });

    expect(result.success).toBe(true);
  });
});

describe('email settings credentials', () => {
  const base = {
    smtp_host: 'smtp.example.com',
    smtp_port: 587,
    from_address: 'notifier@example.com',
    to_addresses: ['ops@example.com'],
    use_tls: true,
    min_priority: 8,
    locale: '',
    enabled: true,
  };

  it('accepts an unauthenticated relay', () => {
    const result = EmailSettingsSchema.safeParse({
      ...base,
      username: '',
      password: '',
    });

    expect(result.success).toBe(true);
  });

  it('accepts a complete credential pair', () => {
    const result = EmailSettingsSchema.safeParse({
      ...base,
      username: 'notifier',
      password: 'hunter2',
    });

    expect(result.success).toBe(true);
  });

  // `EmailChannel::build_transport` errors on a half-configured pair rather than silently
  // dropping it, so the form has to reject the same input.
  it('rejects a username with no password', () => {
    const result = EmailSettingsSchema.safeParse({
      ...base,
      username: 'notifier',
      password: '',
    });

    expect(result.success).toBe(false);
    expect(result.error?.issues[0]?.path).toEqual(['password']);
  });

  it('rejects a password with no username', () => {
    const result = EmailSettingsSchema.safeParse({
      ...base,
      username: '   ',
      password: 'hunter2',
    });

    expect(result.success).toBe(false);
    expect(result.error?.issues[0]?.path).toEqual(['username']);
  });
});
