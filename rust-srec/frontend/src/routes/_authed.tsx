import { createFileRoute, Outlet } from '@tanstack/react-router'
import { redirect } from '@tanstack/react-router'
import { WebSocketProvider } from '@/providers/WebSocketProvider'

export const Route = createFileRoute('/_authed')({
  beforeLoad: ({ context }) => {
    console.log("context : ", context)
    if (!context.user) {
      throw redirect({ to: '/login' })
    }
  },
  component: () => (
    <WebSocketProvider>
      <Outlet />
    </WebSocketProvider>
  ),
})