import { HeadContent, Scripts, createRootRoute } from '@tanstack/react-router';
import { lazy, Suspense, useEffect, useRef } from 'react';

import appCss from '../styles.css?url';
import { NotFound } from '@/components/not-found';
import { createServerFn } from '@tanstack/react-start';
import { getRequestHeader } from '@tanstack/react-start/server';

// Lazy load devtools to prevent hydration mismatch (client-only component)
const TanStackRouterDevtools =
  process.env.NODE_ENV === 'production'
    ? () => null
    : lazy(() =>
      import('@tanstack/react-router-devtools').then((res) => ({
        default: res.TanStackRouterDevtools,
      })),
    );

const fetchUser = createServerFn({ method: 'GET' }).handler(async () => {
  // Use ensureValidToken to validate the session and refresh if needed
  // This prevents users with expired tokens from appearing authenticated
  const { ensureValidToken } = await import('@/server/tokenRefresh');
  return await ensureValidToken();
});

// Detect locale on the server from Accept-Language header
const detectServerLocale = createServerFn({ method: 'GET' }).handler(async () => {
  const { parseAcceptLanguage, defaultLocale } = await import('../i18n');
  try {
    const acceptLanguage = getRequestHeader('accept-language');
    return parseAcceptLanguage(acceptLanguage);
  } catch {
    return defaultLocale;
  }
});

export const Route = createRootRoute({
  beforeLoad: async () => {
    const [user, serverLocale] = await Promise.all([
      fetchUser(),
      detectServerLocale(),
    ]);
    return {
      user,
      serverLocale,
    };
  },
  head: () => ({
    meta: [
      {
        charSet: 'utf-8',
      },
      {
        name: 'viewport',
        content: 'width=device-width, initial-scale=1',
      },
      {
        title: 'Rust-Srec',
      },
    ],
    links: [
      {
        rel: 'stylesheet',
        href: appCss,
      },
      {
        rel: 'preconnect',
        href: 'https://fonts.googleapis.com',
      },
      {
        rel: 'preconnect',
        href: 'https://fonts.gstatic.com',
        crossOrigin: 'anonymous',
      },
      {
        rel: 'stylesheet',
        href: 'https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700&display=swap',
      },
    ],
  }),
  shellComponent: RootDocument,
  notFoundComponent: () => <NotFound />,
});

import { I18nProvider } from '@lingui/react';
import { i18n, activateLocale, initializeLocale, type Locale } from '../i18n';
import { ThemeProvider } from '../components/theme-provider';
import { Toaster } from '../components/ui/sonner';

import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { useRouteContext } from '@tanstack/react-router';

// Export a shared QueryClient so beforeLoad hooks can use ensureQueryData
export const queryClient = new QueryClient();

function RootDocument({ children }: { children: React.ReactNode }) {
  // Get the server-detected locale from route context
  const { serverLocale } = useRouteContext({ from: '__root__' }) as { serverLocale?: Locale };

  // Track if we've done initial activation
  const initializedRef = useRef(false);

  // On first render (both SSR and hydration), activate the server-detected locale
  // This ensures server and client render with the same locale during hydration
  if (!initializedRef.current && serverLocale) {
    activateLocale(serverLocale);
    initializedRef.current = true;
  }

  // After hydration, switch to client-detected locale (which may differ if user has localStorage preference)
  useEffect(() => {
    initializeLocale();
  }, []);

  return (
    <html lang={i18n.locale} suppressHydrationWarning>
      <head>
        <link rel="icon" type="image/svg+xml" href="/stream-rec.svg"></link>
        <HeadContent />
      </head>
      <body suppressHydrationWarning>
        <QueryClientProvider client={queryClient}>
          <I18nProvider i18n={i18n}>
            <ThemeProvider defaultTheme="system" storageKey="vite-ui-theme">
              {children}
              <Toaster />
              <Suspense>
                <TanStackRouterDevtools position="bottom-right" />
              </Suspense>
              <Scripts />
            </ThemeProvider>
          </I18nProvider>
        </QueryClientProvider>
      </body>
    </html>
  );
}
