import { createServerFn } from '@tanstack/react-start';
import { setResponseHeader } from '@tanstack/react-start/server';
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
