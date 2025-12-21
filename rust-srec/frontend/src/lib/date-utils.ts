import { formatDistanceToNow } from 'date-fns';
import { enUS, zhCN } from 'date-fns/locale';

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
