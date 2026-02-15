import { createFileRoute, Outlet } from '@tanstack/react-router';
import { redirect } from '@tanstack/react-router';
import { WebSocketProvider } from '@/providers/WebSocketProvider';
import { createServerFn } from '@/server/createServerFn';
import { ensureValidToken } from '@/server/tokenRefresh';
import { BrowserNotificationListener } from '@/components/notifications/browser-notification-listener';

export const fetchUser = createServerFn({ method: 'GET' }).handler(async () => {
  return await ensureValidToken();
});

export const Route = createFileRoute('/_authed')({
  beforeLoad: async ({ location }) => {
    const user = await fetchUser();

    if (!user && location.pathname !== '/login') {
      throw redirect({ to: '/login', replace: true });
    }

    if (user?.mustChangePassword && location.pathname !== '/change-password') {
      throw redirect({ to: '/change-password', replace: true });
    }

    return { user };
  },
  component: () => (
    <WebSocketProvider>
      <Outlet />
      <BrowserNotificationListener />
    </WebSocketProvider>
  ),
});
