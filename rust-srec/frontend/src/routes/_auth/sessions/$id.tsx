import { createFileRoute } from '@tanstack/react-router'

export const Route = createFileRoute('/_auth/sessions/$id')({
  component: RouteComponent,
})

function RouteComponent() {
  return <div>Hello "/sessions/$id"!</div>
}
