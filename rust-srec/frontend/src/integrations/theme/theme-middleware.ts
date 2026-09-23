import { createMiddleware } from '@tanstack/react-start';
import { parse, serialize } from 'cookie-es';
import {
  COOKIE_KEY_MODE,
  COOKIE_MAX_AGE,
  DEFAULT_MODE,
  isMode,
} from '@/lib/theme-config';

export const themeMiddleware = createMiddleware({ type: 'request' }).server(
  async ({ request, next }) => {
    const cookie = parse(request.headers.get('cookie') ?? '');
    const rawMode = cookie[COOKIE_KEY_MODE];

    const mode = isMode(rawMode) ? rawMode : DEFAULT_MODE;

    const result = await next({
      context: {
        theme: { mode },
      },
    });

    // Refresh cookie expiry on every request
    result.response.headers.append(
      'Set-Cookie',
      serialize(COOKIE_KEY_MODE, mode, {
        path: '/',
        maxAge: COOKIE_MAX_AGE,
        sameSite: 'lax',
      }),
    );

    return result;
  },
);
