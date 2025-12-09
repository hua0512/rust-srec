import { createFileRoute } from '@tanstack/react-router';
import { useQuery } from '@tanstack/react-query';
import { engineApi } from '@/api/endpoints';
import { EngineCard, CreateEngineCard } from '@/components/config/engines/engine-card';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { Loader2, AlertCircle } from 'lucide-react';
import { Trans } from "@lingui/react/macro";

export const Route = createFileRoute('/_auth/config/engines')({
  component: EnginesPage,
});

function EnginesPage() {
  const { data: engines, isLoading, error } = useQuery({
    queryKey: ['engines'],
    queryFn: engineApi.list,
  });

  if (isLoading) {
    return (
      <div className="flex h-[50vh] w-full items-center justify-center">
        <Loader2 className="h-8 w-8 animate-spin text-muted-foreground" />
      </div>
    );
  }

  if (error) {
    return (
      <Alert variant="destructive">
        <AlertCircle className="h-4 w-4" />
        <AlertTitle><Trans>Error</Trans></AlertTitle>
        <AlertDescription>
          <Trans>Failed to load engines: {error.message}</Trans>
        </AlertDescription>
      </Alert>
    );
  }

  return (
    <div className="space-y-6 p-6">
      <div className="grid gap-6 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4">
        {engines?.map((engine) => (
          <EngineCard key={engine.id} engine={engine} />
        ))}
        <CreateEngineCard />
      </div>
    </div>
  );
}
