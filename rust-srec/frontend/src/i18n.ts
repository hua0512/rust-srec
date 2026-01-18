import { setupI18n, type I18n } from '@lingui/core';
import { useEffect, useState } from 'react';
import { messages as enMessages } from './locales/en/messages';
import { messages as zhCNMessages } from './locales/zh-CN/messages';

export const locales = ['en', 'zh-CN'] as const;
export type Locale = (typeof locales)[number];
export const defaultLocale: Locale = 'en';

export const localeStorageKey = 'app-locale';

/**
 * Create and configure a fresh i18n instance.
 * Essential for SSR to prevent locale leakage between requests.
 */
export function createI18nInstance() {
  const instance = setupI18n();
  instance.load({
    en: enMessages,
    'zh-CN': zhCNMessages,
  });
  instance.activate(defaultLocale);
  return instance;
}

// Keep a global instance for simple client-side usage if needed,
// but preferred way is to use the instance from context.
export const i18n = createI18nInstance();

/**
 * Detect locale from browser settings.
 * Only runs on the client side.
 */
function detectClientLocale(): Locale {
  if (typeof window === 'undefined') return defaultLocale;

  // 1. Check saved preference
  const preferred = localStorage.getItem(localeStorageKey);
  const legacy = preferred ? null : localStorage.getItem('locale');
  const saved = preferred ?? legacy;
  if (saved && locales.includes(saved as Locale)) {
    if (legacy) localStorage.setItem(localeStorageKey, saved);
    console.log(`[i18n] Client: Using saved locale preference: ${saved}`);
    return saved as Locale;
  }

  // 2. Check browser language
  const browserLanguage = navigator.language;
  if (browserLanguage) {
    // Exact match
    if (locales.includes(browserLanguage as Locale)) {
      return browserLanguage as Locale;
    }
    // Partial match (e.g. 'en-US' -> 'en')
    const shortLang = browserLanguage.split('-')[0] as Locale;
    if (locales.includes(shortLang)) {
      return shortLang;
    }
    // Handle zh variants
    if (browserLanguage.toLowerCase().startsWith('zh')) {
      return 'zh-CN';
    }
  }

  return defaultLocale;
}

/**
 * Parse Accept-Language header and return the best matching locale.
 */
export function parseAcceptLanguage(
  acceptLanguage: string | null | undefined,
): Locale {
  if (!acceptLanguage) return defaultLocale;

  // Parse Accept-Language header (e.g., "zh-CN,zh;q=0.9,en;q=0.8")
  const languages = acceptLanguage
    .split(',')
    .map((lang) => {
      const [code, qValue] = lang.trim().split(';q=');
      return {
        code: code?.trim().toLowerCase() || '',
        q: qValue ? parseFloat(qValue) : 1.0,
      };
    })
    .sort((a, b) => b.q - a.q);

  for (const { code } of languages) {
    // Exact match
    if (locales.includes(code as Locale)) {
      return code as Locale;
    }
    // Handle zh variants -> zh-CN
    if (code.startsWith('zh')) {
      return 'zh-CN';
    }
    // Partial match (e.g., 'en-us' -> 'en')
    const shortCode = code.split('-')[0];
    if (shortCode && locales.includes(shortCode as Locale)) {
      return shortCode as Locale;
    }
  }

  return defaultLocale;
}

/**
 * Activate a specific locale on a specific i18n instance.
 */
export function activateLocale(instance: I18n, locale: Locale): void {
  if (instance.locale !== locale) {
    console.log(`[i18n] Activating locale: ${locale}`);
    instance.activate(locale);
  }

  if (typeof document !== 'undefined') {
    document.documentElement.lang = locale;
  }
}

export function persistLocale(locale: Locale): void {
  if (typeof window === 'undefined') return;
  localStorage.setItem(localeStorageKey, locale);
}

export function activateAndPersistLocale(instance: I18n, locale: Locale): void {
  activateLocale(instance, locale);
  persistLocale(locale);
}

/**
 * Initialize locale on the client side for a specific instance.
 */
export function initializeLocale(instance: I18n): Locale {
  if (typeof window === 'undefined') return defaultLocale;

  const locale = detectClientLocale();
  activateAndPersistLocale(instance, locale);
  return locale;
}

/**
 * Hook to initialize locale on client hydration.
 * Returns true once locale is initialized.
 *
 * IMPORTANT: During SSR and initial hydration, returns false so that
 * components can choose not to render i18n-dependent content until
 * the locale is properly set up on the client.
 */
export function useInitLocale(instance: I18n): boolean {
  const [isInitialized, setIsInitialized] = useState(false);

  useEffect(() => {
    initializeLocale(instance);
    setIsInitialized(true);
  }, [instance]);

  return isInitialized;
}

// Final exports are already handled as 'export const' or 'export function'
