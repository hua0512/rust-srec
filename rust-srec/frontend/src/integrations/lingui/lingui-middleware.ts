import { createMiddleware } from '@tanstack/react-start';
import { getLocaleFromRequest } from './i18n.server';
import { setupI18n } from '@lingui/core';
import { dynamicActivate } from './i18n';
import { setZodFallbackI18n } from '@/lib/zod-i18n';

let zodFallback: Promise<void> | undefined;

/** Loads the English catalog once for the messages of server-side validation. */
function ensureZodFallback() {
  zodFallback ??= (async () => {
    const english = setupI18n();
    await dynamicActivate(english, 'en');
    setZodFallbackI18n(english);
  })();
  return zodFallback;
}

export const linguiMiddleware = createMiddleware({ type: 'request' }).server(
  async ({ request, next }) => {
    const { locale, headers } = getLocaleFromRequest(request);
    const i18n = setupI18n();

    // Pre-load the locale for SSR
    await Promise.all([dynamicActivate(i18n, locale), ensureZodFallback()]);

    const result = await next({
      context: {
        locale,
        i18n,
      },
    });

    if (headers) {
      headers.forEach(({ key, value }) => {
        result.response.headers.append(key, value);
      });
    }

    return result;
  },
);
