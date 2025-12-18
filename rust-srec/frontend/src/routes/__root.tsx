import { HeadContent, Scripts, createRootRoute } from '@tanstack/react-router';
import { TanStackRouterDevtools } from '@tanstack/react-router-devtools';

import appCss from '../styles.css?url';
import { NotFound } from '@/components/not-found';
import { createServerFn } from '@tanstack/react-start';

const fetchUser = createServerFn({ method: 'GET' }).handler(async () => {
  // We need to auth on the server so we have access to secure cookies
  const { useAppSession } = await import('@/utils/session');
  const session = await useAppSession();

  if (!session.data.username || !session.data.roles) {
    return null;
  }

  return session.data;
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
import { i18n } from '../i18n';
import { ThemeProvider } from '../components/theme-provider';
import { Toaster } from '../components/ui/sonner';

import { QueryClient, QueryClientProvider } from '@tanstack/react-query';

// Export a shared QueryClient so beforeLoad hooks can use ensureQueryData
export const queryClient = new QueryClient();

function RootDocument({ children }: { children: React.ReactNode }) {
  return (
    <html lang={i18n.locale}>
      <head>
        <link rel="icon" type="image/svg+xml" href="/stream-rec.svg"></link>
        <HeadContent />
      </head>
      <body>
        <QueryClientProvider client={queryClient}>
          <I18nProvider i18n={i18n}>
            <ThemeProvider defaultTheme="system" storageKey="vite-ui-theme">
              {children}
              <Toaster />
              <TanStackRouterDevtools position="bottom-right" />
              <Scripts />
            </ThemeProvider>
          </I18nProvider>
        </QueryClientProvider>
      </body>
    </html>
  );
}
