import { createFileRoute } from '@tanstack/react-router';
import { useForm } from 'react-hook-form';
import { zodResolver } from '@hookform/resolvers/zod';
import { GlobalConfigSchema } from '../../../../api/schemas';
import { getGlobalConfig, updateGlobalConfig, listEngines } from '@/server/functions';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Button } from '../../../../components/ui/button';
import { Form } from '../../../../components/ui/form';
import { toast } from 'sonner';
import { z } from 'zod';
import { t } from '@lingui/core/macro';
import { Skeleton } from '../../../../components/ui/skeleton';
import { useEffect } from 'react';
import { Save } from 'lucide-react';
import { FileConfigCard } from '../../../../components/config/global/file-config-card';
import { ResourceLimitsCard } from '../../../../components/config/global/resource-limits-card';
import { ConcurrencyCard } from '../../../../components/config/global/concurrency-card';
import { NetworkSystemCard } from '../../../../components/config/global/network-system-card';

export const Route = createFileRoute('/_authed/_dashboard/config/global')({
  component: GlobalConfigPage,
});

function GlobalConfigPage() {
  const queryClient = useQueryClient();

  const { data: config, isLoading } = useQuery({
    queryKey: ['config', 'global'],
    queryFn: () => getGlobalConfig(),
  });

  const { data: engines, isLoading: enginesLoading } = useQuery({
    queryKey: ['engines'],
    queryFn: () => listEngines(),
  });

  const form = useForm<z.infer<typeof GlobalConfigSchema>>({
    resolver: zodResolver(GlobalConfigSchema),
    defaultValues: config ? {
      ...config,
      proxy_config: config.proxy_config ?? '',
    } : {
      output_folder: '',
      output_filename_template: '',
      output_file_format: 'flv',
      min_segment_size_bytes: 0,
      max_download_duration_secs: 0,
      max_part_size_bytes: 0,
      record_danmu: false,
      max_concurrent_downloads: 0,
      max_concurrent_uploads: 0,
      max_concurrent_cpu_jobs: 0,
      max_concurrent_io_jobs: 0,
      streamer_check_delay_ms: 0,
      proxy_config: '',
      offline_check_delay_ms: 0,
      offline_check_count: 0,
      default_download_engine: 'default-mesio',
      job_history_retention_days: 30,
    },
    values: config ? {
      ...config,
      proxy_config: config.proxy_config ?? '',
    } : undefined,
  });

  useEffect(() => {
    if (config) {
      form.reset({
        ...config,
        proxy_config: config.proxy_config ?? '',
      });
    }
  }, [config, form]);

  const updateMutation = useMutation({
    mutationFn: (data: z.infer<typeof GlobalConfigSchema>) => updateGlobalConfig({ data }),
    onSuccess: () => {
      toast.success(t`Settings updated successfully`);
      queryClient.invalidateQueries({ queryKey: ['config', 'global'] });
    },
    onError: (error: any) => {
      toast.error(error.message || t`Failed to update settings`);
    },
  });

  const onSubmit = (data: z.infer<typeof GlobalConfigSchema>) => {
    updateMutation.mutate(data);
  };

  if (isLoading) {
    return (
      <div className="space-y-6 form-container">
        <div className="flex items-center justify-between">
          <Skeleton className="h-8 w-48" />
          <Skeleton className="h-10 w-32" />
        </div>
        <div className="grid gap-6 md:grid-cols-2">
          <Skeleton className="h-[300px]" />
          <Skeleton className="h-[300px]" />
          <Skeleton className="h-[300px]" />
          <Skeleton className="h-[300px]" />
        </div>
      </div>
    )
  }

  return (
    <Form {...form}>
      <form onSubmit={form.handleSubmit(onSubmit)} className="space-y-8 pb-32">
        <div className="grid gap-8 md:grid-cols-2">
          {/* File Configuration */}
          <FileConfigCard control={form.control} />

          {/* Resource Limits */}
          <ResourceLimitsCard control={form.control} />

          {/* Concurrency & Performance */}
          <ConcurrencyCard
            control={form.control}
            engines={engines}
            enginesLoading={enginesLoading}
          />

          {/* Network & System */}
          <NetworkSystemCard control={form.control} />
        </div>

        {form.formState.isDirty && (
          <div className="fixed bottom-8 right-8 z-50 animate-in fade-in slide-in-from-bottom-4 duration-300">
            <Button
              type="submit"
              disabled={updateMutation.isPending}
              size="lg"
              className="shadow-2xl shadow-primary/40 hover:shadow-primary/50 transition-all hover:scale-105 active:scale-95 rounded-full px-8 h-14 bg-gradient-to-r from-primary to-primary/90 text-base font-semibold"
            >
              <Save className="w-5 h-5 mr-2" />
              {updateMutation.isPending ? t`Saving...` : t`Save Changes`}
            </Button>
          </div>
        )}
      </form>
    </Form>
  );
}
