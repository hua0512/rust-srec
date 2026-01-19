import { parse, serialize } from 'cookie-es';
import { defaultLocale, getPreferredLocale, isLocaleValid, Locale, localeStorageKey } from './i18n';

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

  const acceptedLanguages = headers.get('accept-language')?.split(',') ?? [];
  for (const lang of acceptedLanguages) {
    const tag = lang.split(';')[0].trim();
    const preferred = getPreferredLocale(tag);
    if (preferred) {
      return { locale: preferred };
    }
  }

  return { locale: defaultLocale };
}
