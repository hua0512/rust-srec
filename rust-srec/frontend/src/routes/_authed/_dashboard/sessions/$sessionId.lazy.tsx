import React, { useState, useEffect, Suspense } from 'react';
import { createLazyFileRoute } from '@tanstack/react-router';
import { useQuery } from '@tanstack/react-query';
import {
  getSession,
  getSessionDanmuStatistics,
} from '@/server/functions/sessions';
import {
  listPipelines,
  listPipelineOutputs,
} from '@/server/functions/pipeline';
import { Button } from '@/components/ui/button';
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card';
import { Badge } from '@/components/ui/badge';
import { Skeleton } from '@/components/ui/skeleton';
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Tabs, TabsList, TabsTrigger, TabsContent } from '@/components/ui/tabs';

import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { toast } from 'sonner';
import { ArrowLeft, AlertCircle } from 'lucide-react';
import { getMediaUrl } from '@/lib/url';
import { formatDuration } from '@/lib/format';
import { BackendApiError } from '@/server/api';
import { SessionHeader } from '@/components/sessions/session-header';
import { OverviewTab } from '@/components/sessions/overview-tab';
import { RecordingsTab } from '@/components/sessions/recordings-tab';
import { JobsTab } from '@/components/sessions/jobs-tab';
import { DanmuViewer } from '@/components/danmu/danmu-viewer';

import { TimelineTab } from '@/components/sessions/timeline-tab';

export const Route = createLazyFileRoute(
  '/_authed/_dashboard/sessions/$sessionId',
)({
  component: SessionDetailPage,
});

const PlayerCard = React.lazy(() =>
  import('@/components/player/player-card').then((module) => ({
    default: module.PlayerCard,
  })),
);

