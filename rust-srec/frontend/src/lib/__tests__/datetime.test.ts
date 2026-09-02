import { setupI18n } from '@lingui/core';
import { describe, expect, it } from 'vitest';

import { formatDate } from '../datetime';

/**
 * `formatDate` replaces the deprecated `i18n.date`. These assert equivalence
 * against Lingui itself across the locales and option shapes actually used in
 * the app, so the migration provably does not change rendered output. When
 * Lingui eventually deletes the helper, delete this file — `datetime.ts` is
 * then the definition rather than a port.
 *
 * The `i18n.date` calls below are the point of the file: do not migrate them.
 */
const LOCALES = ['en', 'zh-CN'];

const OPTION_SHAPES: (Intl.DateTimeFormatOptions | undefined)[] = [
  undefined,
  { dateStyle: 'medium' },
  { dateStyle: 'medium', timeStyle: 'short' },
  { dateStyle: 'short', timeStyle: 'short' },
  { timeStyle: 'short' },
  { timeStyle: 'medium' },
  { month: 'short', day: 'numeric' },
  { hour: '2-digit', minute: '2-digit' },
];

const VALUES = [
  new Date('2026-09-02T09:17:00Z'),
  new Date('2026-01-01T00:00:00Z'),
  new Date('2026-12-31T23:59:59Z'),
  0,
];

describe('formatDate matches the deprecated i18n.date', () => {
  for (const locale of LOCALES) {
    const i18n = setupI18n({ locale, messages: { [locale]: {} } });

    for (const options of OPTION_SHAPES) {
      it(`${locale} with ${options ? JSON.stringify(options) : 'no options'}`, () => {
        for (const value of VALUES) {
          expect(formatDate(locale, value, options)).toBe(
            i18n.date(value, options),
          );
        }
      });
    }
  }

  it('parses string input the way i18n.date does', () => {
    const i18n = setupI18n({ locale: 'en', messages: { en: {} } });
    const iso = '2026-09-02T09:17:00Z';
    expect(formatDate('en', iso, { dateStyle: 'medium' })).toBe(
      i18n.date(iso, { dateStyle: 'medium' }),
    );
  });

  it('treats a number as epoch milliseconds', () => {
    expect(formatDate('en', 0, { dateStyle: 'medium', timeZone: 'UTC' })).toBe(
      'Jan 1, 1970',
    );
  });

  it('reuses the formatter for repeated locale/option pairs', () => {
    // Cheap proxy for the cache working: repeated calls stay consistent and do
    // not depend on call order.
    const first = formatDate('en', VALUES[0], { dateStyle: 'medium' });
    const other = formatDate('zh-CN', VALUES[0], { dateStyle: 'medium' });
    expect(formatDate('en', VALUES[0], { dateStyle: 'medium' })).toBe(first);
    expect(other).not.toBe(first);
  });
});
