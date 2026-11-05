import type { ReactNode } from 'react';

/**
 * Stands in for the router's `Link` outside a router:
 * `vi.mock('@tanstack/react-router', async () => ({ Link: (await import('@/test/router')).MockLink }))`.
 * The href fills each `$param` of `to` from `params`, so a test can check where
 * a link leads.
 */
export function MockLink({
  children,
  to = '',
  params,
  className,
}: {
  children?: ReactNode;
  to?: string;
  params?: Record<string, string>;
  className?: string;
}) {
  const href = Object.entries(params ?? {}).reduce(
    (path, [name, value]) => path.replace(`$${name}`, value),
    to,
  );
  return (
    <a href={href} className={className}>
      {children}
    </a>
  );
}
