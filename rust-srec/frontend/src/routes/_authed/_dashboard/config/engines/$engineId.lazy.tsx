import { createLazyFileRoute, useNavigate } from '@tanstack/react-router';
import { queryKeys } from '@/api/query-keys';
import { EngineEditor } from '@/components/config/engines/engine-editor';
import { useQuery } from '@tanstack/react-query';
import { getEngine } from '@/server/functions';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { toast } from 'sonner';
import { Skeleton } from '@/components/ui/skeleton';
import { Trans } from '@lingui/react/macro';
import { DetailLoadError } from '@/components/shared/detail-page';

export const Route = createLazyFileRoute(
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
    queryKey: queryKeys.engines.detail(engineId),
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
      <DetailLoadError
        title={<Trans>Error loading engine</Trans>}
        error={error}
        notFound={<Trans>Engine not found</Trans>}
        back={{
          to: '/config/engines',
          label: <Trans>Back to engines</Trans>,
        }}
      />
    );
  }

  return (
    <div className="max-w-8xl p-4 sm:p-6 lg:p-8 pb-32">
      <EngineEditor
        engine={engine}
        onSuccess={() => {
          toast.success(i18n._(msg`Engine updated successfully`));
          void navigate({ to: '/config/engines' });
        }}
      />
    </div>
  );
}
