import { createFileRoute, redirect } from '@tanstack/react-router'
export const Route = createFileRoute('/')({
  beforeLoad(ctx) {
    throw redirect(
      {
        to: '/dashboard'
      }
    )
  },
})