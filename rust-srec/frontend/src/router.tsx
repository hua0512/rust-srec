import { createRouter as createTanStackRouter } from '@tanstack/react-router';

import { routeTree } from './routeTree.gen';
import { DefaultCatchBoundary } from './components/default-catch-boundary';
import { NotFound } from './components/not-found';
import * as TanstackQuery from './integrations/tanstack-query/root-provider';
import { createI18nInstance } from './integrations/lingui/i18n';
import { routerWithLingui } from './integrations/lingui/router-plugin';
import { registerPasswordChangeRedirect } from './lib/password-change-redirect';
import { getGlobalStartContext } from '@tanstack/react-start';
import type { Mode } from '@/lib/theme-config';

export function getRouter() {
  const rqContext = TanstackQuery.getContext();
  const startContext = getGlobalStartContext();
  const i18n = (startContext as any)?.i18n ?? createI18nInstance();

  const router = routerWithLingui(
    createTanStackRouter({
      routeTree,
      context: {
        ...rqContext,
        i18n,
        theme: {
          mode: ((startContext as any)?.theme?.mode ?? 'system') as Mode,
        },
      },
      defaultPreload: 'intent',
      defaultErrorComponent: DefaultCatchBoundary,
      defaultNotFoundComponent: () => <NotFound />,
      scrollRestoration: true,
    }),
    i18n,
    {
      WrapProvider: (props) => (
        <TanstackQuery.Provider {...rqContext}>
          {props.children}
        </TanstackQuery.Provider>
      ),
    },
  );

  registerPasswordChangeRedirect(router, i18n);

  return router;
}

declare module '@tanstack/react-router' {
  interface Register {
    router: ReturnType<typeof getRouter>;
  }
}
