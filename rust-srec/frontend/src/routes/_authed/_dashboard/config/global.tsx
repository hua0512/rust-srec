import { createFileRoute } from '@tanstack/react-router';
import { zodResolver } from '@hookform/resolvers/zod';
import { motion } from 'motion/react';
import { GlobalConfigFormSchema } from '@/api/schemas';
import { getGlobalConfig, updateGlobalConfig } from '@/server/functions';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Button } from '@/components/ui/button';
import { Form } from '@/components/ui/form';
import { toast } from 'sonner';
import { z } from 'zod';
import { t } from '@lingui/core/macro';
import { Skeleton } from '@/components/ui/skeleton';
import { Save } from 'lucide-react';
import { useForm, useFormContext } from 'react-hook-form';
import { useMemo, useCallback, lazy, Suspense } from 'react';

const FileConfigCard = lazy(() =>
  import('@/components/config/global/file-config-card').then((m) => ({
    default: m.FileConfigCard,
  })),
);
const ResourceLimitsCard = lazy(() =>
  import('@/components/config/global/resource-limits-card').then((m) => ({
    default: m.ResourceLimitsCard,
  })),
);
const ConcurrencyCard = lazy(() =>
  import('@/components/config/global/concurrency-card').then((m) => ({
    default: m.ConcurrencyCard,
  })),
);
const NetworkSystemCard = lazy(() =>
  import('@/components/config/global/network-system-card').then((m) => ({
    default: m.NetworkSystemCard,
  })),
);
const PipelineConfigCard = lazy(() =>
  import('@/components/config/global/pipeline-config-card').then((m) => ({
    default: m.PipelineConfigCard,
  })),
);

export const Route = createFileRoute('/_authed/_dashboard/config/global')({
  component: GlobalConfigPage,
});

function GlobalConfigPage() {
  const { data: config, isLoading } = useQuery({
    queryKey: ['config', 'global'],
    queryFn: () => getGlobalConfig(),
  });

  if (isLoading || !config) {
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

  return <GlobalConfigForm config={config} />;
}

function SaveButton({ isPending }: { isPending: boolean }) {
  const { formState } = useFormContext();

  if (!formState.isDirty) return null;

  return (
    <div className="fixed bottom-8 right-8 z-50 animate-in fade-in slide-in-from-bottom-4 duration-300">
      <Button
        type="submit"
        disabled={isPending}
        size="lg"
        className="shadow-2xl shadow-primary/40 hover:shadow-primary/50 transition-all hover:scale-105 active:scale-95 rounded-full px-8 h-14 bg-gradient-to-r from-primary to-primary/90 text-base font-semibold"
      >
        <Save className="w-5 h-5 mr-2" />
        {isPending ? t`Saving...` : t`Save Changes`}
      </Button>
    </div>
  );
}

const CardSkeleton = () => (
  <Skeleton className="h-[400px] rounded-xl border-border/40 bg-muted/60" />
);

function GlobalConfigForm({
  config,
}: {
  config: z.infer<typeof GlobalConfigFormSchema>;
}) {
  type GlobalConfigFormValues = z.infer<typeof GlobalConfigFormSchema>;
  const queryClient = useQueryClient();

  const defaultValues = useMemo(
    () => ({
      ...config,
      proxy_config: config.proxy_config ?? null,
      pipeline: config.pipeline ?? null,
      session_complete_pipeline: config.session_complete_pipeline ?? null,
      paired_segment_pipeline: config.paired_segment_pipeline ?? null,
    }),
    [config],
  );

  const form = useForm<GlobalConfigFormValues>({
    // Work around a react-hook-form resolver type incompatibility (often caused by
    // `exactOptionalPropertyTypes` + resolver type definitions).
    resolver: zodResolver(GlobalConfigFormSchema) as any,
    defaultValues,
    values: defaultValues,
    reValidateMode: 'onBlur',
  });

  const updateMutation = useMutation({
    mutationFn: (data: GlobalConfigFormValues) => updateGlobalConfig({ data }),
    onSuccess: () => {
      toast.success(t`Settings updated successfully`);
      queryClient.invalidateQueries({ queryKey: ['config', 'global'] });
    },
    onError: (error: any) => {
      toast.error(error.message || t`Failed to update settings`);
    },
  });

  const onSubmit = useCallback(
    (data: GlobalConfigFormValues) => {
      updateMutation.mutate(data);
    },
    [updateMutation],
  );

  return (
    <Form {...form}>
      <form onSubmit={form.handleSubmit(onSubmit)} className="space-y-8 pb-32">
        <motion.div
          className="grid gap-6 lg:gap-8 lg:grid-cols-2"
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          transition={{ duration: 0.3 }}
        >
          <Suspense fallback={<CardSkeleton />}>
            <motion.div
              initial={{ opacity: 0, y: 20 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ duration: 0.3, delay: 0 }}
            >
              <FileConfigCard control={form.control} />
            </motion.div>
          </Suspense>

          <Suspense fallback={<CardSkeleton />}>
            <motion.div
              initial={{ opacity: 0, y: 20 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ duration: 0.3, delay: 0.05 }}
            >
              <ResourceLimitsCard control={form.control} />
            </motion.div>
          </Suspense>

          <Suspense fallback={<CardSkeleton />}>
            <motion.div
              initial={{ opacity: 0, y: 20 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ duration: 0.3, delay: 0.1 }}
            >
              <ConcurrencyCard control={form.control} />
            </motion.div>
          </Suspense>

          <Suspense fallback={<CardSkeleton />}>
            <motion.div
              initial={{ opacity: 0, y: 20 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ duration: 0.3, delay: 0.15 }}
            >
              <NetworkSystemCard control={form.control} />
            </motion.div>
          </Suspense>

          <Suspense
            fallback={
              <Skeleton className="h-[500px] lg:col-span-2 rounded-xl border-border/40 bg-muted/60" />
            }
          >
            <motion.div
              initial={{ opacity: 0, y: 20 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ duration: 0.3, delay: 0.25 }}
              className="lg:col-span-2 min-w-0"
            >
              <PipelineConfigCard control={form.control} />
            </motion.div>
          </Suspense>
        </motion.div>

        <SaveButton isPending={updateMutation.isPending} />
      </form>
    </Form>
  );
}
