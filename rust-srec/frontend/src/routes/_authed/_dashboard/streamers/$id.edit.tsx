import { createFileRoute, useNavigate } from '@tanstack/react-router';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { toast } from 'sonner';
import { t } from '@lingui/core/macro';
import { Trans } from '@lingui/react/macro';
import { Skeleton } from '@/components/ui/skeleton';
import { motion } from 'motion/react';

import { StreamerGeneralSettings } from '@/components/streamers/config/streamer-general-settings';
import { StreamerConfiguration } from '@/components/streamers/config/streamer-configuration';
import {
  UpdateStreamerSchema,
  StreamerFormSchema,
  StreamerFormValues,
} from '@/api/schemas';
import { z } from 'zod';
import { useDownloadProgress } from '@/hooks/use-download-progress';
import { useDownloadStore } from '@/store/downloads';
import { useShallow } from 'zustand/react/shallow';

import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { Settings, Filter as FilterIcon, Activity } from 'lucide-react';
import {
  getStreamer,
  updateStreamer,
  listFilters,
  deleteFilter,
  listSessions,
  listPlatformConfigs,
  listTemplates,
  listEngines,
} from '@/server/functions';

import { getPlatformFromUrl } from '@/lib/utils';
import { useForm } from 'react-hook-form';
import { zodResolver } from '@hookform/resolvers/zod';
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
import { StreamerFiltersTab } from '@/components/streamers/edit/streamer-filters-tab';
import { StreamerSaveFab } from '@/components/streamers/edit/streamer-save-fab';

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
  const queryClient = useQueryClient();

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
    return (
      <div className="max-w-4xl mx-auto space-y-8 p-6 animate-pulse">
        <div className="flex items-center gap-4">
          <Skeleton className="h-10 w-10 rounded-full" />
          <div className="space-y-2">
            <Skeleton className="h-8 w-48" />
            <Skeleton className="h-4 w-24" />
          </div>
        </div>
        <div className="space-y-6">
          <Skeleton className="h-10 w-full rounded-xl" />
          <Skeleton className="h-[400px] w-full rounded-xl" />
        </div>
      </div>
    );
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
      queryClient={queryClient}
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
  queryClient,
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
  downloads: ReturnType<
    ReturnType<typeof useDownloadStore>['getDownloadsByStreamer']
  >;
  queryClient: ReturnType<typeof useQueryClient>;
}) {
  const navigate = useNavigate();
  const isRecording = downloads.length > 0;
  const isLive = streamer.state === 'LIVE';
  const platform = getPlatformFromUrl(streamer.url);

  const updateMutation = useMutation({
    mutationFn: (data: z.infer<typeof UpdateStreamerSchema>) =>
      updateStreamer({ data: { id, data } }),
    onSuccess: () => {
      toast.success(t`Streamer updated successfully`);
      queryClient.invalidateQueries({ queryKey: ['streamers'] });
      queryClient.invalidateQueries({ queryKey: ['streamer', id] });
      navigate({ to: '/streamers' });
    },
    onError: (error: any) => {
      toast.error(error.message || t`Failed to update streamer`);
    },
  });

  const onSubmit = (data: StreamerFormValues) => {
    const payload: z.infer<typeof UpdateStreamerSchema> = {
      ...data,
      platform_config_id:
        data.platform_config_id === 'none' || data.platform_config_id === ''
          ? undefined
          : data.platform_config_id,
      template_id: data.template_id === 'none' ? undefined : data.template_id,
      streamer_specific_config: data.streamer_specific_config ?? undefined,
    };
    updateMutation.mutate(payload);
  };

  const onInvalid = (errors: any) => {
    console.error('Form validation errors:', errors);
    toast.error(t`Please fix validation errors`);
  };

  const deleteFilterMutation = useMutation({
    mutationFn: (filterId: string) =>
      deleteFilter({ data: { streamerId: id, filterId } }),
    onSuccess: () => {
      toast.success(t`Filter deleted successfully`);
      queryClient.invalidateQueries({ queryKey: ['streamers', id, 'filters'] });
    },
    onError: (error: any) => {
      toast.error(error.message || t`Failed to delete filter`);
    },
  });

  // Parse the specific config
  const specificConfig =
    typeof streamer.streamer_specific_config === 'string'
      ? JSON.parse(streamer.streamer_specific_config)
      : (streamer.streamer_specific_config ?? {});

  // Initialize form with correct values from the start
  const form = useForm<StreamerFormValues>({
    resolver: zodResolver(StreamerFormSchema) as any,
    defaultValues: {
      name: streamer.name,
      url: streamer.url,
      enabled: streamer.enabled,
      priority: streamer.priority,
      platform_config_id: streamer.platform_config_id || '',
      template_id: streamer.template_id ?? undefined,
      streamer_specific_config: specificConfig,
    },
  });

  return (
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

            <Form {...form}>
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
                        />
                      </CardContent>
                    </Card>
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
                        <StreamerConfiguration form={form} engines={engines} />
                      </CardContent>
                    </Card>
                  </motion.div>
                </TabsContent>
              </form>
            </Form>

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
                  onDeleteFilter={(filterId) =>
                    deleteFilterMutation.mutate(filterId)
                  }
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

      <StreamerSaveFab
        isDirty={form.formState.isDirty}
        isSaving={updateMutation.isPending}
        formId="streamer-edit-form"
      />
    </motion.div>
  );
}
