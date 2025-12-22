import { HeadContent, Scripts, createRootRoute } from '@tanstack/react-router';
import { lazy, Suspense } from 'react';

import appCss from '../styles.css?url';
import { NotFound } from '@/components/not-found';
import { createServerFn } from '@tanstack/react-start';

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

export const Route = createRootRoute({
  beforeLoad: async () => {
    const user = await fetchUser();
    return {
      user,
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
import { i18n, useInitLocale } from '../i18n';
import { ThemeProvider } from '../components/theme-provider';
import { Toaster } from '../components/ui/sonner';

import { QueryClient, QueryClientProvider } from '@tanstack/react-query';

// Export a shared QueryClient so beforeLoad hooks can use ensureQueryData
export const queryClient = new QueryClient();

function RootDocument({ children }: { children: React.ReactNode }) {
  // Initialize user's preferred locale after hydration
  useInitLocale();

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
