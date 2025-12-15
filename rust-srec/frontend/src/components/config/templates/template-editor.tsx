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
  Server,
  Workflow,
  FileBox,
  ArrowLeft,
} from 'lucide-react';
import { TemplateSchema, UpdateTemplateRequestSchema } from '@/api/schemas';
import { GeneralTab } from './tabs/general-tab';
import { StreamSelectionTab } from '../platforms/tabs/stream-selection-tab';
import { AuthTab } from '../platforms/tabs/auth-tab';
import { AdvancedTab } from '../platforms/tabs/advanced-tab';
import { ProxyTab } from '../platforms/tabs/proxy-tab';
import { EngineOverridesTab } from './tabs/engine-overrides-tab';
import { PlatformOverridesTab } from './tabs/platform-overrides-tab';
import { PipelineTab } from '../platforms/tabs/pipeline-tab';
import { cn } from '@/lib/utils';

export type TemplateFormValues = z.infer<typeof UpdateTemplateRequestSchema>;

interface TemplateEditorProps {
  template?: z.infer<typeof TemplateSchema>;
  onSubmit: (data: TemplateFormValues) => void;
  isSubmitting: boolean;
  mode: 'create' | 'edit';
}

export function TemplateEditor({
  template,
  onSubmit,
  isSubmitting,
  mode,
}: TemplateEditorProps) {
  const form = useForm<TemplateFormValues>({
    resolver: zodResolver(UpdateTemplateRequestSchema),
    defaultValues: template
      ? {
        name: template.name,
        output_folder: template.output_folder,
        output_filename_template: template.output_filename_template,
        output_file_format: template.output_file_format,
        min_segment_size_bytes: template.min_segment_size_bytes,
        max_download_duration_secs: template.max_download_duration_secs,
        max_part_size_bytes: template.max_part_size_bytes,
        record_danmu: template.record_danmu,
        cookies: template.cookies,
        platform_overrides: template.platform_overrides,
        download_retry_policy: template.download_retry_policy,
        danmu_sampling_config: template.danmu_sampling_config,
        download_engine: template.download_engine,
        engines_override: template.engines_override,
        proxy_config: template.proxy_config,
        event_hooks: template.event_hooks,
        stream_selection_config: template.stream_selection_config,
        pipeline: template.pipeline,
      }
      : {
        name: '',
        output_folder: null,
        record_danmu: null,
        pipeline: null,
      },
  });

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
                <div className="p-3 rounded-2xl ring-1 ring-inset ring-black/5 dark:ring-white/10 shadow-sm bg-indigo-500/10 text-indigo-600 dark:text-indigo-400">
                  <FileBox className="w-8 h-8" />
                </div>
                <div className="space-y-1">
                  <h1 className="text-3xl font-bold tracking-tight">
                    {mode === 'create' ? (
                      <Trans>Create Template</Trans>
                    ) : (
                      template?.name
                    )}
                  </h1>
                  <p className="text-muted-foreground text-sm flex items-center gap-2">
                    {mode === 'edit' && (
                      <span className="inline-flex items-center gap-1.5 px-2 py-0.5 rounded-full bg-accent/50 text-xs font-medium border border-border/50">
                        ID: <span className="font-mono">{template?.id}</span>
                      </span>
                    )}
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
                  disabled={isSubmitting}
                  className={cn(
                    'min-w-[140px] gap-2 shadow-lg shadow-primary/20 transition-all hover:scale-105 active:scale-95',
                    isSubmitting && 'opacity-80',
                  )}
                >
                  {isSubmitting ? (
                    <Loader2 className="w-4 h-4 animate-spin" />
                  ) : (
                    <Save className="w-4 h-4" />
                  )}
                  {isSubmitting ? (
                    <Trans>Saving...</Trans>
                  ) : mode === 'create' ? (
                    <Trans>Create</Trans>
                  ) : (
                    <Trans>Save Changes</Trans>
                  )}
                </Button>
              </div>
            </div>
          </div>

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
                  value="engine-overrides"
                  className="gap-2 px-4 py-2.5 h-10 data-[state=active]:bg-background data-[state=active]:text-foreground data-[state=active]:shadow-sm rounded-xl transition-all"
                >
                  <Server className="w-4 h-4" />
                  <span className="font-medium">
                    <Trans>Engines</Trans>
                  </span>
                </TabsTrigger>
                <TabsTrigger
                  value="platform-overrides"
                  className="gap-2 px-4 py-2.5 h-10 data-[state=active]:bg-background data-[state=active]:text-foreground data-[state=active]:shadow-sm rounded-xl transition-all"
                >
                  <Settings className="w-4 h-4" />
                  <span className="font-medium">
                    <Trans>Platforms</Trans>
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
                  value="pipeline"
                  className="gap-2 px-4 py-2.5 h-10 data-[state=active]:bg-background data-[state=active]:text-foreground data-[state=active]:shadow-sm rounded-xl transition-all"
                >
                  <Workflow className="w-4 h-4" />
                  <span className="font-medium">
                    <Trans>Pipeline</Trans>
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
                value="engine-overrides"
                className="mt-0 focus-visible:outline-none animate-in fade-in-50 slide-in-from-bottom-1 duration-300"
              >
                <EngineOverridesTab form={form} />
              </TabsContent>

              <TabsContent
                value="platform-overrides"
                className="mt-0 focus-visible:outline-none animate-in fade-in-50 slide-in-from-bottom-1 duration-300"
              >
                <PlatformOverridesTab form={form} />
              </TabsContent>

              <TabsContent
                value="proxy"
                className="mt-0 focus-visible:outline-none animate-in fade-in-50 slide-in-from-bottom-1 duration-300"
              >
                <ProxyTab form={form} />
              </TabsContent>

              <TabsContent
                value="pipeline"
                className="mt-0 focus-visible:outline-none animate-in fade-in-50 slide-in-from-bottom-1 duration-300"
              >
                <PipelineTab form={form} />
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

        {/* Floating Save Button - Only show if dirty and not already shown in header (mobile optimization) */}
        <div className="fixed bottom-6 right-6 z-50 md:hidden">
          <Button
            type="submit"
            disabled={isSubmitting}
            size="lg"
            className="shadow-xl rounded-full h-12 w-12 p-0"
          >
            {isSubmitting ? (
              <Loader2 className="w-5 h-5 animate-spin" />
            ) : (
              <Save className="w-5 h-5" />
            )}
          </Button>
        </div>
      </form>
    </Form>
  );
}
