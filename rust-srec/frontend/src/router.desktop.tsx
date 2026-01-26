import {
  createHashHistory,
  createRouter as createTanStackRouter,
} from '@tanstack/react-router';
import type { I18n } from '@lingui/core';

import { routeTree } from './routeTree.gen';
import { DefaultCatchBoundary } from './components/default-catch-boundary';
import { NotFound } from './components/not-found';
import * as TanstackQuery from './integrations/tanstack-query/root-provider';
import { createI18nInstance } from './integrations/lingui/i18n';
import { routerWithLingui } from './integrations/lingui/router-plugin';

export function getRouter(i18n?: I18n) {
  const rqContext = TanstackQuery.getContext();
  const resolvedI18n = i18n ?? createI18nInstance();
  const history = createHashHistory();

  const router = routerWithLingui(
    createTanStackRouter({
      routeTree,
      history,
      context: {
        ...rqContext,
        i18n: resolvedI18n,
      },
      defaultPreload: 'intent',
      defaultErrorComponent: DefaultCatchBoundary,
      defaultNotFoundComponent: () => <NotFound />,
      scrollRestoration: true,
    }),
    resolvedI18n,
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
