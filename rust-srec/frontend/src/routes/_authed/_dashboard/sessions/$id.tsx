import { createFileRoute } from '@tanstack/react-router'

export const Route = createFileRoute('/_authed/_dashboard/sessions/$id')({
  component: RouteComponent,
})

function RouteComponent() {
  return <div>Hello "/sessions/$id"!</div>
}