function SessionDetailPage() {
  const { sessionId } = Route.useParams();
  const { user } = Route.useRouteContext();
  const [playingOutput, setPlayingOutput] = useState<any>(null);

  const [now, setNow] = useState<Date | null>(null);
  useEffect(() => {
    setNow(new Date());
  }, []);

  const {
    data: session,
    isLoading: isSessionLoading,
    isError,
    error,
  } = useQuery({
    queryKey: ['session', sessionId],
    queryFn: () => getSession({ data: sessionId }),
  });

  const danmuStatsQuery = useQuery({
    queryKey: ['session', 'danmu-statistics', sessionId],
    queryFn: () => getSessionDanmuStatistics({ data: sessionId }),
    enabled: Boolean(sessionId),
    staleTime: 30000,
    retry: 1,
  });

  const isDanmuStatsUnavailable =
    danmuStatsQuery.error instanceof BackendApiError &&
    danmuStatsQuery.error.status === 404;

  const { data: outputsData, isLoading: isOutputsLoading } = useQuery({
    queryKey: ['pipeline', 'outputs', sessionId],
    queryFn: () => listPipelineOutputs({ data: { session_id: sessionId } }),
  });

  const { data: dagsData, isLoading: isDagsLoading } = useQuery({
    queryKey: ['pipeline', 'dags', sessionId],
    queryFn: () => listPipelines({ data: { session_id: sessionId } }),
  });

  const outputs = outputsData?.items || [];
  const dags = dagsData?.dags || [];

  const handleDownload = async (outputId: string, filename: string) => {
    try {
      const url = getMediaUrl(
        `/api/media/${outputId}/content`,
        user?.token?.access_token,
      );
      if (!url) throw new Error('Invalid download URL');

      toast.promise(
        async () => {
          const response = await fetch(url, {
            headers: {
              Authorization: `Bearer ${user?.token?.access_token}`,
            },
          });

          if (!response.ok) {
            throw new Error(
              `Download failed: ${response.status} ${response.statusText}`,
            );
          }

          const blob = await response.blob();
          const downloadUrl = window.URL.createObjectURL(blob);
          const a = document.createElement('a');
          a.href = downloadUrl;
          a.download = filename;
          document.body.appendChild(a);
          a.click();
          window.URL.revokeObjectURL(downloadUrl);
          document.body.removeChild(a);
        },
        {
          loading: 'Downloading...',
          success: 'Download started',
          error: (err) => `Download failed: ${err.message}`,
        },
      );
    } catch (error: any) {
      toast.error(error.message);
    }
  };

  const { i18n } = useLingui();

  if (isSessionLoading) {
    return (
      <div className="min-h-screen p-4 md:p-10 max-w-7xl mx-auto space-y-8 bg-background">
        <div className="flex flex-col md:flex-row items-center gap-6">
          <Skeleton className="h-16 w-16 rounded-xl" />
          <div className="space-y-2">
            <Skeleton className="h-8 w-64" />
            <Skeleton className="h-4 w-32" />
          </div>
        </div>
        <div className="grid grid-cols-1 md:grid-cols-3 gap-6">
          <Skeleton className="h-50 md:col-span-2 rounded-xl md:rounded-2xl" />
          <Skeleton className="h-50 rounded-xl md:rounded-2xl" />
        </div>
        <Skeleton className="h-100 rounded-xl md:rounded-2xl" />
      </div>
    );
  }

  if (isError || !session) {
    return (
      <div className="min-h-screen flex items-center justify-center p-6 bg-background">
        <Card className="max-w-md w-full border-destructive/20 bg-destructive/5 backdrop-blur-sm">
          <CardHeader className="text-center pb-2">
            <div className="mx-auto p-3 rounded-full bg-destructive/10 w-fit mb-2">
              <AlertCircle className="h-6 w-6 text-destructive" />
            </div>
            <CardTitle className="text-xl text-destructive">
              <Trans>Error Loading Session</Trans>
            </CardTitle>
            <CardDescription>
              {error?.message || i18n._(msg`Session not found`)}
            </CardDescription>
          </CardHeader>
          <CardContent className="flex justify-center pb-6">
            <Button variant="outline" onClick={() => window.history.back()}>
              <ArrowLeft className="mr-2 h-4 w-4" /> <Trans>Go Back</Trans>
            </Button>
          </CardContent>
        </Card>
      </div>
    );
  }

  const duration = session.duration_secs
    ? formatDuration(session.duration_secs)
    : session.start_time && now
      ? formatDuration(
          (now.getTime() - new Date(session.start_time).getTime()) / 1000,
        )
      : '-';

  return (
    <div className="relative min-h-screen bg-background overflow-hidden selection:bg-primary/20">
      <div className="fixed inset-0 pointer-events-none">
        <div className="absolute top-0 left-0 -mt-20 -ml-20 w-125 h-125 bg-primary/5 rounded-full blur-[120px]" />
        <div className="absolute bottom-0 right-0 -mb-40 -mr-20 w-150 h-150 bg-blue-500/5 rounded-full blur-[120px]" />
        <div
          className="absolute inset-0 opacity-[0.03]"
          style={{
            backgroundImage: `radial-gradient(#000 1px, transparent 1px)`,
            backgroundSize: '24px 24px',
          }}
        />
        <div
          className="absolute inset-0 opacity-[0.03] dark:invert"
          style={{
            backgroundImage: `radial-gradient(#fff 1px, transparent 1px)`,
            backgroundSize: '24px 24px',
          }}
        />
      </div>

      <div className="relative z-10 w-full px-4 py-6 pb-32 md:px-12 lg:px-16 xl:px-24">
        <SessionHeader session={session} />

        <Tabs defaultValue="overview" className="space-y-6 md:space-y-8">
          <div className="flex items-center justify-between -mx-4 px-4 md:mx-0 md:px-0 overflow-x-auto scrollbar-hide">
            <TabsList className="bg-secondary/50 backdrop-blur-sm border border-border/10 p-1 h-11 md:h-12 rounded-full gap-1 md:gap-2 min-w-max flex-nowrap">
              <TabsTrigger
                value="overview"
                className="rounded-full px-4 md:px-6 h-8 md:h-9 transition-all data-[state=active]:bg-background data-[state=active]:text-primary data-[state=active]:shadow-sm hover:text-foreground/80"
              >
                <Trans>Overview</Trans>
              </TabsTrigger>
              <TabsTrigger
                value="timeline"
                className="rounded-full px-4 md:px-6 h-8 md:h-9 transition-all data-[state=active]:bg-background data-[state=active]:text-primary data-[state=active]:shadow-sm hover:text-foreground/80 gap-2"
              >
                <Trans>Timeline</Trans>
                <Badge
                  variant="secondary"
                  className="rounded-full px-1.5 h-5 text-[10px] min-w-5 justify-center bg-muted/50 data-[state=active]:bg-primary/10 data-[state=active]:text-primary"
                >
                  {session.titles?.length || 0}
                </Badge>
              </TabsTrigger>
              <TabsTrigger
                value="recordings"
                className="rounded-full px-4 md:px-6 h-8 md:h-9 transition-all data-[state=active]:bg-background data-[state=active]:text-primary data-[state=active]:shadow-sm hover:text-foreground/80 gap-2"
              >
                <Trans>Recordings</Trans>
                <Badge
                  variant="secondary"
                  className="rounded-full px-1.5 h-5 text-[10px] min-w-5 justify-center bg-muted/50 data-[state=active]:bg-primary/10 data-[state=active]:text-primary"
                >
                  {outputs.length}
                </Badge>
              </TabsTrigger>
              <TabsTrigger
                value="jobs"
                className="rounded-full px-4 md:px-6 h-8 md:h-9 transition-all data-[state=active]:bg-background data-[state=active]:text-primary data-[state=active]:shadow-sm hover:text-foreground/80 gap-2"
              >
                <Trans>Jobs</Trans>
                <Badge
                  variant="secondary"
                  className="rounded-full px-1.5 h-5 text-[10px] min-w-5 justify-center bg-muted/50 data-[state=active]:bg-primary/10 data-[state=active]:text-primary"
                >
                  {dags.length}
                </Badge>
              </TabsTrigger>
            </TabsList>
          </div>

          <TabsContent
            value="overview"
            className="mt-6 focus-visible:outline-none"
          >
            <OverviewTab
              session={session}
              duration={duration}
              outputs={outputs}
              onPlay={setPlayingOutput}
              token={user?.token?.access_token}
              danmuStats={danmuStatsQuery.data}
              isDanmuStatsLoading={danmuStatsQuery.isLoading}
              isDanmuStatsError={danmuStatsQuery.isError}
              isDanmuStatsUnavailable={isDanmuStatsUnavailable}
              onRetryDanmuStats={() => {
                void danmuStatsQuery.refetch();
              }}
            />
          </TabsContent>

          <TabsContent
            value="timeline"
            className="mt-6 focus-visible:outline-none"
          >
            <TimelineTab session={session} />
          </TabsContent>

          <TabsContent
            value="recordings"
            className="mt-6 focus-visible:outline-none"
          >
            <RecordingsTab
              isLoading={isOutputsLoading}
              outputs={outputs}
              onDownload={handleDownload}
              onPlay={setPlayingOutput}
            />
          </TabsContent>

          <TabsContent value="jobs" className="mt-6 focus-visible:outline-none">
            <JobsTab isLoading={isDagsLoading} dags={dags} />
          </TabsContent>
        </Tabs>
      </div>

      <Dialog
        open={!!playingOutput}
        onOpenChange={(open) => !open && setPlayingOutput(null)}
      >
        <DialogContent
          className={
            playingOutput?.format === 'DANMU_XML'
              ? 'w-[95vw] sm:w-full max-w-5xl p-0 overflow-hidden bg-transparent border-0 shadow-none focus:outline-none'
              : 'w-[95vw] sm:w-full max-w-4xl p-0 overflow-hidden bg-black/95 border-border/20'
          }
        >
          <DialogHeader className="sr-only">
            <DialogTitle>Media Player</DialogTitle>
          </DialogHeader>
          <div
            className={
              playingOutput?.format === 'DANMU_XML'
                ? 'w-full flex items-center justify-center'
                : 'aspect-video w-full flex items-center justify-center'
            }
          >
            {playingOutput &&
              (playingOutput.format === 'DANMU_XML' ? (
                <DanmuViewer
                  url={
                    getMediaUrl(
                      `/api/media/${playingOutput.id}/content`,
                      user?.token?.access_token,
                    ) || ''
                  }
                  title={playingOutput.file_path.split('/').pop()}
                  onClose={() => setPlayingOutput(null)}
                />
              ) : (
                <Suspense
                  fallback={
                    <div className="w-full h-full flex items-center justify-center bg-muted/10">
                      <div className="h-8 w-8 animate-spin rounded-full border-2 border-primary border-t-transparent" />
                    </div>
                  }
                >
                  <PlayerCard
                    url={
                      getMediaUrl(
                        `/api/media/${playingOutput.id}/content`,
                        user?.token?.access_token,
                      ) || ''
                    }
                    title={playingOutput.file_path.split('/').pop()}
                    className="w-full h-full border-0 rounded-none bg-black"
                    contentClassName="min-h-0"
                    defaultWebFullscreen
                  />
                </Suspense>
              ))}
          </div>
        </DialogContent>
      </Dialog>
    </div>
  );
}
