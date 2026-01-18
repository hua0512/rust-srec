import { setupI18n, type I18n } from '@lingui/core';

export const locales = ['en', 'zh-CN'] as const;
export type Locale = (typeof locales)[number];
export const defaultLocale: Locale = 'en';

export const localeStorageKey = 'app-locale';

export function isLocaleValid(locale: string): locale is Locale {
  return locales.includes(locale as Locale);
}

/**
 * Dynamically load and activate a locale.
 */
export async function dynamicActivate(i18n: I18n, locale: Locale) {
  // Use the compiled messages for better performance
  // The path depends on where this file is relative to locales
  const { messages } = await import(`../../locales/${locale}/messages.ts`);
  i18n.loadAndActivate({ locale, messages });
}

export function createI18nInstance() {
  const i18n = setupI18n();
  // We don't load messages here, we let the middleware or components do it
  return i18n;
}
