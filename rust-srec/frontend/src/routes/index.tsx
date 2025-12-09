import { createFileRoute, redirect } from '@tanstack/react-router'

export const Route = createFileRoute('/')({
  beforeLoad: ({ context, location }) => {
    if (context.auth.isAuthenticated) {
      console.log('Index Route: Authenticated, redirecting to /dashboard');
      throw redirect({
        to: '/dashboard',
        replace: true,
      })
    } else {
      console.log('Index Route: Not authenticated, redirecting to /login');
      throw redirect({
        to: '/login',
        search: {
          redirect: location.href,
        },
      })
    }
  },
})
