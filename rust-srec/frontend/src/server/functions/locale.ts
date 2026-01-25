import { createServerFn } from '@/server/createServerFn';
import { serialize } from 'cookie-es';
import {
  isLocaleValid,
  Locale,
  localeStorageKey,
} from '../../integrations/lingui/i18n';

export const updateLocale = createServerFn({ method: 'POST' })
  .inputValidator((locale: string) => locale as Locale)
  .handler(async ({ data }) => {
    if (isLocaleValid(data)) {
      if (import.meta.env.VITE_DESKTOP === '1') {
        if (typeof window !== 'undefined') {
          window.localStorage.setItem(localeStorageKey, data);
        }
        return;
      }

      const { setResponseHeader } = await import('@tanstack/react-start/server');
      setResponseHeader(
        'Set-Cookie',
        serialize(localeStorageKey, data, {
          maxAge: 30 * 24 * 60 * 60,
          path: '/',
          sameSite: 'lax',
        }),
      );
    }
  });
