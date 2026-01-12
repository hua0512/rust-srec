import { createFileRoute, Outlet } from '@tanstack/react-router';

export const Route = createFileRoute('/_public')({
  beforeLoad: ({ context }) => {
    // Root route already validated/rotated tokens via `fetchUser()`.
    // Avoid re-calling `checkAuthFn()` here to reduce duplicate refresh races.
    return { session: context.user ?? null };
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
