import type { AnyRouter } from '@tanstack/react-router';
import type { I18n } from '@lingui/core';
import { msg } from '@lingui/core/macro';
import { toast } from 'sonner';

import { isPasswordChangeRequiredError } from './api-error';

// Registered by getRouter (router.tsx and router.desktop.tsx) so the
// QueryCache/MutationCache onError handlers created in
// integrations/tanstack-query/root-provider can navigate.
let registered: { router: AnyRouter; i18n: I18n } | null = null;
let redirectInFlight = false;

export function registerPasswordChangeRedirect(router: AnyRouter, i18n: I18n) {
  registered = { router, i18n };
}

/**
 * Routes into the change-password flow the same way handleLoginSuccess in
 * routes/_public/login.lazy.tsx does: warn, invalidate so the /_authed
 * beforeLoad guard re-reads the session (fetchBackend has already persisted
 * mustChangePassword there), then replace-navigate to /change-password.
 */
export function redirectToChangePasswordOnError(error: unknown) {
  if (!isPasswordChangeRequiredError(error)) return;
  // During SSR the /_authed beforeLoad redirect performs this instead.
  if (typeof window === 'undefined') return;
  const active = registered;
  if (!active) return;
  // Calls issued while /change-password is mounted (e.g. WebSocketProvider
  // renders under /_authed) can still fail with this code; skip so repeated
  // 403s do not stack navigations.
  if (redirectInFlight) return;
  if (active.router.state.location.pathname === '/change-password') return;
  redirectInFlight = true;
  void (async () => {
    try {
      await active.router.invalidate();
      toast.warning(active.i18n._(msg`Password change required`));
      await active.router.navigate({ to: '/change-password', replace: true });
    } finally {
      redirectInFlight = false;
    }
  })();
}
