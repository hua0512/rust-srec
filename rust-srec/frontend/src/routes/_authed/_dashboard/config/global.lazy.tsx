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
import {
  useMemo,
  useCallback,
  lazy,
  Suspense,
  ComponentType,
  ReactNode,
} from 'react';
import { SettingsCardSkeleton } from '@/components/config/settings-card-skeleton';
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
const GlobalDanmuStatisticsCard = lazy(() =>
  import('@/components/config/global/danmu-statistics-card').then((m) => ({
    default: m.GlobalDanmuStatisticsCard,
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
 *
 * `skeletonRows` describes each card's field rows per section at this page's two-column width,
 * where the cards' `@md:grid-cols-2` grids put two fields on one row. Keep it in step with the
 * card when fields are added, or the placeholder stops matching what replaces it.
 */
const SECTIONS: {
  Card: ComponentType;
  wide?: boolean;
  skeletonRows: number[];
  skeletonBody?: ReactNode;
}[] = [
  { Card: FileConfigCard, skeletonRows: [5] },
  { Card: ResourceLimitsCard, skeletonRows: [3] },
  { Card: ConcurrencyCard, skeletonRows: [3, 2, 1] },
  { Card: NetworkSystemCard, skeletonRows: [2, 1, 2] },
  // One switch row, a 2x2 numeric grid, then the ignored-words textarea.
  { Card: GlobalDanmuStatisticsCard, skeletonRows: [1, 2, 2, 1] },
  {
    Card: PipelineConfigCard,
    wide: true,
    skeletonRows: [],
    // Not a field list: a three-tab bar over an alert and the DAG editor, which reserves
    // `min-h-[500px]` of its own.
    skeletonBody: (
      <div className="space-y-6">
        <Skeleton className="h-11 w-full rounded-xl" />
        <Skeleton className="h-20 w-full rounded-lg" />
        <Skeleton className="h-[500px] w-full rounded-lg" />
      </div>
    ),
  },
];

export const Route = createLazyFileRoute('/_authed/_dashboard/config/global')({
  component: GlobalConfigPage,
});

/**
 * The column span lives on the wrapper in both the loading and loaded paths, so the skeleton
 * renders at the same width as the card it stands in for.
 */
function CardSkeleton({
  skeletonRows,
  skeletonBody,
}: (typeof SECTIONS)[number]) {
  return (
    <SettingsCardSkeleton sections={skeletonRows}>
      {skeletonBody}
    </SettingsCardSkeleton>
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
        {SECTIONS.map((section, i) => (
          <div
            key={i}
            className={cn('min-w-0', section.wide && 'lg:col-span-2')}
          >
            <CardSkeleton {...section} />
          </div>
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
          {SECTIONS.map((section, i) => (
            <motion.div
              key={i}
              variants={itemVariants}
              className={cn('min-w-0', section.wide && 'lg:col-span-2')}
            >
              <Suspense fallback={<CardSkeleton {...section} />}>
                <section.Card />
              </Suspense>
            </motion.div>
          ))}
        </motion.div>

        <SaveFab isSaving={isPending} formId="global-config-form" />
      </form>
    </Form>
  );
}
