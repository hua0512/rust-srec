import { createFileRoute, Outlet } from '@tanstack/react-router';
import { queryClient } from './__root';
import { sessionQueryOptions } from '../api/session';

export const Route = createFileRoute('/_public')({
  beforeLoad: async () => {
    // Check session
    // We don't redirect here to avoid swallowing Set-Cookie headers on the server
    const session = await queryClient.ensureQueryData(sessionQueryOptions);
    return { session };
  },
  component: PublicLayout,
});

function PublicLayout() {
  // const { session } = Route.useRouteContext()

  // if (session) {
  //   // Redirect to dashboard if authenticated
  //   return <Navigate to="/dashboard" />
  // }

  return <Outlet />;
}
