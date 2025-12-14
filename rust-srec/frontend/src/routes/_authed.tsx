import { createFileRoute, Outlet } from '@tanstack/react-router';
import { redirect } from '@tanstack/react-router';
import { WebSocketProvider } from '@/providers/WebSocketProvider';

export const Route = createFileRoute('/_authed')({
  beforeLoad: ({ context, location }) => {
    if (!context.user && location.pathname !== '/login') {
      throw redirect({ to: '/login', replace: true });
    }

    if (
      context.user?.mustChangePassword &&
      location.pathname !== '/change-password'
    ) {
      throw redirect({ to: '/change-password', replace: true });
    }
  },
  component: () => (
    <WebSocketProvider>
      <Outlet />
    </WebSocketProvider>
  ),
});
