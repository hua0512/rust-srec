import { QueryClient } from '@tanstack/react-query';
import type { AnyRouter } from '@tanstack/react-router';

import { routerWithQueryHydration } from './router-hydration';

function fakeRouter(isServer: boolean, options: Record<string, unknown> = {}) {
  return { isServer, options } as unknown as AnyRouter;
}

describe('routerWithQueryHydration', () => {
  it('sends the server cache alongside what other plugins dehydrate', async () => {
    const queryClient = new QueryClient();
    queryClient.setQueryData(['streamers', 1], { items: ['a'] });
    const router = routerWithQueryHydration(
      fakeRouter(true, {
        dehydrate: async () => ({ dehydratedI18n: { locale: 'en' } }),
      }),
      queryClient,
    );

    const dehydrated = (await router.options.dehydrate!()) as any;

    expect(dehydrated.dehydratedI18n).toEqual({ locale: 'en' });
    expect(dehydrated.queryClientState.queries).toHaveLength(1);
    expect(dehydrated.queryClientState.queries[0].queryKey).toEqual([
      'streamers',
      1,
    ]);
  });

  it('fills the browser cache before the other hydrate steps run', async () => {
    const server = new QueryClient();
    server.setQueryData(['streamers', 1], { items: ['a'] });
    const serverRouter = routerWithQueryHydration(fakeRouter(true), server);
    const dehydrated = await serverRouter.options.dehydrate!();

    const browser = new QueryClient();
    let seenByNextStep: unknown;
    const browserRouter = routerWithQueryHydration(
      fakeRouter(false, {
        hydrate: async () => {
          seenByNextStep = browser.getQueryData(['streamers', 1]);
        },
      }),
      browser,
    );
    await browserRouter.options.hydrate!(dehydrated);

    expect(browser.getQueryData(['streamers', 1])).toEqual({ items: ['a'] });
    expect(seenByNextStep).toEqual({ items: ['a'] });
  });

  it('leaves the cache alone when the page carries no query state', async () => {
    const browser = new QueryClient();
    const router = routerWithQueryHydration(fakeRouter(false), browser);

    await router.options.hydrate!({});

    expect(browser.getQueryCache().getAll()).toHaveLength(0);
  });
});
