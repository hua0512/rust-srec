import { useForm } from 'react-hook-form';
import { zodResolver } from '@hookform/resolvers/zod';
import { z } from 'zod';
import { Trans } from '@lingui/react/macro';
import { motion } from 'motion/react';
import { Form } from '@/components/ui/form';
import { Button } from '@/components/ui/button';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import {
  Settings,
  Cookie,
  Shield,
  Code,
  Filter,
  Save,
  Loader2,
  Workflow,
  ArrowLeft,
} from 'lucide-react';
import { PlatformConfigSchema } from '@/api/schemas';
import { GeneralTab } from './tabs/general-tab';
import { StreamSelectionTab } from './tabs/stream-selection-tab';
import { AuthTab } from './tabs/auth-tab';
import { AdvancedTab } from './tabs/advanced-tab';
import { ProxyTab } from './tabs/proxy-tab';
import { PipelineTab } from './tabs/pipeline-tab';
import {
  getPlatformIcon,
  getPlatformColor,
} from '@/components/pipeline/constants';
import { cn } from '@/lib/utils';
import { Link } from '@tanstack/react-router';


const EditPlatformSchema = PlatformConfigSchema.partial();
export type EditPlatformFormValues = z.infer<typeof EditPlatformSchema>;

interface PlatformEditorProps {
  platform: z.infer<typeof PlatformConfigSchema>;
  onSubmit: (data: EditPlatformFormValues) => void;
  isUpdating: boolean;
}

