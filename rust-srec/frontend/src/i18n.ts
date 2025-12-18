import { i18n } from '@lingui/core';
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

// Set initial locale
i18n.activate(detectLocale());

export { i18n };
