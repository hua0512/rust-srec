import { createFileRoute, redirect } from '@tanstack/react-router';

export const Route = createFileRoute('/_authed/_dashboard/config')({
  beforeLoad: ({ location }) => {
    if (location.pathname === '/config') {
      throw redirect({
        to: '/config/global',
      });
    }
  },
});
