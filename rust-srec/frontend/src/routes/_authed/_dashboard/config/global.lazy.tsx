import { createLazyFileRoute, useBlocker } from '@tanstack/react-router';
import { zodResolver } from '@hookform/resolvers/zod';
import { motion } from 'motion/react';
import { GlobalConfigFormSchema } from '@/api/schemas';
import { getGlobalConfig, updateGlobalConfig } from '@/server/functions';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Form } from '@/components/ui/form';
import { toast } from 'sonner';
import { z } from 'zod';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { Skeleton } from '@/components/ui/skeleton';
import { useForm } from 'react-hook-form';
import { useMemo, useCallback, lazy, Suspense, ComponentType } from 'react';
import { containerVariants, itemVariants } from '@/lib/animation';
import { cn } from '@/lib/utils';
import { SaveFab } from '@/components/shared/save-fab';

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

/**
 * The settings cards, in display order.
 *
 * Listed as data rather than five near-identical `Suspense`/`motion` blocks, which is what the
 * page previously carried — each differing only by an animation delay.
 */
const SECTIONS: { Card: ComponentType; wide?: boolean }[] = [
  { Card: FileConfigCard },
  { Card: ResourceLimitsCard },
  { Card: ConcurrencyCard },
  { Card: NetworkSystemCard },
  { Card: PipelineConfigCard, wide: true },
];

export const Route = createLazyFileRoute('/_authed/_dashboard/config/global')({
  component: GlobalConfigPage,
});

function CardSkeleton({ wide }: { wide?: boolean }) {
  return (
    <Skeleton
      className={cn('h-96 rounded-xl', wide && 'h-[28rem] lg:col-span-2')}
    />
  );
}

function GlobalConfigPage() {
  const { data: config, isLoading } = useQuery({
    queryKey: ['config', 'global'],
    queryFn: () => getGlobalConfig(),
  });

  if (isLoading || !config) {
    return (
      <div className="grid gap-6 pb-32 lg:gap-8 lg:grid-cols-2">
        {SECTIONS.map(({ wide }, i) => (
          <CardSkeleton key={i} wide={wide} />
        ))}
      </div>
    );
  }

  return <GlobalConfigForm config={config} />;
}

function GlobalConfigForm({
  config,
}: {
  config: z.infer<typeof GlobalConfigFormSchema>;
}) {
  type GlobalConfigFormValues = z.infer<typeof GlobalConfigFormSchema>;
  const queryClient = useQueryClient();
  const { i18n } = useLingui();

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
      toast.success(i18n._(msg`Settings updated successfully`));
      void queryClient.invalidateQueries({ queryKey: ['config', 'global'] });
    },
    onError: (error: any) => {
      toast.error(error.message || i18n._(msg`Failed to update settings`));
    },
  });

  const isDirty = form.formState.isDirty;
  const isPending = updateMutation.isPending;

  // Settings only take effect once saved, so leaving with edits in place loses them silently.
  useBlocker({
    shouldBlockFn: () => {
      if (!isDirty || isPending) return false;
      return !window.confirm(
        i18n._(msg`You have unsaved changes. Leave without saving?`),
      );
    },
    enableBeforeUnload: () => isDirty && !isPending,
  });

  const onSubmit = useCallback(
    (data: GlobalConfigFormValues) => {
      updateMutation.mutate(data);
    },
    [updateMutation],
  );

  return (
    <Form {...form}>
      <form
        id="global-config-form"
        onSubmit={form.handleSubmit(onSubmit)}
        className="pb-32"
      >
        <motion.div
          className="grid gap-6 lg:gap-8 lg:grid-cols-2"
          variants={containerVariants}
          initial="hidden"
          animate="visible"
        >
          {SECTIONS.map(({ Card, wide }, i) => (
            <motion.div
              key={i}
              variants={itemVariants}
              className={cn('min-w-0', wide && 'lg:col-span-2')}
            >
              <Suspense fallback={<CardSkeleton wide={wide} />}>
                <Card />
              </Suspense>
            </motion.div>
          ))}
        </motion.div>

        <SaveFab
          isSaving={isPending}
          formId="global-config-form"
          alwaysVisible
          disabledWhenPristine
        />
      </form>
    </Form>
  );
}
