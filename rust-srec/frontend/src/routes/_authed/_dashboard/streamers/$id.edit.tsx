import { lazy, Suspense, useMemo } from 'react';
import { createFileRoute } from '@tanstack/react-router';
import { useQuery } from '@tanstack/react-query';
import { Trans } from '@lingui/react/macro';
import { Skeleton } from '@/components/ui/skeleton';
import { motion } from 'motion/react';

const StreamerGeneralSettings = lazy(() =>
  import('@/components/streamers/config/streamer-general-settings').then(
    (m) => ({ default: m.StreamerGeneralSettings }),
  ),
);
const StreamerConfiguration = lazy(() =>
  import('@/components/streamers/config/streamer-configuration').then((m) => ({
    default: m.StreamerConfiguration,
  })),
);
const StreamerFiltersTab = lazy(() =>
  import('@/components/streamers/edit/streamer-filters-tab').then((m) => ({
    default: m.StreamerFiltersTab,
  })),
);
const StreamerSaveFab = lazy(() =>
  import('@/components/streamers/edit/streamer-save-fab').then((m) => ({
    default: m.StreamerSaveFab,
  })),
);

import { type Download } from '@/store/downloads';
import { useDownloadStore } from '@/store/downloads';
import { useShallow } from 'zustand/react/shallow';

import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { Settings, Filter as FilterIcon, Activity } from 'lucide-react';
import {
  getStreamer,
  listFilters,
  listSessions,
  listPlatformConfigs,
  listTemplates,
  listEngines,
} from '@/server/functions';

import { getPlatformFromUrl } from '@/lib/utils';
import { Form } from '@/components/ui/form';
import {
  Card,
  CardHeader,
  CardTitle,
  CardDescription,
  CardContent,
} from '@/components/ui/card';

// Modular Components
import { StreamerHeader } from '@/components/streamers/edit/streamer-header';
import { ActiveDownloadCard } from '@/components/streamers/edit/active-download-card';
import { RecentSessionsList } from '@/components/streamers/edit/recent-sessions-list';
import { EditStreamerSkeleton } from '@/components/streamers/edit/edit-streamer-skeleton';
import { useDownloadProgress } from '@/hooks/use-download-progress';
import { useEditStreamer } from '@/hooks/use-edit-streamer';

export const Route = createFileRoute('/_authed/_dashboard/streamers/$id/edit')({
  component: EditStreamerPage,
});

const containerVariants: any = {
  hidden: { opacity: 0 },
  visible: {
    opacity: 1,
    transition: {
      staggerChildren: 0.1,
    },
  },
};

const itemVariants: any = {
  hidden: { opacity: 0, y: 20 },
  visible: {
    opacity: 1,
    y: 0,
    transition: { type: 'spring', stiffness: 300, damping: 24 },
  },
};

const tabContentVariants: any = {
  hidden: { opacity: 0, x: -10 },
  visible: { opacity: 1, x: 0, transition: { duration: 0.2 } },
};

function EditStreamerPage() {
  const { id } = Route.useParams();

  // Subscribe to download progress updates for this specific streamer
  useDownloadProgress({ streamerId: id });

  const { data: streamer, isLoading: isStreamerLoading } = useQuery({
    queryKey: ['streamer', id],
    queryFn: () => getStreamer({ data: id }),
  });

  const { data: sessions, isLoading: isLoadingSessions } = useQuery({
    queryKey: ['sessions', { streamer_id: id }],
    queryFn: () => listSessions({ data: { streamer_id: id } }),
    enabled: !!id,
  });

  const { data: platforms, isLoading: isPlatformsLoading } = useQuery({
    queryKey: ['platform-configs'],
    queryFn: () => listPlatformConfigs(),
  });

  const { data: templates, isLoading: isTemplatesLoading } = useQuery({
    queryKey: ['templates'],
    queryFn: () => listTemplates(),
  });

  const { data: engines } = useQuery({
    queryKey: ['engines'],
    queryFn: () => listEngines(),
  });

  const { data: filters, isLoading: isFiltersLoading } = useQuery({
    queryKey: ['streamers', id, 'filters'],
    queryFn: () => listFilters({ data: id }),
  });

  const downloads = useDownloadStore(
    useShallow((state) => state.getDownloadsByStreamer(id)),
  );

  // Wait for all required data before rendering the form
  const isLoading =
    isStreamerLoading || isPlatformsLoading || isTemplatesLoading;

  if (isLoading || !streamer) {
    return <EditStreamerSkeleton />;
  }

  // Render the form component only when all data is ready
  return (
    <EditStreamerForm
      id={id}
      streamer={streamer}
      platforms={platforms || []}
      templates={templates || []}
      engines={engines}
      filters={filters || []}
      sessions={sessions}
      isLoadingSessions={isLoadingSessions}
      isFiltersLoading={isFiltersLoading}
      downloads={downloads}
    />
  );
}

