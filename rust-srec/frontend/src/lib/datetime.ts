import { formatDistanceToNow, type Locale } from 'date-fns';
import { enUS, zhCN } from 'date-fns/locale';
import { formatDuration } from './format';

/**
 * Locale-aware date, relative time and duration formatting.
 *
 * `formatDate` replaces `i18n.date`, which Lingui v6 deprecates ("Use `Intl.DateTimeFormat`
 * directly. This helper will be removed."). The behaviour is deliberately kept
 * identical to Lingui's so migrating a call site cannot change what a user
 * sees — see `src/lib/__tests__/datetime.test.ts`, which asserts that against
 * `i18n.date` itself.
 */

/**
 * What Lingui applies when no options are given, including a short month name
 * rather than Intl's default numeric month.
 */
const DEFAULT_DATE_OPTIONS: Intl.DateTimeFormatOptions = {
  day: 'numeric',
  month: 'short',
  year: 'numeric',
};

// Constructing an `Intl.DateTimeFormat` is expensive relative to formatting one
// value, and these run per row on list pages, so formatters are reused. Lingui
// caches for the same reason.
const formatterCache = new Map<string, Intl.DateTimeFormat>();

/**
 * The shared formatter for `locale` and `options`, for callers that format
 * many values with the same options and want to skip the per-call cache key.
 */
export function getDateFormatter(
  locale: string,
  options: Intl.DateTimeFormatOptions,
): Intl.DateTimeFormat {
  const key = `${locale}\u0000${JSON.stringify(options)}`;
  let formatter = formatterCache.get(key);
  if (!formatter) {
    // `en` trails the active locale as a fallback, matching Lingui's
    // `normalizeLocales`, so an unsupported tag still formats.
    formatter = new Intl.DateTimeFormat([locale, 'en'], options);
    formatterCache.set(key, formatter);
  }
  return formatter;
}

/** What `Date.prototype.toLocaleString` shows: the numeric date and time. */
export const NUMERIC_DATE_TIME: Intl.DateTimeFormatOptions = {
  year: 'numeric',
  month: 'numeric',
  day: 'numeric',
  hour: 'numeric',
  minute: 'numeric',
  second: 'numeric',
};

/**
 * Format `value` for `locale`. Strings are parsed as dates; numbers are treated
 * as epoch milliseconds.
 */
export function formatDate(
  locale: string,
  value: Date | number | string,
  options: Intl.DateTimeFormatOptions = DEFAULT_DATE_OPTIONS,
): string {
  return getDateFormatter(locale, options).format(
    typeof value === 'string' ? new Date(value) : value,
  );
}

const DATE_FNS_LOCALES: Record<string, Locale> = {
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
  const dateLocale = DATE_FNS_LOCALES[locale] ?? enUS;

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
) => DurationFormatter;
type DurationFormatter = {
  format: (duration: Record<string, number>) => string;
};

// Durations are formatted on every progress tick, so one formatter is kept
// per locale.
const durationFormatters = new Map<string, DurationFormatter>();

function getDurationFormatter(
  ctor: DurationFormatCtor,
  locale: string,
): DurationFormatter {
  let formatter = durationFormatters.get(locale);
  if (!formatter) {
    formatter = new ctor(locale, { style: 'narrow' });
    durationFormatters.set(locale, formatter);
  }
  return formatter;
}

/**
 * Locale-aware counterpart of formatDuration's verbose mode: same unit
 * selection (days/hours/minutes, seconds only when the total is under an
 * hour unless `showSeconds` is set), but with unit labels from
 * Intl.DurationFormat for the given locale (en "2h 5m", zh-CN "2小时5分钟").
 * Falls back to the English-suffixed formatDuration when the engine lacks
 * Intl.DurationFormat.
 */
export function formatLocalizedDuration(
  seconds: number | null | undefined,
  locale: string = 'en',
  { showSeconds = false }: { showSeconds?: boolean } = {},
): string {
  if (seconds == null || seconds === 0) return '-';

  const durationFormat = (
    Intl as unknown as { DurationFormat?: DurationFormatCtor }
  ).DurationFormat;
  if (!durationFormat) {
    return formatDuration(seconds < 1 ? seconds : Math.round(seconds), {
      decimals: 0,
      showSeconds,
    });
  }

  if (seconds < 1) {
    return getDurationFormatter(durationFormat, locale).format({
      milliseconds: Math.round(seconds * 1000),
    });
  }

  // Rounded before splitting so that 119.6 s carries into "2m", not "1m 60s".
  const total = Math.round(seconds);
  const days = Math.floor(total / 86400);
  const hours = Math.floor((total % 86400) / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const secs = total % 60;

  const duration: Record<string, number> = {};
  if (days > 0) duration.days = days;
  if (hours > 0) duration.hours = hours;
  if (minutes > 0) duration.minutes = minutes;
  if ((total < 3600 || showSeconds) && secs > 0) duration.seconds = secs;

  if (Object.keys(duration).length === 0) return formatDuration(seconds);

  return getDurationFormatter(durationFormat, locale).format(duration);
}
