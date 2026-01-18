import { createFileRoute, useNavigate } from '@tanstack/react-router';
import { EngineEditor } from '@/components/config/engines/engine-editor';
import { useQuery } from '@tanstack/react-query';
import { getEngine } from '@/server/functions';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { toast } from 'sonner';
import { Skeleton } from '@/components/ui/skeleton';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { Trans } from '@lingui/react/macro';
import { AlertCircle } from 'lucide-react';

export const Route = createFileRoute(
  '/_authed/_dashboard/config/engines/$engineId',
)({
  component: EditEnginePage,
});

function EditEnginePage() {
  const { engineId } = Route.useParams();
  const navigate = useNavigate();
  const { i18n } = useLingui();

  const {
    data: engine,
    isLoading,
    error,
  } = useQuery({
    queryKey: ['engine', engineId],
    queryFn: () => getEngine({ data: engineId }),
  });

  if (isLoading) {
    return (
      <div className="max-w-5xl mx-auto p-4 sm:p-6 lg:p-8 space-y-6">
        <Skeleton className="h-12 w-1/3 mb-8" />
        <Skeleton className="h-[200px] w-full rounded-xl" />
        <Skeleton className="h-[300px] w-full rounded-xl" />
      </div>
    );
  }

  if (error || !engine) {
    return (
      <div className="max-w-5xl mx-auto p-4 sm:p-6 lg:p-8">
        <Alert variant="destructive">
          <AlertCircle className="h-4 w-4" />
          <AlertTitle>
            <Trans>Error</Trans>
          </AlertTitle>
          <AlertDescription>
            <Trans>
              Failed to load engine: {error?.message || 'Engine not found'}
            </Trans>
          </AlertDescription>
        </Alert>
      </div>
    );
  }

  return (
    <div className="max-w-8xl p-4 sm:p-6 lg:p-8 pb-32">
      <EngineEditor
        engine={engine}
        onSuccess={() => {
          toast.success(i18n._(msg`Engine updated successfully`));
          navigate({ to: '/config/engines' });
        }}
      />
    </div>
  );
}