// Inner form component - only mounts when all data is ready
function EditStreamerForm({
  id,
  streamer,
  platforms,
  templates,
  engines,
  filters,
  sessions,
  isLoadingSessions,
  isFiltersLoading,
  downloads,
}: {
  id: string;
  streamer: NonNullable<Awaited<ReturnType<typeof getStreamer>>>;
  platforms: Awaited<ReturnType<typeof listPlatformConfigs>>;
  templates: Awaited<ReturnType<typeof listTemplates>>;
  engines: Awaited<ReturnType<typeof listEngines>> | undefined;
  filters: Awaited<ReturnType<typeof listFilters>>;
  sessions: Awaited<ReturnType<typeof listSessions>> | undefined;
  isLoadingSessions: boolean;
  isFiltersLoading: boolean;
  downloads: Download[];
}) {
  const isRecording = useMemo(() => downloads.length > 0, [downloads.length]);
  const isLive = useMemo(() => streamer.state === 'LIVE', [streamer.state]);
  const platform = useMemo(
    () => getPlatformFromUrl(streamer.url),
    [streamer.url],
  );

  const {
    form,
    isAutofilling,
    handleAutofillName,
    onSubmit,
    onInvalid,
    isSaving,
    deleteFilter,
  } = useEditStreamer({
    id,
    streamer,
  });

  const platformNameHint = useMemo(
    () => platforms?.find((p) => p.id === streamer.platform_config_id)?.name,
    [platforms, streamer.platform_config_id],
  );

  return (
    <Form {...form}>
      <motion.div
        variants={containerVariants}
        initial="hidden"
        animate="visible"
        className="max-w-7xl mx-auto p-4 md:p-8 space-y-8"
      >
        <motion.div variants={itemVariants}>
          <StreamerHeader
            streamer={streamer}
            isRecording={isRecording}
            isLive={isLive}
            platform={platform}
          />
        </motion.div>

        <div className="grid grid-cols-1 lg:grid-cols-4 gap-8">
          <div className="lg:col-span-3 space-y-6">
            <Tabs defaultValue="general" className="w-full">
              <motion.div variants={itemVariants}>
                <TabsList className="grid w-full grid-cols-1 sm:grid-cols-3 h-auto p-1 bg-muted/30 border rounded-xl md:rounded-full md:inline-flex md:w-auto backdrop-blur-sm">
                  <TabsTrigger
                    value="general"
                    className="rounded-lg md:rounded-full px-4 md:px-6 py-2.5 data-[state=active]:bg-background data-[state=active]:text-foreground data-[state=active]:shadow-sm transition-all"
                  >
                    <Settings className="w-4 h-4 mr-2" />
                    <Trans>General</Trans>
                  </TabsTrigger>
                  <TabsTrigger
                    value="advanced"
                    className="rounded-lg md:rounded-full px-4 md:px-6 py-2.5 data-[state=active]:bg-background data-[state=active]:text-foreground data-[state=active]:shadow-sm transition-all"
                  >
                    <Activity className="w-4 h-4 mr-2" />
                    <Trans>Advanced</Trans>
                  </TabsTrigger>
                  <TabsTrigger
                    value="filters"
                    className="rounded-lg md:rounded-full px-4 md:px-6 py-2.5 data-[state=active]:bg-background data-[state=active]:text-foreground data-[state=active]:shadow-sm transition-all"
                  >
                    <FilterIcon className="w-4 h-4 mr-2" />
                    <Trans>Recording Filters</Trans>
                    {filters && filters.length > 0 && (
                      <span className="ml-2 bg-primary/10 text-primary text-[10px] font-bold px-1.5 py-0.5 rounded-full">
                        {filters.length}
                      </span>
                    )}
                  </TabsTrigger>
                </TabsList>
              </motion.div>

              <form
                id="streamer-edit-form"
                onSubmit={form.handleSubmit(onSubmit, onInvalid)}
                className="space-y-6"
              >
                <TabsContent
                  value="general"
                  className="mt-6 border-none ring-offset-background focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2"
                >
                  <motion.div
                    variants={tabContentVariants}
                    initial="hidden"
                    animate="visible"
                    key="general"
                  >
                    <Suspense
                      fallback={<Skeleton className="h-[400px] w-full" />}
                    >
                      <Card>
                        <CardHeader>
                          <CardTitle>
                            <Trans>General Configuration</Trans>
                          </CardTitle>
                          <CardDescription>
                            <Trans>Basic settings for the streamer.</Trans>
                          </CardDescription>
                        </CardHeader>
                        <CardContent>
                          <StreamerGeneralSettings
                            form={form}
                            platformConfigs={platforms || []}
                            templates={templates || []}
                            onAutofillName={handleAutofillName}
                            isAutofilling={isAutofilling}
                          />
                        </CardContent>
                      </Card>
                    </Suspense>
                  </motion.div>
                </TabsContent>

                <TabsContent
                  value="advanced"
                  className="mt-6 border-none ring-offset-background focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2"
                >
                  <motion.div
                    variants={tabContentVariants}
                    initial="hidden"
                    animate="visible"
                    key="advanced"
                  >
                    <Suspense
                      fallback={<Skeleton className="h-[600px] w-full" />}
                    >
                      <Card>
                        <CardHeader>
                          <CardTitle>
                            <Trans>Advanced Configuration</Trans>
                          </CardTitle>
                          <CardDescription>
                            <Trans>
                              Override global defaults for this streamer.
                            </Trans>
                          </CardDescription>
                        </CardHeader>
                        <CardContent>
                          <StreamerConfiguration
                            form={form}
                            engines={engines}
                            streamerId={id}
                            credentialPlatformNameHint={platformNameHint}
                          />
                        </CardContent>
                      </Card>
                    </Suspense>
                  </motion.div>
                </TabsContent>
              </form>

              <TabsContent
                value="filters"
                className="mt-6 space-y-4 border-none ring-offset-background focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2"
              >
                <motion.div
                  variants={tabContentVariants}
                  initial="hidden"
                  animate="visible"
                  key="filters"
                >
                  <StreamerFiltersTab
                    streamerId={id}
                    filters={filters || []}
                    isLoading={isFiltersLoading}
                    onDeleteFilter={deleteFilter}
                  />
                </motion.div>
              </TabsContent>
            </Tabs>
          </div>

          <div className="space-y-6">
            <motion.div variants={itemVariants}>
              <ActiveDownloadCard
                downloads={downloads}
                isRecording={isRecording}
              />
            </motion.div>

            <motion.div variants={itemVariants}>
              <RecentSessionsList
                sessions={sessions?.items || []}
                isLoading={isLoadingSessions}
              />
            </motion.div>
          </div>
        </div>

        <Suspense fallback={null}>
          <StreamerSaveFab isSaving={isSaving} formId="streamer-edit-form" />
        </Suspense>
      </motion.div>
    </Form>
  );
}
