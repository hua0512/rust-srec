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
