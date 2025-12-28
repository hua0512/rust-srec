import { useForm } from 'react-hook-form';
import { zodResolver } from '@hookform/resolvers/zod';
import { z } from 'zod';
import { Trans } from '@lingui/react/macro';
import { motion } from 'motion/react';
import { Form } from '@/components/ui/form';
import { Button } from '@/components/ui/button';
import {
  Settings,
  Save,
  Loader2,
  Server,
  FileBox,
  ArrowLeft,
} from 'lucide-react';
import { TemplateSchema, UpdateTemplateRequestSchema } from '@/api/schemas';
import { GeneralTab } from './tabs/general-tab';
import { EngineOverridesTab } from './tabs/engine-overrides-tab';
import { PlatformOverridesTab } from './tabs/platform-overrides-tab';
import { cn } from '@/lib/utils';
import { SharedConfigEditor } from '../shared-config-editor';

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
  console.log('TemplateEditor render template:', template);

  const form = useForm({
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
          session_complete_pipeline: template.session_complete_pipeline,
          paired_segment_pipeline: template.paired_segment_pipeline,
        }
      : {
          name: '',
          output_folder: null,
          output_filename_template: null,
          output_file_format: null,
          min_segment_size_bytes: null,
          max_download_duration_secs: null,
          max_part_size_bytes: null,
          record_danmu: null,
          cookies: null,
          platform_overrides: null,
          download_retry_policy: null,
          danmu_sampling_config: null,
          download_engine: null,
          engines_override: null,
          proxy_config: null,
          event_hooks: null,
          stream_selection_config: null,
          pipeline: null,
          session_complete_pipeline: null,
          paired_segment_pipeline: null,
        },
  });

  const platformOverrides = form.watch('platform_overrides');
  const platformOverrideKeys =
    platformOverrides && typeof platformOverrides === 'object'
      ? Object.keys(platformOverrides as Record<string, unknown>)
      : [];
  const credentialPlatformNameHint =
    platformOverrideKeys.length === 1
      ? platformOverrideKeys[0]
      : platformOverrideKeys.includes('bilibili')
        ? 'bilibili'
        : undefined;

  return (
    <Form {...form}>
      <form
        onSubmit={form.handleSubmit((data) => {
          console.log('Submitting template form data:', data);
          onSubmit(data);
        })}
        className="min-h-screen pb-20"
      >
        <motion.div
          initial={{ opacity: 0, y: 10 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ duration: 0.3 }}
          className="max-w-8xl space-y-6 sm:space-y-8 px-3 sm:px-6 lg:px-8 py-4 sm:py-8"
        >
          {/* Header Section */}
          <div className="flex flex-col gap-6">
            <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-4">
              <div className="flex items-center gap-4">
                <div className="p-2 sm:p-3 rounded-2xl ring-1 ring-inset ring-black/5 dark:ring-white/10 shadow-sm bg-indigo-500/10 text-indigo-600 dark:text-indigo-400 shrink-0">
                  <FileBox className="w-6 h-6 sm:w-8 sm:h-8" />
                </div>
                <div className="space-y-1 min-w-0">
                  <h1 className="text-xl sm:text-3xl font-bold tracking-tight truncate">
                    {mode === 'create' ? (
                      <Trans>Create Template</Trans>
                    ) : (
                      template?.name
                    )}
                  </h1>
                  {mode === 'edit' && (
                    <p className="text-muted-foreground text-xs flex items-center gap-2">
                      <span className="inline-flex items-center gap-1.5 px-2 py-0.5 rounded-full bg-accent/50 text-xs font-medium border border-border/50">
                        ID:{' '}
                        <span className="font-mono truncate max-w-[100px] sm:max-w-none">
                          {template?.id}
                        </span>
                      </span>
                    </p>
                  )}
                </div>
              </div>
              <div className="flex gap-2 w-full sm:w-auto">
                <Button
                  type="button"
                  variant="outline"
                  onClick={() => window.history.back()}
                  className="gap-2 shrink-0 h-9 sm:h-10"
                >
                  <ArrowLeft className="w-4 h-4" />
                  <span className="hidden sm:inline">
                    <Trans>Back</Trans>
                  </span>
                  <span className="sm:hidden text-xs">
                    <Trans>Back</Trans>
                  </span>
                </Button>
                <Button
                  type="submit"
                  disabled={isSubmitting}
                  className={cn(
                    'flex-1 sm:min-w-[140px] gap-2 h-9 sm:h-10 shadow-lg shadow-primary/20 transition-all hover:scale-105 active:scale-95 text-xs sm:text-sm',
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
          <SharedConfigEditor
            form={form}
            paths={{
              streamSelection: 'stream_selection_config',
              cookies: 'cookies',
              proxy: 'proxy_config',
              retryPolicy: 'download_retry_policy',
              output: '',
              limits: '',
              danmu: '',
              danmuSampling: 'danmu_sampling_config',
              hooks: 'event_hooks',
              pipeline: 'pipeline',
              sessionCompletePipeline: 'session_complete_pipeline',
              pairedSegmentPipeline: 'paired_segment_pipeline',
            }}
            credentialScope={
              template ? { type: 'template', id: template.id } : undefined
            }
            credentialPlatformNameHint={credentialPlatformNameHint}
            proxyMode="object"
            configMode="object"
            extraTabs={[
              {
                value: 'general',
                label: (
                  <span className="font-medium">
                    <Trans>General</Trans>
                  </span>
                ),
                icon: Settings,
                content: <GeneralTab form={form} />,
              },
              {
                value: 'engine-overrides',
                label: (
                  <span className="font-medium">
                    <Trans>Engines</Trans>
                  </span>
                ),
                icon: Server,
                content: <EngineOverridesTab form={form} />,
              },
              {
                value: 'platform-overrides',
                label: (
                  <span className="font-medium">
                    <Trans>Platforms</Trans>
                  </span>
                ),
                icon: Settings,
                content: <PlatformOverridesTab form={form} />,
              },
            ]}
            defaultTab="general"
            availableTabs={[
              'filters',
              'output',
              'network',
              'proxy',
              'danmu',
              'pipeline',
              'hooks',
            ]}
          />
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
