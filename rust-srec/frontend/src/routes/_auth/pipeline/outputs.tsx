import { createFileRoute } from '@tanstack/react-router'

export const Route = createFileRoute('/_auth/pipeline/outputs')({
  component: RouteComponent,
})

function RouteComponent() {
  return <div>Hello "/pipeline/outputs"!</div>
}
