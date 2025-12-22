import { i18n } from '@lingui/core';
import { useEffect, useState } from 'react';
import { messages as enMessages } from './locales/en/messages';
import { messages as zhCNMessages } from './locales/zh-CN/messages';

// Load messages for all locales
i18n.load({
  en: enMessages,
  'zh-CN': zhCNMessages,
});

const locales = ['en', 'zh-CN'] as const;
type Locale = (typeof locales)[number];

const defaultLocale: Locale = 'en';

function detectLocale(): string {
  if (typeof window === 'undefined') return defaultLocale;

  // 1. Check saved preference
  const saved = localStorage.getItem('locale');
  if (saved && locales.includes(saved as Locale)) {
    return saved;
  }

  // 2. Check browser language
  const browserLanguage = navigator.language;
  if (browserLanguage) {
    // Exact match
    if (locales.includes(browserLanguage as Locale)) {
      return browserLanguage;
    }
    // Partial match (e.g. 'en-US' -> 'en')
    const shortLang = browserLanguage.split('-')[0] as Locale;
    if (locales.includes(shortLang)) {
      return shortLang;
    }
  }

  return defaultLocale;
}

// Activate default locale at module level (SSR-safe)
// Client-side locale detection happens via useInitLocale hook
i18n.activate(defaultLocale);

/**
 * Initialize locale on the client side.
 * Call this early in the app to switch to the user's preferred locale.
 */
export function initializeLocale(): void {
  if (typeof window === 'undefined') return;
  const locale = detectLocale();
  if (i18n.locale !== locale) {
    i18n.activate(locale);
  }
}

/**
 * Hook to initialize locale on client hydration.
 * Returns true once locale is initialized.
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
