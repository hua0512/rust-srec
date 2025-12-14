import { createFileRoute, Link } from '@tanstack/react-router';
import { useQuery } from '@tanstack/react-query';
import { getSession } from '@/server/functions/sessions';
import {
  listPipelineJobs,
  listPipelineOutputs,
} from '@/server/functions/pipeline';
import { Button } from '@/components/ui/button';
import {
  ChevronLeft,
  Download,
  FileVideo,
  Activity,
  Clock,
  Server,
  Monitor,
  Play,
  AlertCircle,
  CheckCircle2,
  History,
} from 'lucide-react';
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  CardDescription,
} from '@/components/ui/card';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { Badge } from '@/components/ui/badge';
import { Skeleton } from '@/components/ui/skeleton';
import { format, formatDistanceToNow } from 'date-fns';
import { Trans } from '@lingui/react/macro';
import { Avatar, AvatarFallback, AvatarImage } from '@/components/ui/avatar';
import { cn } from '@/lib/utils';
import { toast } from 'sonner';
import { BASE_URL } from '@/utils/env';

export const Route = createFileRoute('/_authed/_dashboard/sessions/$sessionId')(
  {
    component: SessionDetailPage,
  },
);

function SessionDetailPage() {
  const { sessionId } = Route.useParams();
  const { user } = Route.useRouteContext();

  const { data: session, isLoading: isSessionLoading } = useQuery({
    queryKey: ['session', sessionId],
    queryFn: () => getSession({ data: sessionId }),
  });

  const { data: outputsData, isLoading: isOutputsLoading } = useQuery({
    queryKey: ['pipeline', 'outputs', sessionId],
    queryFn: () => listPipelineOutputs({ data: { session_id: sessionId } }),
  });

  const { data: jobsData, isLoading: isJobsLoading } = useQuery({
    queryKey: ['pipeline', 'jobs', sessionId],
    queryFn: () => listPipelineJobs({ data: { session_id: sessionId } }),
  });

  const outputs = outputsData?.items || [];
  const jobs = jobsData?.items || [];

  if (isSessionLoading) {
    return <SessionDetailSkeleton />;
  }

  if (!session) {
    return (
      <div className="flex flex-col items-center justify-center h-[50vh] gap-4">
        <AlertCircle className="h-12 w-12 text-destructive/50" />
        <h2 className="text-2xl font-bold tracking-tight">
          <Trans>Session Not Found</Trans>
        </h2>
        <Button asChild variant="secondary">
          <Link to="/sessions">
            <Trans>Back to Sessions</Trans>
          </Link>
        </Button>
      </div>
    );
  }

  const duration = session.duration_secs
    ? formatDuration(session.duration_secs)
    : session.start_time
      ? formatDuration(
          (new Date().getTime() - new Date(session.start_time).getTime()) /
            1000,
        )
      : '-';

  const handleDownload = async (outputId: string, filename: string) => {
    try {
      const baseUrl = BASE_URL.endsWith('/') ? BASE_URL.slice(0, -1) : BASE_URL;
      const url = `${baseUrl}/media/${outputId}/content`;

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

  return (
    <div className="relative min-h-screen pb-20">
      {/* Immersive Header Background */}
      <div className="absolute inset-0 h-[500px] overflow-hidden -z-10 select-none pointer-events-none">
        <div className="absolute inset-0 bg-linear-to-b from-background/0 via-background/80 to-background/100 z-10" />
        {session.thumbnail_url ? (
          <img
            src={session.thumbnail_url}
            alt=""
            className="w-full h-full object-cover opacity-20 blur-3xl scale-110"
          />
        ) : (
          <div className="w-full h-full bg-linear-to-br from-primary/10 to-background opacity-50 blur-3xl" />
        )}
      </div>

      <div className="w-full p-4 animate-in fade-in slide-in-from-bottom-4 duration-700">
        {/* Navigation & Actions */}
        <div className="flex items-center justify-between mb-4">
          <Button
            variant="ghost"
            size="sm"
            className="gap-2 text-muted-foreground hover:bg-background/20 backdrop-blur-md transition-colors"
            asChild
          >
            <Link to="/sessions">
              <ChevronLeft className="h-4 w-4" />
              <Trans>Back to Sessions</Trans>
            </Link>
          </Button>
        </div>

        {/* Main Header Information */}
        <div className="flex flex-col lg:flex-row gap-8 items-start mb-12">
          <div className="shrink-0 relative">
            <div className="absolute -inset-0.5 bg-linear-to-b from-primary/20 to-transparent rounded-full blur-sm" />
            <Avatar className="h-32 w-32 border-4 border-background shadow-xl">
              {session.streamer_avatar && (
                <AvatarImage
                  src={session.streamer_avatar}
                  alt={session.streamer_name}
                  className="object-cover"
                />
              )}
              <AvatarFallback className="text-4xl font-bold bg-muted text-muted-foreground">
                {session.streamer_name.substring(0, 2).toUpperCase()}
              </AvatarFallback>
            </Avatar>
            {session.end_time ? (
              <div className="absolute bottom-1 right-1 bg-background rounded-full p-1.5 shadow-sm border">
                <CheckCircle2 className="h-6 w-6 text-green-500 fill-green-500/20" />
              </div>
            ) : (
              <div className="absolute bottom-1 right-1">
                <span className="relative flex h-8 w-8">
                  <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-red-400 opacity-75"></span>
                  <span className="relative inline-flex rounded-full h-8 w-8 bg-red-500 border-4 border-background" />
                </span>
              </div>
            )}
          </div>

          <div className="flex-1 space-y-4">
            <div className="space-y-2">
              <div className="flex items-center gap-3 flex-wrap">
                <h1 className="text-4xl md:text-5xl font-bold tracking-tight bg-clip-text text-transparent bg-linear-to-r from-foreground to-foreground/70">
                  {session.title || (
                    <span className="italic opacity-50">Untitled Session</span>
                  )}
                </h1>
              </div>

              <div className="flex items-center gap-4 text-muted-foreground flex-wrap">
                <div className="flex items-center gap-2 px-3 py-1 rounded-full bg-background/40 backdrop-blur-sm border border-border/50">
                  <span className="font-semibold text-foreground">
                    {session.streamer_name}
                  </span>
                </div>
                <span className="text-border">|</span>
                <div className="flex items-center gap-2">
                  <Clock className="h-4 w-4" />
                  <span>{format(new Date(session.start_time), 'PPP p')}</span>
                </div>
                {session.duration_secs !== null && (
                  <>
                    <span className="text-border">|</span>
                    <Badge
                      variant="secondary"
                      className="font-mono text-sm bg-background/50 backdrop-blur-md"
                    >
                      {duration}
                    </Badge>
                  </>
                )}
              </div>
            </div>
          </div>
        </div>

        {/* Content Tabs */}
        <Tabs defaultValue="overview" className="space-y-8">
          <div className="sticky top-4 z-50 bg-background/80 backdrop-blur-xl p-1.5 rounded-full border shadow-sm inline-flex w-full md:w-auto overflow-x-auto">
            <TabsList className="bg-transparent p-0 h-auto w-full md:w-auto gap-2">
              <TabsTrigger
                value="overview"
                className="rounded-full px-6 py-2.5 data-[state=active]:bg-primary data-[state=active]:text-primary-foreground transition-all"
              >
                <Activity className="h-4 w-4 mr-2" />
                <Trans>Overview</Trans>
              </TabsTrigger>
              <TabsTrigger
                value="content"
                className="rounded-full px-6 py-2.5 data-[state=active]:bg-primary data-[state=active]:text-primary-foreground transition-all"
              >
                <FileVideo className="h-4 w-4 mr-2" />
                <Trans>Content</Trans>
                <Badge
                  variant="secondary"
                  className="ml-2 bg-background/20 text-current h-5 px-1.5 min-w-[1.25rem] text-[10px]"
                >
                  {outputs.length}
                </Badge>
              </TabsTrigger>
              <TabsTrigger
                value="processing"
                className="rounded-full px-6 py-2.5 data-[state=active]:bg-primary data-[state=active]:text-primary-foreground transition-all"
              >
                <Server className="h-4 w-4 mr-2" />
                <Trans>Processing</Trans>
                <Badge
                  variant="secondary"
                  className="ml-2 bg-background/20 text-current h-5 px-1.5 min-w-[1.25rem] text-[10px]"
                >
                  {jobs.length}
                </Badge>
              </TabsTrigger>
              <TabsTrigger
                value="history"
                className="rounded-full px-6 py-2.5 data-[state=active]:bg-primary data-[state=active]:text-primary-foreground transition-all"
              >
                <History className="h-4 w-4 mr-2" />
                <Trans>History</Trans>
              </TabsTrigger>
            </TabsList>
          </div>

          <TabsContent
            value="overview"
            className="space-y-8 animate-in slide-in-from-bottom-2 duration-500"
          >
            {/* Key Stats Grid */}
            <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-4">
              <StatsCard
                title="Total Size"
                value={formatBytes(session.total_size_bytes)}
                icon={Server}
                description="Disk usage"
              />
              <StatsCard
                title="Outputs"
                value={session.output_count}
                icon={FileVideo}
                description="Generated files"
              />
              <StatsCard
                title="Danmu"
                value={session.danmu_count?.toLocaleString() || 0}
                icon={Activity}
                description="Chat messages"
              />
              <StatsCard
                title="Start Time"
                value={format(new Date(session.start_time), 'HH:mm')}
                icon={Clock}
                description={formatDistanceToNow(new Date(session.start_time), {
                  addSuffix: true,
                })}
              />
            </div>

            {/* Preview Section */}
            <Card className="overflow-hidden border-border/50 shadow-lg bg-card/40 backdrop-blur-xs">
              <CardHeader>
                <CardTitle className="flex items-center gap-2">
                  <Monitor className="h-5 w-5 text-primary" />
                  <Trans>Session Preview</Trans>
                </CardTitle>
              </CardHeader>
              <CardContent className="p-0">
                <div className="aspect-video bg-muted/30 flex items-center justify-center relative group overflow-hidden rounded-md">
                  {session.thumbnail_url ? (
                    <>
                      <img
                        src={session.thumbnail_url}
                        alt="Session thumbnail"
                        className="w-full h-full object-cover transition-transform duration-700 group-hover:scale-105"
                      />
                      <div className="absolute inset-0 bg-black/60 opacity-0 group-hover:opacity-100 transition-opacity duration-300 flex flex-col items-center justify-center gap-3">
                        <Button
                          size="lg"
                          className="rounded-full h-16 w-16 p-0"
                          variant="outline"
                        >
                          <Play className="h-8 w-8 fill-current ml-1" />
                        </Button>
                        <p className="text-white/80 font-medium tracking-wide">
                          <Trans>Preview Only</Trans>
                        </p>
                      </div>
                    </>
                  ) : (
                    <div className="flex flex-col items-center gap-4 text-muted-foreground/50">
                      <Monitor className="h-16 w-16" />
                      <p className="font-medium">
                        <Trans>Preview not available</Trans>
                      </p>
                    </div>
                  )}
                </div>
              </CardContent>
            </Card>
          </TabsContent>

          <TabsContent
            value="content"
            className="animate-in slide-in-from-bottom-2 duration-500"
          >
            <div className="grid lg:grid-cols-3 gap-8">
              <Card className="lg:col-span-2 border-border/50 shadow-sm bg-card/40 backdrop-blur-xs h-fit">
                <CardHeader>
                  <CardTitle className="text-xl">
                    <Trans>Media Files</Trans>
                  </CardTitle>
                  <CardDescription>
                    <Trans>Download or view recorded content</Trans>
                  </CardDescription>
                </CardHeader>
                <CardContent>
                  {isOutputsLoading ? (
                    <div className="space-y-4">
                      <Skeleton className="h-16 w-full" />
                      <Skeleton className="h-16 w-full" />
                    </div>
                  ) : outputs.length === 0 ? (
                    <EmptyState
                      icon={FileVideo}
                      title="No outputs generated"
                      description="Files will appear here once processing is complete."
                    />
                  ) : (
                    <div className="space-y-3">
                      {outputs.map((output: any) => (
                        <div
                          key={output.id}
                          className="group flex items-center justify-between p-4 border rounded-xl bg-background/50 hover:bg-accent/5 transition-all hover:border-primary/20 hover:shadow-md"
                        >
                          <div className="flex items-center gap-4 overflow-hidden">
                            <div className="h-12 w-12 rounded-lg bg-primary/10 flex items-center justify-center shrink-0">
                              <FileVideo className="h-6 w-6 text-primary" />
                            </div>
                            <div className="min-w-0">
                              <p className="font-medium truncate">
                                {output.file_path.split('/').pop()}
                              </p>
                              <div className="flex items-center gap-3 text-xs text-muted-foreground mt-1">
                                <Badge
                                  variant="outline"
                                  className="text-[10px] px-1.5 h-5 uppercase tracking-wider"
                                >
                                  {output.format}
                                </Badge>
                                <span>
                                  {formatBytes(output.file_size_bytes)}
                                </span>
                                <span className="text-border">•</span>
                                <span>
                                  {formatDistanceToNow(
                                    new Date(output.created_at),
                                    {
                                      addSuffix: true,
                                    },
                                  )}
                                </span>
                              </div>
                            </div>
                          </div>
                          <Button
                            variant="outline"
                            size="sm"
                            className="ml-4 gap-2 hover:bg-primary hover:text-primary-foreground transition-colors"
                            onClick={() =>
                              handleDownload(
                                output.id,
                                output.file_path.split('/').pop() || 'video',
                              )
                            }
                          >
                            <Download className="h-4 w-4" />
                            <span className="hidden sm:inline">
                              <Trans>Download</Trans>
                            </span>
                          </Button>
                        </div>
                      ))}
                    </div>
                  )}
                </CardContent>
              </Card>

              <div className="space-y-6">
                <Card className="border-border/50 shadow-sm bg-card/40 backdrop-blur-xs">
                  <CardHeader>
                    <CardTitle className="text-lg">
                      <Trans>File Summary</Trans>
                    </CardTitle>
                  </CardHeader>
                  <CardContent className="space-y-4">
                    <div className="flex justify-between items-center py-2 border-b">
                      <span className="text-muted-foreground">
                        <Trans>Total Files</Trans>
                      </span>
                      <span className="font-mono font-medium">
                        {outputs.length}
                      </span>
                    </div>
                    <div className="flex justify-between items-center py-2 border-b">
                      <span className="text-muted-foreground">
                        <Trans>Total Size</Trans>
                      </span>
                      <span className="font-mono font-medium">
                        {formatBytes(session.total_size_bytes)}
                      </span>
                    </div>
                    <div className="flex justify-between items-center py-2">
                      <span className="text-muted-foreground">
                        <Trans>Formats</Trans>
                      </span>
                      <div className="flex gap-1">
                        {Array.from(
                          new Set(outputs.map((o: any) => o.format)),
                        ).map((fmt: any) => (
                          <Badge
                            key={fmt}
                            variant="secondary"
                            className="text-[10px]"
                          >
                            {fmt}
                          </Badge>
                        ))}
                      </div>
                    </div>
                  </CardContent>
                </Card>
              </div>
            </div>
          </TabsContent>

          <TabsContent
            value="processing"
            className="animate-in slide-in-from-bottom-2 duration-500"
          >
            <Card className="border-border/50 shadow-sm bg-card/40 backdrop-blur-xs">
              <CardHeader>
                <CardTitle className="text-xl">
                  <Trans>Job Pipeline</Trans>
                </CardTitle>
                <CardDescription>
                  <Trans>Track processing status and history</Trans>
                </CardDescription>
              </CardHeader>
              <CardContent>
                {isJobsLoading ? (
                  <div className="space-y-4">
                    <Skeleton className="h-16 w-full" />
                    <Skeleton className="h-16 w-full" />
                    <Skeleton className="h-16 w-full" />
                  </div>
                ) : jobs.length === 0 ? (
                  <EmptyState
                    icon={Server}
                    title="No jobs found"
                    description="No processing jobs have been triggered for this session."
                  />
                ) : (
                  <div className="space-y-4 relative before:absolute before:inset-0 before:ml-8 before:w-0.5 before:-translate-x-px before:bg-gradient-to-b before:from-border before:via-border/50 before:to-transparent">
                    {jobs.map((job: any) => (
                      <div
                        key={job.id}
                        className="relative flex items-start gap-6 group"
                      >
                        <div
                          className={cn(
                            'absolute left-8 -translate-x-1/2 mt-3 h-3 w-3 rounded-full border-2 ring-4 ring-background transition-colors',
                            job.status === 'Completed'
                              ? 'bg-green-500 border-green-500'
                              : job.status === 'Failed'
                                ? 'bg-red-500 border-red-500'
                                : job.status === 'Processing'
                                  ? 'bg-blue-500 border-blue-500 animate-pulse'
                                  : 'bg-muted border-muted-foreground',
                          )}
                        />

                        <div className="flex-1 ml-10 p-4 border rounded-xl bg-background/50 hover:bg-accent/5 transition-all hover:border-primary/20 hover:shadow-sm">
                          <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-4">
                            <div>
                              <div className="flex items-center gap-2 mb-1">
                                <h4 className="font-semibold text-foreground">
                                  {job.step}
                                </h4>
                                <Badge
                                  variant={
                                    job.status === 'Completed'
                                      ? 'default'
                                      : job.status === 'Failed'
                                        ? 'destructive'
                                        : 'secondary'
                                  }
                                  className={cn(
                                    'text-[10px] h-5',
                                    job.status === 'Completed' &&
                                      'bg-green-500/15 text-green-600 hover:bg-green-500/25 border-green-500/20',
                                  )}
                                >
                                  {job.status}
                                </Badge>
                              </div>
                              <p className="text-xs text-muted-foreground font-mono">
                                ID: {job.id}
                              </p>
                            </div>

                            <div className="text-right text-xs text-muted-foreground">
                              <div className="flex items-center gap-1 justify-end">
                                <Clock className="h-3 w-3" />
                                <span>
                                  created{' '}
                                  {formatDistanceToNow(
                                    new Date(job.created_at),
                                    {
                                      addSuffix: true,
                                    },
                                  )}
                                </span>
                              </div>
                              {job.started_at && (
                                <p>
                                  started{' '}
                                  {format(new Date(job.started_at), 'HH:mm:ss')}
                                </p>
                              )}
                            </div>
                          </div>

                          {job.error_message && (
                            <div className="mt-3 p-3 rounded-md bg-destructive/10 border border-destructive/20 text-destructive text-sm flex items-start gap-2">
                              <AlertCircle className="h-4 w-4 shrink-0 mt-0.5" />
                              <p>{job.error_message}</p>
                            </div>
                          )}
                        </div>
                      </div>
                    ))}
                  </div>
                )}
              </CardContent>
            </Card>
          </TabsContent>

          <TabsContent
            value="history"
            className="animate-in slide-in-from-bottom-2 duration-500"
          >
            <Card className="border-border/50 shadow-sm bg-card/40 backdrop-blur-xs">
              <CardHeader>
                <CardTitle className="text-xl">
                  <Trans>Timeline</Trans>
                </CardTitle>
                <CardDescription>
                  <Trans>Session events and title changes</Trans>
                </CardDescription>
              </CardHeader>
              <CardContent>
                <div className="space-y-8 relative pl-6 before:absolute before:inset-y-0 before:left-2 before:w-0.5 before:bg-gradient-to-b before:from-primary before:via-primary/20 before:to-transparent">
                  {session.titles && session.titles.length > 0 ? (
                    session.titles.map((change: any, i: number) => (
                      <div key={i} className="relative group">
                        <div className="absolute -left-[23px] mt-1.5 h-4 w-4 rounded-full border-4 border-background bg-primary shadow-sm group-hover:scale-125 transition-transform" />
                        <div className="p-4 rounded-xl border bg-background/50 hover:bg-accent/5 transition-colors">
                          <p className="text-base font-medium leading-relaxed">
                            <span className="text-muted-foreground font-normal mr-2">
                              <Trans>Renamed to:</Trans>
                            </span>
                            "{change.title}"
                          </p>
                          <p className="text-xs text-muted-foreground mt-2 flex items-center gap-1.5">
                            <Clock className="h-3 w-3" />
                            {format(new Date(change.timestamp), 'PPpp')}
                          </p>
                        </div>
                      </div>
                    ))
                  ) : (
                    <div className="text-muted-foreground italic pl-4">
                      <Trans>No title changes recorded.</Trans>
                    </div>
                  )}

                  {/* Start Event */}
                  <div className="relative group">
                    <div className="absolute -left-[23px] mt-1.5 h-4 w-4 rounded-full border-4 border-background bg-green-500 shadow-sm" />
                    <div className="p-4 rounded-xl border bg-green-500/5 border-green-500/20">
                      <p className="text-base font-medium text-green-700 dark:text-green-400">
                        <Trans>Session Started</Trans>
                      </p>
                      <p className="text-xs text-muted-foreground mt-1 text-green-600/70 dark:text-green-400/70">
                        {format(new Date(session.start_time), 'PPpp')}
                      </p>
                    </div>
                  </div>
                </div>
              </CardContent>
            </Card>
          </TabsContent>
        </Tabs>
      </div>
    </div>
  );
}

// --- Specialized Components ---

function StatsCard({
  title,
  value,
  icon: Icon,
  description,
}: {
  title: string;
  value: string | number;
  icon: any;
  description: string;
}) {
  return (
    <Card className="border-border/50 shadow-sm bg-card/60 backdrop-blur-xs hover:bg-card/80 transition-colors">
      <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
        <CardTitle className="text-sm font-medium text-muted-foreground">
          {title}
        </CardTitle>
        <Icon className="h-4 w-4 text-muted-foreground" />
      </CardHeader>
      <CardContent>
        <div className="text-2xl font-bold tracking-tight">{value}</div>
        <p className="text-xs text-muted-foreground mt-1">{description}</p>
      </CardContent>
    </Card>
  );
}

function EmptyState({
  icon: Icon,
  title,
  description,
}: {
  icon: any;
  title: string;
  description: string;
}) {
  return (
    <div className="flex flex-col items-center justify-center py-12 text-center">
      <div className="bg-muted/50 p-4 rounded-full mb-4">
        <Icon className="h-8 w-8 text-muted-foreground/50" />
      </div>
      <h3 className="text-lg font-semibold">{title}</h3>
      <p className="text-muted-foreground text-sm max-w-sm mt-1">
        {description}
      </p>
    </div>
  );
}

function SessionDetailSkeleton() {
  return (
    <div className="min-h-screen pb-20 relative">
      <div className="absolute inset-0 h-[400px] bg-muted/20 -z-10" />
      <div className="container mx-auto p-6 max-w-7xl">
        <div className="mb-8">
          <Skeleton className="h-10 w-32" />
        </div>

        <div className="flex gap-8 mb-12">
          <Skeleton className="h-32 w-32 rounded-full" />
          <div className="space-y-4 flex-1">
            <Skeleton className="h-12 w-3/4 max-w-xl" />
            <div className="flex gap-4">
              <Skeleton className="h-6 w-24" />
              <Skeleton className="h-6 w-32" />
            </div>
          </div>
        </div>

        <Skeleton className="h-12 w-full max-w-md rounded-full mb-8" />

        <div className="grid gap-4 md:grid-cols-4 mb-8">
          {Array.from({ length: 4 }).map((_, i) => (
            <Skeleton key={i} className="h-32 rounded-xl" />
          ))}
        </div>

        <Skeleton className="h-96 rounded-xl" />
      </div>
    </div>
  );
}

function formatBytes(bytes: number) {
  if (bytes === 0) return '0 B';
  const k = 1024;
  const sizes = ['B', 'KB', 'MB', 'GB', 'TB'];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
}

function formatDuration(seconds: number) {
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = Math.floor(seconds % 60);
  return `${h > 0 ? h + 'h ' : ''}${m}m ${s}s`;
}
