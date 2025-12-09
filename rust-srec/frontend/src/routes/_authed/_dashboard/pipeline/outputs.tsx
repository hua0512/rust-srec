import { createFileRoute } from '@tanstack/react-router'

export const Route = createFileRoute('/_authed/_dashboard/pipeline/outputs')({
  component: RouteComponent,
})

function RouteComponent() {
  return <div>Hello "/pipeline/outputs"!</div>
}
