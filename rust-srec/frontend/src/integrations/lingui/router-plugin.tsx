import { type PropsWithChildren, type ReactNode } from 'react';
import { I18nProvider } from '@lingui/react';
import type { AnyRouter } from '@tanstack/react-router';
import { type I18n } from '@lingui/core';

type AdditionalOptions = {
  WrapProvider?: (props: { children: ReactNode }) => ReactNode;
};

type WrapperProps = PropsWithChildren<Record<string, unknown>>;

function SafeFragment({ children }: WrapperProps): ReactNode {
  return children;
}

export type ValidateRouter<TRouter extends AnyRouter> =
  NonNullable<TRouter['options']['context']> extends {
    i18n: I18n;
  }
    ? TRouter
    : never;

export function routerWithLingui<TRouter extends AnyRouter>(
  router: TRouter,
  i18n: I18n,
  additionalOpts?: AdditionalOptions,
): TRouter {
  const ogOptions = router.options;

  router.options = {
    ...router.options,
    context: {
      ...ogOptions.context,
      i18n,
    },
    // Wrap the app in a I18nProvider
    Wrap: ({ children }: PropsWithChildren) => {
      const OuterWrapper = additionalOpts?.WrapProvider || SafeFragment;
      const OGWrap = ogOptions.Wrap || SafeFragment;
      return (
        <OuterWrapper>
          <I18nProvider i18n={i18n}>
            <OGWrap>{children}</OGWrap>
          </I18nProvider>
        </OuterWrapper>
      );
    },
  };

  if (router.isServer) {
    const ogDehydrate = router.options.dehydrate;
    router.options.dehydrate = async () => {
      const ogDehydrated = await ogDehydrate?.();

      return {
        ...ogDehydrated,
        dehydratedI18n: {
          locale: i18n.locale,
          messages: i18n.messages,
        },
      };
    };
  } else {
    const ogHydrate = router.options.hydrate;
    router.options.hydrate = async (dehydrated: any) => {
      await ogHydrate?.(dehydrated);

      if (dehydrated.dehydratedI18n) {
        // On the client, hydrate the i18n catalog with the dehydrated data
        i18n.loadAndActivate({
          locale: dehydrated.dehydratedI18n.locale,
          messages: dehydrated.dehydratedI18n.messages,
        });
      }
    };
  }

  return router;
}
