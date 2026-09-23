import { createFileRoute, redirect } from '@tanstack/react-router';
import { sessionQueryOptions } from '@/api/session';

export const Route = createFileRoute('/_public/login')({
  validateSearch: (search: Record<string, unknown>): { redirect?: string } => {
    return {
      redirect:
        typeof search.redirect === 'string' ? search.redirect : undefined,
    };
  },
  beforeLoad: async ({ context }) => {
    // Same cached check as the `/_authed` guard; a signed-out result is never
    // reused, so reaching this page after a sign-out always re-checks.
    const user = await context.queryClient.fetchQuery(sessionQueryOptions);
    if (user && !user.mustChangePassword) {
      throw redirect({ to: '/dashboard' });
    }
  },
});
