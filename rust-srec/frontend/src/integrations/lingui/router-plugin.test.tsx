import type { AnyRouter } from '@tanstack/react-router';

import { createI18nInstance } from './i18n';
import { routerWithLingui } from './router-plugin';

describe('routerWithLingui', () => {
  it('dehydrates only the active locale', async () => {
    const i18n = createI18nInstance();
    i18n.loadAndActivate({ locale: 'en', messages: {} });
    const router = {
      isServer: true,
      options: {
        context: { i18n },
        dehydrate: async () => ({ existing: true }),
      },
    };

    routerWithLingui(router as unknown as AnyRouter, i18n);

    await expect(router.options.dehydrate()).resolves.toEqual({
      existing: true,
      dehydratedI18n: { locale: 'en' },
    });
  });

  it('loads and activates the dehydrated locale before hydration completes', async () => {
    const i18n = createI18nInstance();
    const router = {
      isServer: false,
      options: {
        context: { i18n },
        hydrate: async (_dehydrated: unknown) => undefined,
      },
    };

    routerWithLingui(router as unknown as AnyRouter, i18n);
    await router.options.hydrate({
      dehydratedI18n: { locale: 'zh-CN' },
    });

    expect(i18n.locale).toBe('zh-CN');
    expect(i18n._('n1ekoW')).toBe('登录');
  });
});