export function PlatformEditor({
  platform,
  onSubmit,
  isUpdating,
}: PlatformEditorProps) {
  const form = useForm<EditPlatformFormValues>({
    resolver: zodResolver(EditPlatformSchema),
    defaultValues: {
      fetch_delay_ms: platform.fetch_delay_ms,
      download_delay_ms: platform.download_delay_ms,
      record_danmu: platform.record_danmu,
      cookies: platform.cookies,
      platform_specific_config: platform.platform_specific_config,
      proxy_config: platform.proxy_config,
      output_folder: platform.output_folder,
      output_filename_template: platform.output_filename_template,
      download_engine: platform.download_engine,
      stream_selection_config: platform.stream_selection_config,
      output_file_format: platform.output_file_format,
      min_segment_size_bytes: platform.min_segment_size_bytes,
      max_download_duration_secs: platform.max_download_duration_secs,
      max_part_size_bytes: platform.max_part_size_bytes,
      download_retry_policy: platform.download_retry_policy,
      event_hooks: platform.event_hooks,
      pipeline: platform.pipeline,
    },
  });

  const Icon = getPlatformIcon(platform.name);
  const colorClass = getPlatformColor(platform.name);

  return (
    <Form {...form}>
      <form
        onSubmit={form.handleSubmit(onSubmit)}
        className="min-h-screen pb-20"
      >
        <motion.div
          initial={{ opacity: 0, y: 10 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ duration: 0.3 }}
          className="max-w-8xl space-y-8 px-4 sm:px-6 lg:px-8 py-8"
        >
          {/* Header Section */}
          <div className="flex flex-col gap-6">

            <div className="flex items-start justify-between">
              <div className="flex items-center gap-4">
                <div
                  className={cn(
                    'p-3 rounded-2xl ring-1 ring-inset ring-black/5 dark:ring-white/10 shadow-sm',
                    colorClass,
                  )}
                >
                  <Icon className="w-8 h-8" />
                </div>
                <div className="space-y-1">
                  <h1 className="text-3xl font-bold tracking-tight">
                    {platform.name}
                  </h1>
                  <p className="text-muted-foreground text-sm flex items-center gap-2">
                    <span className="inline-flex items-center gap-1.5 px-2 py-0.5 rounded-full bg-accent/50 text-xs font-medium border border-border/50">
                      ID: <span className="font-mono">{platform.id}</span>
                    </span>
                  </p>
                </div>
              </div>
              <div className="flex gap-2">
                <Button
                  type="button"
                  variant="outline"
                  onClick={() => window.history.back()}
                  className="gap-2"
                >
                  <ArrowLeft className="w-4 h-4" />
                  <Trans>Back</Trans>
                </Button>
                <Button
                  type="submit"
                  disabled={isUpdating}
                  className={cn(
                    'min-w-[140px] gap-2 shadow-lg shadow-primary/20 transition-all hover:scale-105 active:scale-95',
                    isUpdating && 'opacity-80',
                  )}
                >
                  {isUpdating ? (
                    <Loader2 className="w-4 h-4 animate-spin" />
                  ) : (
                    <Save className="w-4 h-4" />
                  )}
                  {isUpdating ? (
                    <Trans>Saving...</Trans>
                  ) : (
                    <Trans>Save Changes</Trans>
                  )}
                </Button>
              </div>
            </div>
          </div>

          {/* Tabs Section */}
          <Tabs defaultValue="general" className="w-full space-y-6">
            <div className="bg-muted/50 p-1 rounded-2xl backdrop-blur-sm border border-border/50 inline-flex flex-wrap gap-1">
              <TabsList className="h-auto bg-transparent p-0 gap-1 flex-wrap justify-start">
                <TabsTrigger
                  value="general"
                  className="gap-2 px-4 py-2.5 h-10 data-[state=active]:bg-background data-[state=active]:text-foreground data-[state=active]:shadow-sm rounded-xl transition-all"
                >
                  <Settings className="w-4 h-4" />
                  <span className="font-medium">
                    <Trans>General</Trans>
                  </span>
                </TabsTrigger>
                <TabsTrigger
                  value="stream-selection"
                  className="gap-2 px-4 py-2.5 h-10 data-[state=active]:bg-background data-[state=active]:text-foreground data-[state=active]:shadow-sm rounded-xl transition-all"
                >
                  <Filter className="w-4 h-4" />
                  <span className="font-medium">
                    <Trans>Selection</Trans>
                  </span>
                </TabsTrigger>
                <TabsTrigger
                  value="auth"
                  className="gap-2 px-4 py-2.5 h-10 data-[state=active]:bg-background data-[state=active]:text-foreground data-[state=active]:shadow-sm rounded-xl transition-all"
                >
                  <Cookie className="w-4 h-4" />
                  <span className="font-medium">
                    <Trans>Auth</Trans>
                  </span>
                </TabsTrigger>
                <TabsTrigger
                  value="pipeline"
                  className="gap-2 px-4 py-2.5 h-10 data-[state=active]:bg-background data-[state=active]:text-foreground data-[state=active]:shadow-sm rounded-xl transition-all"
                >
                  <Workflow className="w-4 h-4" />
                  <span className="font-medium">
                    <Trans>Pipeline</Trans>
                  </span>
                </TabsTrigger>
                <TabsTrigger
                  value="proxy"
                  className="gap-2 px-4 py-2.5 h-10 data-[state=active]:bg-background data-[state=active]:text-foreground data-[state=active]:shadow-sm rounded-xl transition-all"
                >
                  <Shield className="w-4 h-4" />
                  <span className="font-medium">
                    <Trans>Proxy</Trans>
                  </span>
                </TabsTrigger>
                <TabsTrigger
                  value="advanced"
                  className="gap-2 px-4 py-2.5 h-10 data-[state=active]:bg-background data-[state=active]:text-foreground data-[state=active]:shadow-sm rounded-xl transition-all"
                >
                  <Code className="w-4 h-4" />
                  <span className="font-medium">
                    <Trans>Advanced</Trans>
                  </span>
                </TabsTrigger>
              </TabsList>
            </div>

            <div className="space-y-6">
              <TabsContent
                value="general"
                className="mt-0 focus-visible:outline-none animate-in fade-in-50 slide-in-from-bottom-1 duration-300"
              >
                <GeneralTab form={form} />
              </TabsContent>

              <TabsContent
                value="stream-selection"
                className="mt-0 focus-visible:outline-none animate-in fade-in-50 slide-in-from-bottom-1 duration-300"
              >
                <StreamSelectionTab form={form} />
              </TabsContent>

              <TabsContent
                value="auth"
                className="mt-0 focus-visible:outline-none animate-in fade-in-50 slide-in-from-bottom-1 duration-300"
              >
                <AuthTab form={form} />
              </TabsContent>

              <TabsContent
                value="pipeline"
                className="mt-0 focus-visible:outline-none animate-in fade-in-50 slide-in-from-bottom-1 duration-300"
              >
                <PipelineTab form={form} />
              </TabsContent>

              <TabsContent
                value="proxy"
                className="mt-0 focus-visible:outline-none animate-in fade-in-50 slide-in-from-bottom-1 duration-300"
              >
                <ProxyTab form={form} />
              </TabsContent>

              <TabsContent
                value="advanced"
                className="mt-0 focus-visible:outline-none animate-in fade-in-50 slide-in-from-bottom-1 duration-300"
              >
                <AdvancedTab form={form} />
              </TabsContent>
            </div>
          </Tabs>
        </motion.div>
      </form>
    </Form>
  );
}
