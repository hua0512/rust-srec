import { createFileRoute, Outlet } from '@tanstack/react-router';
import { redirect } from '@tanstack/react-router';
import { WebSocketProvider } from '@/providers/WebSocketProvider';
import { sessionQueryOptions } from '@/api/session';
import { BrowserNotificationListener } from '@/components/notifications/browser-notification-listener';
import { useSignedOutRedirect } from '@/hooks/use-signed-out-redirect';

export const Route = createFileRoute('/_authed')({
  // Router runs this on every navigation into the authenticated tree, so the
  // check goes through the session query rather than straight to the server.
  // `fetchQuery` returns the cached session while it is fresh and waits for a
  // new check once it is not, and `sessionQueryOptions` treats an
  // unauthenticated or about-to-expire result as stale straight away. Moving
  // between pages therefore reuses one verified session instead of unsealing
  // the cookie each time, while a first load, a sign-in and an expiry are
  // still verified before anything renders.
  beforeLoad: async ({ context, location }) => {
    const user = await context.queryClient.fetchQuery(sessionQueryOptions);

    if (!user && location.pathname !== '/login') {
      // `href` is the pathname plus search and hash, so the filters and page
      // number the visitor followed survive signing in. The login page drops
      // anything that is not a path on this application.
      throw redirect({
        to: '/login',
        search: { redirect: location.href },
        replace: true,
      });
    }

    if (user?.mustChangePassword && location.pathname !== '/change-password') {
      throw redirect({ to: '/change-password', replace: true });
    }

    return { user };
  },
  component: AuthedLayout,
});

function AuthedLayout() {
  // The guard above only runs on navigation; this covers a session that ends
  // while the visitor stays on one page.
  useSignedOutRedirect();

  return (
    <WebSocketProvider>
      <Outlet />
      <BrowserNotificationListener />
    </WebSocketProvider>
  );
}
