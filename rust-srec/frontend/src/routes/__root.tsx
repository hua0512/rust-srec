import {
  HeadContent,
  Outlet,
  Scripts,
  createRootRouteWithContext,
} from '@tanstack/react-router';
import { useLingui } from '@lingui/react';
import { type I18n } from '@lingui/core';

import appCss from '../styles.css?url';
import { NotFound } from '@/components/not-found';
import { createServerFn } from '@tanstack/react-start';
import { QueryClient } from '@tanstack/react-query';
import { ThemeProvider } from '@/components/providers/theme-provider';
import { Toaster } from '@/components/ui/sonner';
import { TanStackDevtools } from '@tanstack/react-devtools';
import { TanStackRouterDevtoolsPanel } from '@tanstack/react-router-devtools';
import TanStackQueryDevtools from '../integrations/tanstack-query/dev-tools';

export const fetchUser = createServerFn({ method: 'GET' }).handler(async () => {
  const { ensureValidToken } = await import('@/server/tokenRefresh');
  return await ensureValidToken();
});

interface MyRouterContext {
  queryClient: QueryClient;
  i18n: I18n;
}

export const Route = createRootRouteWithContext<MyRouterContext>()({
  beforeLoad: async () => {
    const user = await fetchUser();
    return {
      user,
    };
  },
  head: () => ({
    meta: [
      { charSet: 'utf-8' },
      { name: 'viewport', content: 'width=device-width, initial-scale=1' },
      { title: 'Rust-Srec' },
    ],
    links: [
      { rel: 'stylesheet', href: appCss },
      { rel: 'preconnect', href: 'https://fonts.googleapis.com' },
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
  component: RootComponent,
  notFoundComponent: () => <NotFound />,
});

function RootComponent() {
  return (
    <RootDocument>
      <Outlet />
    </RootDocument>
  );
}

function RootDocument({ children }: { children: React.ReactNode }) {
  const { i18n } = useLingui();

  return (
    <html lang={i18n.locale} suppressHydrationWarning>
      <head>
        <link rel="icon" type="image/svg+xml" href="/stream-rec.svg"></link>
        <HeadContent />
      </head>
      <body>
        <ThemeProvider>{children}</ThemeProvider>
        <TanStackDevtools
          config={{ position: 'bottom-right' }}
          plugins={[
            {
              name: 'Tanstack Router',
              render: <TanStackRouterDevtoolsPanel />,
            },
            TanStackQueryDevtools,
          ]}
        />
        <Toaster position="top-right" />
        <Scripts />
      </body>
    </html>
  );
}
