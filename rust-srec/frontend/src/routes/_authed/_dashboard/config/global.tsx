import { createFileRoute } from '@tanstack/react-router';
import { useForm } from 'react-hook-form';
import { zodResolver } from '@hookform/resolvers/zod';
import { motion } from 'motion/react';
import { GlobalConfigFormSchema } from '@/api/schemas';
import {
  getGlobalConfig,
  updateGlobalConfig,
  listEngines,
} from '@/server/functions';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Button } from '@/components/ui/button';
import { Form } from '@/components/ui/form';
import { toast } from 'sonner';
import { z } from 'zod';
import { t } from '@lingui/core/macro';
import { Skeleton } from '@/components/ui/skeleton';
import { Save } from 'lucide-react';
import { FileConfigCard } from '@/components/config/global/file-config-card';
import { ResourceLimitsCard } from '@/components/config/global/resource-limits-card';
import { ConcurrencyCard } from '@/components/config/global/concurrency-card';
import { NetworkSystemCard } from '@/components/config/global/network-system-card';
import { PipelineConfigCard } from '@/components/config/global/pipeline-config-card';

export const Route = createFileRoute('/_authed/_dashboard/config/global')({
  component: GlobalConfigPage,
});

function GlobalConfigPage() {
  const { data: config, isLoading } = useQuery({
    queryKey: ['config', 'global'],
    queryFn: () => getGlobalConfig(),
  });

  const { data: engines, isLoading: enginesLoading } = useQuery({
    queryKey: ['engines'],
    queryFn: () => listEngines(),
  });

  if (isLoading || enginesLoading || !config) {
    return (
      <div className="space-y-6 form-container">
        <div className="flex items-center justify-between">
          <Skeleton className="h-8 w-48" />
          <Skeleton className="h-10 w-32" />
        </div>
        <div className="grid gap-6 lg:gap-8 lg:grid-cols-2">
          <Skeleton className="h-[400px] rounded-xl border-border/40 bg-muted/60" />
          <Skeleton className="h-[400px] rounded-xl border-border/40 bg-muted/60" />
          <Skeleton className="h-[400px] rounded-xl border-border/40 bg-muted/60" />
          <Skeleton className="h-[400px] rounded-xl border-border/40 bg-muted/60" />
          <Skeleton className="h-[500px] lg:col-span-2 rounded-xl border-border/40 bg-muted/60" />
        </div>
      </div>
    );
  }

  return <GlobalConfigForm config={config} engines={engines} />;
}

function GlobalConfigForm({
  config,
  engines,
}: {
  config: z.infer<typeof GlobalConfigFormSchema>;
  engines: any;
}) {
  const queryClient = useQueryClient();

  const form = useForm<z.infer<typeof GlobalConfigFormSchema>>({
    resolver: zodResolver(GlobalConfigFormSchema),
    defaultValues: {
      ...config,
      proxy_config: config.proxy_config ?? null,
      pipeline: config.pipeline ?? null,
      session_complete_pipeline: config.session_complete_pipeline ?? null,
      paired_segment_pipeline: config.paired_segment_pipeline ?? null,
    },
    values: {
      ...config,
      proxy_config: config.proxy_config ?? null,
      pipeline: config.pipeline ?? null,
      session_complete_pipeline: config.session_complete_pipeline ?? null,
      paired_segment_pipeline: config.paired_segment_pipeline ?? null,
    },
  });

  const updateMutation = useMutation({
    mutationFn: (data: z.infer<typeof GlobalConfigFormSchema>) =>
      updateGlobalConfig({ data }),
    onSuccess: () => {
      toast.success(t`Settings updated successfully`);
      queryClient.invalidateQueries({ queryKey: ['config', 'global'] });
    },
    onError: (error: any) => {
      toast.error(error.message || t`Failed to update settings`);
    },
  });

  const onSubmit = (data: z.infer<typeof GlobalConfigFormSchema>) => {
    updateMutation.mutate(data);
  };

  return (
    <Form {...form}>
      <form onSubmit={form.handleSubmit(onSubmit)} className="space-y-8 pb-32">
        <motion.div
          className="grid gap-6 lg:gap-8 lg:grid-cols-2"
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          transition={{ duration: 0.3 }}
        >
          {/* File Configuration */}
          <motion.div
            initial={{ opacity: 0, y: 20 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ duration: 0.3, delay: 0 }}
          >
            <FileConfigCard control={form.control} />
          </motion.div>

          {/* Resource Limits */}
          <motion.div
            initial={{ opacity: 0, y: 20 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ duration: 0.3, delay: 0.05 }}
          >
            <ResourceLimitsCard control={form.control} />
          </motion.div>

          {/* Concurrency & Performance */}
          <motion.div
            initial={{ opacity: 0, y: 20 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ duration: 0.3, delay: 0.1 }}
          >
            <ConcurrencyCard
              control={form.control}
              engines={engines}
              enginesLoading={false}
            />
          </motion.div>

          {/* Network & System */}
          <motion.div
            initial={{ opacity: 0, y: 20 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ duration: 0.3, delay: 0.15 }}
          >
            <NetworkSystemCard control={form.control} />
          </motion.div>

          {/* Pipeline Configuration */}
          <motion.div
            initial={{ opacity: 0, y: 20 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ duration: 0.3, delay: 0.25 }}
            className="lg:col-span-2 min-w-0"
          >
            <PipelineConfigCard control={form.control} />
          </motion.div>
        </motion.div>

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
