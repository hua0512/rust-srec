import { createRouter as createTanStackRouter } from '@tanstack/react-router';

import { routeTree } from './routeTree.gen';
import { DefaultCatchBoundary } from './components/default-catch-boundary';
import { NotFound } from './components/not-found';
import * as TanstackQuery from './integrations/tanstack-query/root-provider';
import { createI18nInstance } from './integrations/lingui/i18n';
import { routerWithLingui } from './integrations/lingui/router-plugin';
import { getGlobalStartContext } from '@tanstack/react-start';

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

  return router;
}

declare module '@tanstack/react-router' {
  interface Register {
    router: ReturnType<typeof getRouter>;
  }
}
