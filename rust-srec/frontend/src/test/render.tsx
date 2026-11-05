import type { ReactNode } from 'react';
import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, fireEvent, render, screen } from '@testing-library/react';

/** Fails a query on its first error instead of retrying with backoff. */
export function newQueryClient() {
  return new QueryClient({ defaultOptions: { queries: { retry: false } } });
}

/** English with an empty catalog, so messages render their source text. */
export function testI18n() {
  return setupI18n({ locale: 'en', messages: { en: {} } });
}

/** The query and translation providers every page renders under. */
export function providersWrapper(client: QueryClient = newQueryClient()) {
  const i18n = testI18n();
  return function Providers({ children }: { children: ReactNode }) {
    return (
      <QueryClientProvider client={client}>
        <I18nProvider i18n={i18n}>{children}</I18nProvider>
      </QueryClientProvider>
    );
  };
}

/** Renders `ui` under the app providers; a `rerender` keeps them. */
export function renderWithProviders(
  ui: ReactNode,
  { client = newQueryClient() }: { client?: QueryClient } = {},
) {
  return { ...render(ui, { wrapper: providersWrapper(client) }), client };
}

function openSelect(trigger: HTMLElement) {
  // Opening by keyboard from the focused trigger lets Radix restore focus to
  // it, which keeps the list open.
  act(() => trigger.focus());
  fireEvent.keyDown(trigger, { key: 'ArrowDown' });
}

/** Opens a Radix select by keyboard and picks the option named `option`. */
export async function chooseOption(
  trigger: HTMLElement,
  option: string | RegExp,
) {
  openSelect(trigger);
  fireEvent.click(await screen.findByRole('option', { name: option }));
}

/** Opens a Radix select by keyboard and returns its option names. */
export async function optionNames(trigger: HTMLElement) {
  openSelect(trigger);
  await screen.findAllByRole('option');
  return screen
    .getAllByRole('option')
    .map((option) => option.textContent ?? '');
}
