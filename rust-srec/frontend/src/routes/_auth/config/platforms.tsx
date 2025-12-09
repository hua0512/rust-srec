import { createFileRoute } from '@tanstack/react-router';
import { useQuery } from '@tanstack/react-query';
import { configApi } from '../../../api/endpoints';
import { Card, CardContent, CardHeader } from '../../../components/ui/card';
import { Skeleton } from '../../../components/ui/skeleton';
import { PlatformCard } from '../../../components/config/platforms/platform-card';

export const Route = createFileRoute('/_auth/config/platforms')({
  component: PlatformsConfigPage,
});

function PlatformsConfigPage() {
  const { data: platforms, isLoading } = useQuery({
    queryKey: ['config', 'platforms'],
    queryFn: configApi.listPlatforms,
  });

  return (
    <div className="space-y-6">
      {/* <SupportedPlatforms /> */}
      <div className="grid gap-6 grid-cols-1 md:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4">
        {isLoading ? (
          Array.from({ length: 4 }).map((_, i) => (
            <Card key={i} className="overflow-hidden">
              <CardHeader className="pb-4">
                <div className="flex items-center justify-between">
                  <Skeleton className="h-6 w-24" />
                  <Skeleton className="h-8 w-8 rounded-full" />
                </div>
              </CardHeader>
              <CardContent className="space-y-3">
                <Skeleton className="h-4 w-full" />
                <Skeleton className="h-4 w-3/4" />
                <Skeleton className="h-10 w-full mt-4" />
              </CardContent>
            </Card>
          ))
        ) : platforms?.map((platform) => (
          <PlatformCard key={platform.id} platform={platform} />
        ))}
      </div>
    </div>
  );
}
