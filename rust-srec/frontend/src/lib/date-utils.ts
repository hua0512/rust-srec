import { formatDistanceToNow } from 'date-fns';
import { enUS, zhCN } from 'date-fns/locale';
import { formatDuration } from './format';

const locales: Record<string, any> = {
  en: enUS,
  'zh-CN': zhCN,
};

/**
 * Format a date as a relative time string (e.g., "5 minutes ago")
 * Respects the provided locale or defaults to English
 */
export function formatRelativeTime(
  date: string | number | Date,
  locale: string = 'en',
): string {
  const dateObj = new Date(date);
  const dateLocale = locales[locale] || enUS;

  return formatDistanceToNow(dateObj, {
    addSuffix: true,
    locale: dateLocale,
  });
}

const SHORT_RELATIVE_UNITS: [Intl.RelativeTimeFormatUnit, number][] = [
  ['year', 365 * 24 * 60 * 60_000],
  ['month', 30 * 24 * 60 * 60_000],
  ['day', 24 * 60 * 60_000],
  ['hour', 60 * 60_000],
  ['minute', 60_000],
];

/**
 * A compact relative time for dense lists, in the largest whole unit (en
 * "41m ago", "in 4d"; zh-CN "41分钟前"). Under a minute reads as this minute.
 */
export function formatShortRelativeTime(
  date: string | number | Date,
  locale: string = 'en',
  now: number = Date.now(),
): string {
  const delta = new Date(date).getTime() - now;
  const format = new Intl.RelativeTimeFormat(locale, {
    style: 'narrow',
    numeric: 'auto',
  });
  for (const [unit, size] of SHORT_RELATIVE_UNITS) {
    const value = Math.trunc(delta / size);
    if (value !== 0 || unit === 'minute') return format.format(value, unit);
  }
  return format.format(0, 'minute');
}

// Intl.DurationFormat is feature-detected at runtime and absent from the
// project's TS lib, so it gets a minimal local type here.
type DurationFormatCtor = new (
  locale: string,
  options?: { style?: 'long' | 'short' | 'narrow' | 'digital' },
) => { format: (duration: Record<string, number>) => string };

/**
 * Locale-aware counterpart of formatDuration's verbose mode: same unit
 * selection (days/hours/minutes, seconds only when the total is under an
 * hour), but with unit labels from Intl.DurationFormat for the given
 * locale (en "2h 5m", zh-CN "2小时5分钟"). Falls back to the
 * English-suffixed formatDuration when the engine lacks
 * Intl.DurationFormat.
 */
export function formatLocalizedDuration(
  seconds: number | null | undefined,
  locale: string = 'en',
): string {
  if (seconds == null || seconds === 0) return '-';

  const durationFormat = (
    Intl as unknown as { DurationFormat?: DurationFormatCtor }
  ).DurationFormat;
  if (!durationFormat) return formatDuration(seconds);

  if (seconds < 1) {
    return new durationFormat(locale, { style: 'narrow' }).format({
      milliseconds: Math.round(seconds * 1000),
    });
  }

  const days = Math.floor(seconds / 86400);
  const hours = Math.floor((seconds % 86400) / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const secs = Math.round(seconds % 60);

  const duration: Record<string, number> = {};
  if (days > 0) duration.days = days;
  if (hours > 0) duration.hours = hours;
  if (minutes > 0) duration.minutes = minutes;
  if (seconds < 3600 && secs > 0) duration.seconds = secs;

  if (Object.keys(duration).length === 0) return formatDuration(seconds);

  return new durationFormat(locale, { style: 'narrow' }).format(duration);
}
