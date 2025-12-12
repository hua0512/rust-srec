import { createFileRoute, Outlet } from '@tanstack/react-router';

export const Route = createFileRoute('/_authed/_dashboard/pipeline/jobs')({
    component: () => <Outlet />,
});
