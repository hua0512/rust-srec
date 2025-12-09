import { createFileRoute, Outlet } from '@tanstack/react-router'
import DashboardPanelLayout from '@/components/sidebar/panel_layout'
import { useDownloadProgress } from '@/hooks/use-download-progress'

export const Route = createFileRoute('/_authed/_dashboard')({
  component: DashboardLayout,
})

function DashboardLayout() {
  useDownloadProgress()

  return (
    <DashboardPanelLayout>
      <Outlet />
    </DashboardPanelLayout>
  )
}
