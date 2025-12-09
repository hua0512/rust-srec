import { createFileRoute, Outlet, redirect } from '@tanstack/react-router';

import { SidebarProvider } from '../components/ui/sidebar';
import { AppSidebar } from '../components/app-sidebar';
import { TopBar } from '../components/top-bar';
import { Footer } from '../components/footer';
import { useDownloadProgress } from '../hooks/useDownloadProgress';

export const Route = createFileRoute('/_auth')({
  beforeLoad: ({ context, location }) => {
    const { isAuthenticated, user } = context.auth;
    console.log('Auth Guard: Checking auth', { isAuthenticated, path: location.href });

    // check auth
    if (!isAuthenticated) {
      console.log('Auth Guard: User not authenticated, redirecting to /login');
      throw redirect({
        to: '/login',
        search: {
          // Use the current location to power a redirect after login
          // (Do not use `router.state.resolvedLocation` as it can
          // potentially lag behind the actual current location)
          redirect: location.href,
        },
      });
    }

    if (user?.must_change_password && location.pathname !== '/change-password') {
      throw redirect({
        to: '/change-password',
      });
    }
  },
  component: AuthLayout,
});

function AuthLayout() {
  // Initialize WebSocket connection for download progress updates
  useDownloadProgress();

  return (
    <SidebarProvider>
      <AppSidebar />
      <main className="w-full flex flex-col min-h-screen">
        <TopBar />
        <div className="p-6 lg:p-8 flex-1">
          <Outlet />
        </div>
        <Footer />
      </main>
    </SidebarProvider>
  );
}
