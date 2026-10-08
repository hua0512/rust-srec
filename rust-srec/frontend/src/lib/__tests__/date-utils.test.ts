import { formatShortRelativeTime } from '../date-utils';

describe('formatShortRelativeTime', () => {
  const now = Date.UTC(2026, 0, 15, 12);
  const minute = 60_000;

  it('uses the largest whole unit, in the past and the future', () => {
    expect(formatShortRelativeTime(now - 41 * minute, 'en', now)).toBe(
      '41m ago',
    );
    expect(formatShortRelativeTime(now - 4 * 60 * minute, 'en', now)).toBe(
      '4h ago',
    );
    expect(formatShortRelativeTime(now + 4 * 24 * 60 * minute, 'en', now)).toBe(
      'in 4d',
    );
  });

  it('reads anything under a minute as this minute', () => {
    expect(formatShortRelativeTime(now - 20_000, 'en', now)).toBe(
      'this minute',
    );
  });

  it('follows the locale', () => {
    expect(formatShortRelativeTime(now - 41 * minute, 'zh-CN', now)).toBe(
      '41分钟前',
    );
  });
});
