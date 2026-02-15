import {
  HeadContent,
  Outlet,
  Scripts,
  createRootRouteWithContext,
} from '@tanstack/react-router';
import { useLingui } from '@lingui/react';
import { type I18n } from '@lingui/core';
import { useEffect, useState } from 'react';

import appCss from '../styles.css?url';
import '@fontsource/inter/400.css';
import '@fontsource/inter/500.css';
import '@fontsource/inter/600.css';
import '@fontsource/inter/700.css';
import { NotFound } from '@/components/not-found';
import { QueryClient } from '@tanstack/react-query';
import { ThemeProvider } from '@/components/providers/theme-provider';
import { Toaster } from '@/components/ui/sonner';
import { isDesktopBuild } from '@/utils/desktop';

type DevtoolsModules = {
  TanStackDevtools: React.ComponentType<any>;
  TanStackRouterDevtoolsPanel: React.ComponentType<any>;
  ReactQueryDevtoolsPanel: React.ComponentType<any>;
};

const Devtools = (() => {
  if (!import.meta.env.DEV) {
    return function DevtoolsDisabled() {
      return null;
    };
  }

  return function DevtoolsEnabled() {
    const [modules, setModules] = useState<DevtoolsModules | null>(null);

    useEffect(() => {
      let cancelled = false;

      void (async () => {
        const [reactDevtools, routerDevtools, queryDevtools] =
          await Promise.all([
            import('@tanstack/react-devtools'),
            import('@tanstack/react-router-devtools'),
            import('@tanstack/react-query-devtools'),
          ]);

        if (cancelled) return;

        setModules({
          TanStackDevtools: reactDevtools.TanStackDevtools,
          TanStackRouterDevtoolsPanel:
            routerDevtools.TanStackRouterDevtoolsPanel,
          ReactQueryDevtoolsPanel: queryDevtools.ReactQueryDevtoolsPanel,
        });
      })();

      return () => {
        cancelled = true;
      };
    }, []);

    if (!modules) return null;

    const {
      TanStackDevtools,
      TanStackRouterDevtoolsPanel,
      ReactQueryDevtoolsPanel,
    } = modules;

    return (
      <TanStackDevtools
        config={{ position: 'bottom-right' }}
        plugins={[
          {
            name: 'Tanstack Router',
            render: <TanStackRouterDevtoolsPanel />,
          },
          {
            name: 'Tanstack Query',
            render: <ReactQueryDevtoolsPanel />,
          },
        ]}
      />
    );
  };
})();

interface MyRouterContext {
  queryClient: QueryClient;
  i18n: I18n;
}

export const Route = createRootRouteWithContext<MyRouterContext>()({
  pendingComponent: () => (
    <RootDocument>
      <AppLoadingScreen />
    </RootDocument>
  ),
  head: () => ({
    meta: [
      { charSet: 'utf-8' },
      { name: 'viewport', content: 'width=device-width, initial-scale=1' },
      { title: 'Rust-Srec' },
    ],
    links: [{ rel: 'stylesheet', href: appCss }],
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
  const isDesktop = isDesktopBuild();

  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let cancelled = false;

    void (async () => {
      const { initDesktopLaunchListener } = await import('@/desktop/launch');
      if (cancelled) return;

      unlisten = await initDesktopLaunchListener((payload) => {
        window.dispatchEvent(
          new CustomEvent('rust-srec:launch', {
            detail: payload,
          }),
        );
      });
    })();

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  if (isDesktop) {
    return (
      <div className="min-h-dvh">
        <ThemeProvider>{children}</ThemeProvider>

        <Devtools />
        <Toaster position="top-right" />
      </div>
    );
  }

  return (
    <html lang={i18n.locale} suppressHydrationWarning>
      <head>
        <link rel="icon" type="image/svg+xml" href="/stream-rec.svg"></link>
        <HeadContent />
      </head>
      <body suppressHydrationWarning>
        <ThemeProvider>{children}</ThemeProvider>
        <Devtools />
        <Toaster position="top-right" />
        <Scripts />
      </body>
    </html>
  );
}

function AppLoadingScreen() {
  return (
    <div className="min-h-dvh bg-gradient-to-b from-slate-950 via-slate-900 to-slate-950 text-slate-100">
      <div className="mx-auto flex min-h-dvh max-w-md flex-col items-center justify-center gap-6 px-6">
        <img
          src="/stream-rec-white.svg"
          alt=""
          className="h-16 w-16 opacity-90"
        />

        <div className="text-center">
          <div className="text-2xl font-semibold tracking-tight">Rust-Srec</div>
          <div className="mt-1 text-sm text-slate-300">Starting up...</div>
        </div>

        <div className="flex items-center gap-2">
          <span className="h-2.5 w-2.5 animate-bounce rounded-full bg-slate-200 [animation-delay:-0.2s]" />
          <span className="h-2.5 w-2.5 animate-bounce rounded-full bg-slate-200 [animation-delay:-0.1s]" />
          <span className="h-2.5 w-2.5 animate-bounce rounded-full bg-slate-200" />
        </div>
      </div>
    </div>
  );
}
