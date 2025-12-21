import { createFileRoute, Outlet } from '@tanstack/react-router';
import DashboardPanelLayout from '@/components/sidebar/panel_layout';

export const Route = createFileRoute('/_authed/_dashboard')({
  component: DashboardLayout,
});

function DashboardLayout() {
  return (
    <DashboardPanelLayout>
      <Outlet />
    </DashboardPanelLayout>
  );
}
