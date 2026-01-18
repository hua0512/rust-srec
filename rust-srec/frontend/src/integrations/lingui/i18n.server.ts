import { parse, serialize } from 'cookie-es';
import { defaultLocale, isLocaleValid, Locale, localeStorageKey } from './i18n';

export function getLocaleFromRequest(request: Request) {
  const headers = request.headers;
  const url = new URL(request.url);
  const queryLocale = url.searchParams.get('locale') ?? '';

  if (isLocaleValid(queryLocale)) {
    return {
      locale: queryLocale as Locale,
      headers: [
        {
          key: 'Set-Cookie',
          value: serialize(localeStorageKey, queryLocale, {
            maxAge: 30 * 24 * 60 * 60,
            path: '/',
            sameSite: 'lax',
          }),
        },
      ],
    };
  }

  const cookie = parse(headers.get('cookie') ?? '');
  const savedLocale = cookie[localeStorageKey];
  if (savedLocale && isLocaleValid(savedLocale)) {
    return { locale: savedLocale as Locale };
  }

  const acceptedLanguage = headers.get('accept-language')?.split(',')[0] ?? '';
  if (acceptedLanguage) {
    const preferred = acceptedLanguage.split('-')[0];
    if (isLocaleValid(preferred)) {
      return { locale: preferred as Locale };
    }
  }

  return { locale: defaultLocale };
}
