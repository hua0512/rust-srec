import { createFileRoute, redirect } from '@tanstack/react-router'

export const Route = createFileRoute('/_authed/_dashboard/pipeline/')({
  beforeLoad: () => {
    throw redirect({
      to: '/pipeline/presets',
    })
  },
})

