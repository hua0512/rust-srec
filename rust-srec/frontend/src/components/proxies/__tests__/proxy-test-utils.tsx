import type { ReactNode } from 'react';
import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, fireEvent, render, screen } from '@testing-library/react';
import type { SavedProxy } from '@/api/schemas/proxies';

export function savedProxy(overrides: Partial<SavedProxy> = {}): SavedProxy {
  return {
    id: 'proxy-office',
    name: 'Office',
    url: 'http://proxy.example:8080',
    scheme: 'http',
    host: 'proxy.example',
    port: 8080,
    username: 'alice',
    has_password: true,
    version: 3,
    created_at: 0,
    updated_at: 0,
    usage_count: 0,
    ...overrides,
  };
}

export const socksProxy = savedProxy({
  id: 'proxy-socks',
  name: 'Tunnel',
  url: 'socks5h://10.0.0.2:1080',
  scheme: 'socks5h',
  host: '10.0.0.2',
  port: 1080,
  username: null,
  has_password: false,
  usage_count: 2,
});

/** A backend error as it reaches the browser from a server function. */
export function backendError(status: number, body: Record<string, unknown>) {
  return Object.assign(
    new Error(typeof body.message === 'string' ? body.message : 'error'),
    {
      status,
      body,
    },
  );
}

export function renderWithProviders(ui: ReactNode) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <I18nProvider i18n={setupI18n({ locale: 'en', messages: { en: {} } })}>
        {ui}
      </I18nProvider>
    </QueryClientProvider>,
  );
  return client;
}

/** Opens a Radix select by keyboard and picks the option named `option`. */
export async function chooseOption(trigger: HTMLElement, option: RegExp) {
  HTMLElement.prototype.scrollIntoView = vi.fn();
  act(() => trigger.focus());
  fireEvent.keyDown(trigger, { key: 'ArrowDown' });
  fireEvent.click(await screen.findByRole('option', { name: option }));
}

/** Opens a Radix select by keyboard and returns its option names. */
export async function optionNames(trigger: HTMLElement) {
  HTMLElement.prototype.scrollIntoView = vi.fn();
  act(() => trigger.focus());
  fireEvent.keyDown(trigger, { key: 'ArrowDown' });
  await screen.findAllByRole('option');
  return screen
    .getAllByRole('option')
    .map((option) => option.textContent ?? '');
}
