import { createFileRoute, Outlet, redirect } from '@tanstack/react-router';
import { useAuthStore } from '../store/auth';
import { SidebarProvider } from '../components/ui/sidebar';
import { AppSidebar } from '../components/app-sidebar';
import { TopBar } from '../components/top-bar';
import { Footer } from '../components/footer';
import { useDownloadProgress } from '../hooks/useDownloadProgress';

export const Route = createFileRoute('/_auth')({
  beforeLoad: ({ location }) => {
    const { isAuthenticated, user } = useAuthStore.getState();

    // Only check auth on client side
    if (typeof window !== 'undefined' && !isAuthenticated) {
      throw redirect({
        to: '/login',
        search: {
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
  // Requirements: 1.1, 1.2 - Establish connection when user is authenticated
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
