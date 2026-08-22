import { setupI18n, type I18n } from '@lingui/core';

export const locales = ['en', 'zh-CN'] as const;
export type Locale = (typeof locales)[number];
export const defaultLocale: Locale = 'en';

/**
 * Each locale's name in its own language.
 *
 * Deliberately not run through the catalog: someone looking for their language scans for the
 * word as they write it, so "简体中文" stays "简体中文" whatever the interface is set to.
 */
export const localeNativeNames: Record<Locale, string> = {
  en: 'English',
  'zh-CN': '简体中文',
};

export const localeStorageKey = 'app-locale';

export function isLocaleValid(locale: string): locale is Locale {
  return locales.includes(locale as Locale);
}

/**
 * Map base languages to supported locales.
 */
export const languageToLocaleMap: Record<string, Locale> = {
  en: 'en',
  zh: 'zh-CN',
};

/**
 * Get the best matching locale for a given language tag.
 */
export function getPreferredLocale(languageTag: string): Locale | null {
  if (isLocaleValid(languageTag)) {
    return languageTag;
  }

  const baseLang = languageTag.split('-')[0].toLowerCase();
  if (baseLang in languageToLocaleMap) {
    return languageToLocaleMap[baseLang];
  }

  return null;
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
