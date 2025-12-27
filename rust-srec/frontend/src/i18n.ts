import { i18n } from '@lingui/core';
import { useEffect, useState } from 'react';
import { messages as enMessages } from './locales/en/messages';
import { messages as zhCNMessages } from './locales/zh-CN/messages';

// Load messages for all locales
i18n.load({
  en: enMessages,
  'zh-CN': zhCNMessages,
});

export const locales = ['en', 'zh-CN'] as const;
export type Locale = (typeof locales)[number];

export const defaultLocale: Locale = 'en';

/**
 * Detect locale from browser settings.
 * Only runs on the client side.
 */
function detectClientLocale(): Locale {
  if (typeof window === 'undefined') return defaultLocale;

  // 1. Check saved preference
  const saved = localStorage.getItem('locale');
  if (saved && locales.includes(saved as Locale)) {
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

// Activate default locale at module level (SSR-safe)
// The actual locale will be set by activateLocale() on both server and client
i18n.activate(defaultLocale);

/**
 * Activate a specific locale.
 */
export function activateLocale(locale: Locale): void {
  if (i18n.locale !== locale) {
    i18n.activate(locale);
  }
}

/**
 * Initialize locale on the client side.
 * Call this early in the app to switch to the user's preferred locale.
 */
export function initializeLocale(): Locale {
  if (typeof window === 'undefined') return defaultLocale;

  const locale = detectClientLocale();
  activateLocale(locale);
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
export function useInitLocale(): boolean {
  const [isInitialized, setIsInitialized] = useState(false);

  useEffect(() => {
    initializeLocale();
    setIsInitialized(true);
  }, []);

  return isInitialized;
}

export { i18n };
