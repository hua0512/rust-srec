import { createMiddleware } from '@tanstack/react-start';

import { DEFAULT_SIDEBAR_OPEN, readSidebarCookie } from '@/lib/sidebar-cookie';

/**
 * Puts the sidebar's stored state into the request context so the router can
 * seed it into the route context and the dashboard layout renders collapsed or
 * expanded on the server. Reading it here costs one header parse per document
 * request, where asking for it from the route would cost a server call on every
 * navigation.
 */
export const sidebarMiddleware = createMiddleware({ type: 'request' }).server(
  async ({ request, next }) => {
    const open =
      readSidebarCookie(request.headers.get('cookie') ?? undefined) ??
      DEFAULT_SIDEBAR_OPEN;

    return await next({
      context: {
        sidebar: { open },
      },
    });
  },
);
