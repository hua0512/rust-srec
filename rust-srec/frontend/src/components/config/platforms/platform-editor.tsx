import { useEffect } from 'react';
import { useForm } from 'react-hook-form';
import { zodResolver } from '@hookform/resolvers/zod';
import { z } from 'zod';
import { Trans } from '@lingui/react/macro';
import { motion } from 'motion/react';
import { Form } from '@/components/ui/form';
import { Button } from '@/components/ui/button';
import { useQuery } from '@tanstack/react-query';

import { Settings, Save, Loader2, ArrowLeft } from 'lucide-react';
import { PlatformConfigFormSchema, PlatformConfigSchema } from '@/api/schemas';
import { GeneralTab } from './tabs/general-tab';
import { PlatformSpecificTab } from './tabs/platform-specific-tab';
import {
  getPlatformIcon,
  getPlatformColor,
} from '@/components/pipeline/constants';
import { cn } from '@/lib/utils';
import { SharedConfigEditor } from '../shared-config-editor';
import { listEngines } from '@/server/functions';

const EditPlatformSchema = PlatformConfigFormSchema.partial();
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
  const { data: engines = [] } = useQuery({
    queryKey: ['engines'],
    queryFn: () => listEngines(),
  });

  const form = useForm({
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
      session_complete_pipeline: platform.session_complete_pipeline,
      paired_segment_pipeline: platform.paired_segment_pipeline,
    },
  });
  const { reset } = form;

  // Reset form when platform data changes (e.g. after QR login re-fetch)
  useEffect(() => {
    reset({
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
      session_complete_pipeline: platform.session_complete_pipeline,
      paired_segment_pipeline: platform.paired_segment_pipeline,
    });
  }, [platform, reset]);

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
          className="max-w-7xl mx-auto space-y-6 sm:space-y-8 px-4 sm:px-6 lg:px-8 py-4 sm:py-8"
        >
          {/* Header Section */}
          <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-6">
            <div className="flex items-center gap-4">
              <div
                className={cn(
                  'p-2.5 sm:p-3 rounded-2xl ring-1 ring-inset ring-black/5 dark:ring-white/10 shadow-sm shrink-0',
                  colorClass,
                )}
              >
                <Icon className="w-6 h-6 sm:w-8 sm:h-8" />
              </div>
              <div className="min-w-0 space-y-1">
                <h1 className="text-2xl sm:text-3xl font-bold tracking-tight truncate">
                  {platform.name}
                </h1>
                <p className="text-muted-foreground text-sm flex items-center gap-2">
                  <span className="inline-flex items-center gap-1.5 px-2 py-0.5 rounded-full bg-accent/50 text-xs font-medium border border-border/50">
                    <span className="opacity-60">ID:</span>{' '}
                    <span className="font-mono truncate max-w-[120px] sm:max-w-none">
                      {platform.id}
                    </span>
                  </span>
                </p>
              </div>
            </div>
            <div className="flex flex-col-reverse sm:flex-row gap-3 sm:gap-2">
              <Button
                type="button"
                variant="outline"
                onClick={() => window.history.back()}
                className="gap-2 h-11 sm:h-10 order-2 sm:order-1"
              >
                <ArrowLeft className="w-4 h-4" />
                <Trans>Back</Trans>
              </Button>
              <Button
                type="submit"
                disabled={isUpdating}
                className={cn(
                  'min-w-full sm:min-w-[140px] gap-2 shadow-lg shadow-primary/20 transition-all hover:scale-[1.02] active:scale-95 h-11 sm:h-10 order-1 sm:order-2',
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

          {/* Tabs Section */}
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
              hooks: 'event_hooks',
              pipeline: 'pipeline',
              sessionCompletePipeline: 'session_complete_pipeline',
              pairedSegmentPipeline: 'paired_segment_pipeline',
            }}
            credentialScope={{ type: 'platform', id: platform.id }}
            credentialPlatformNameHint={platform.name}
            engines={engines}
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
                value: 'specific',
                label: (
                  <span className="font-medium">
                    <Trans>Specific</Trans>
                  </span>
                ),
                icon: Settings,
                content: (
                  <PlatformSpecificTab
                    form={form}
                    platformName={platform.name}
                  />
                ),
              },
            ]}
            defaultTab="general"
            proxyMode="object"
            configMode="object"
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
      </form>
    </Form>
  );
}
