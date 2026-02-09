import { memo } from 'react';
import { Control, useFormState } from 'react-hook-form';
import { SettingsCard } from '../settings-card';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import { Separator } from '@/components/ui/separator';
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from '@/components/ui/tooltip';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import {
  AlertTriangle,
  CircleHelp,
  Cpu,
  Database,
  Timer,
  Zap,
} from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { useQuery } from '@tanstack/react-query';
import { listEngines } from '@/server/functions';
import { InputWithUnit } from '@/components/ui/input-with-unit';
import { StatusInfoTooltip } from '@/components/shared/status-info-tooltip';

export interface ConcurrencyCardProps {
  control: Control<any>;
}

export const ConcurrencyCard = memo(({ control }: ConcurrencyCardProps) => {
  const { i18n } = useLingui();
  const { data: enginesData, isLoading: enginesLoading } = useQuery({
    queryKey: ['engines'],
    queryFn: () => listEngines(),
  });

  // These particular settings only take effect when the backend process
  // recreates the worker pools (currently startup-only), so surface a
  // restart-required warning only when the user changes them.
  const { dirtyFields } = useFormState({ control });
  const restartRequired = Boolean(
    dirtyFields?.pipeline_cpu_job_timeout_secs ||
    dirtyFields?.pipeline_io_job_timeout_secs ||
    dirtyFields?.pipeline_execute_timeout_secs,
  );

  const engines = enginesData || [];

  return (
    <SettingsCard
      title={<Trans>Concurrency & Performance</Trans>}
      description={<Trans>Job limits and engine settings.</Trans>}
      icon={Cpu}
      iconColor="text-green-500"
      iconBgColor="bg-green-500/10"
    >
      <div className="space-y-6">
        <div className="grid grid-cols-2 gap-6">
          <FormField
            control={control}
            name="max_concurrent_downloads"
            render={({ field }) => (
              <FormItem>
                <FormLabel>
                  <Trans>Max Downloads</Trans>
                </FormLabel>
                <FormControl>
                  <Input
                    type="number"
                    {...field}
                    onChange={(e) => field.onChange(Number(e.target.value))}
                  />
                </FormControl>
                <FormMessage />
              </FormItem>
            )}
          />
          <FormField
            control={control}
            name="max_concurrent_uploads"
            render={({ field }) => (
              <FormItem>
                <FormLabel>
                  <Trans>Max Uploads</Trans>
                </FormLabel>
                <FormControl>
                  <Input
                    type="number"
                    {...field}
                    onChange={(e) => field.onChange(Number(e.target.value))}
                  />
                </FormControl>
                <FormMessage />
              </FormItem>
            )}
          />
          <FormField
            control={control}
            name="max_concurrent_cpu_jobs"
            render={({ field }) => (
              <FormItem>
                <FormLabel>
                  <Trans>Max CPU Jobs</Trans>
                </FormLabel>
                <FormControl>
                  <Input
                    type="number"
                    {...field}
                    onChange={(e) => field.onChange(Number(e.target.value))}
                  />
                </FormControl>
                <FormMessage />
              </FormItem>
            )}
          />
          <FormField
            control={control}
            name="max_concurrent_io_jobs"
            render={({ field }) => (
              <FormItem>
                <FormLabel>
                  <Trans>Max IO Jobs</Trans>
                </FormLabel>
                <FormControl>
                  <Input
                    type="number"
                    {...field}
                    onChange={(e) => field.onChange(Number(e.target.value))}
                  />
                </FormControl>
                <FormMessage />
              </FormItem>
            )}
          />
        </div>

        <Separator />

        <div className="space-y-4">
          <div className="flex items-center gap-2">
            <Timer className="h-4 w-4 text-sky-500" />
            <h3 className="text-sm font-semibold">
              <Trans>Pipeline Job Timeouts</Trans>
            </h3>
            <Tooltip>
              <TooltipTrigger asChild>
                {restartRequired ? (
                  <AlertTriangle className="h-4 w-4 text-orange-500 cursor-help" />
                ) : (
                  <CircleHelp className="h-4 w-4 text-muted-foreground/40 cursor-help" />
                )}
              </TooltipTrigger>
              <TooltipContent className="p-0 border-border/50 shadow-xl bg-background/95 backdrop-blur-md overflow-hidden">
                <StatusInfoTooltip
                  icon={
                    restartRequired ? (
                      <AlertTriangle className="w-4 h-4" />
                    ) : (
                      <Timer className="w-4 h-4" />
                    )
                  }
                  title={
                    restartRequired ? (
                      <Trans>Restart Required</Trans>
                    ) : (
                      <Trans>Job Timeouts</Trans>
                    )
                  }
                  theme={restartRequired ? 'orange' : 'blue'}
                >
                  <p className="text-xs leading-relaxed text-muted-foreground">
                    <Trans>
                      These timeouts are applied when the pipeline worker pools
                      start. Changes require a restart to take effect.
                    </Trans>
                  </p>
                </StatusInfoTooltip>
              </TooltipContent>
            </Tooltip>
          </div>

          <div className="grid grid-cols-1 md:grid-cols-3 gap-6">
            <FormField
              control={control}
              name="pipeline_cpu_job_timeout_secs"
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="flex items-center gap-1.5">
                    <Cpu className="h-3.5 w-3.5 text-blue-500/80" />
                    <Trans>CPU Job</Trans>
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <CircleHelp className="h-3.5 w-3.5 text-muted-foreground/40 cursor-help hover:text-muted-foreground transition-colors" />
                      </TooltipTrigger>
                      <TooltipContent className="p-0 border-border/50 shadow-xl bg-background/95 backdrop-blur-md overflow-hidden">
                        <StatusInfoTooltip
                          icon={<Cpu className="w-4 h-4" />}
                          title={<Trans>CPU Job Timeout</Trans>}
                          theme="blue"
                        >
                          <p className="text-xs leading-relaxed text-muted-foreground">
                            <Trans>
                              Timeout before cancelling CPU-bound processors.
                            </Trans>
                          </p>
                        </StatusInfoTooltip>
                      </TooltipContent>
                    </Tooltip>
                  </FormLabel>
                  <FormControl>
                    <InputWithUnit
                      unitType="duration"
                      min={1}
                      {...field}
                      onChange={(val) => field.onChange(val ?? 0)}
                    />
                  </FormControl>
                  <FormMessage />
                </FormItem>
              )}
            />

            <FormField
              control={control}
              name="pipeline_io_job_timeout_secs"
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="flex items-center gap-1.5">
                    <Database className="h-3.5 w-3.5 text-purple-500/80" />
                    <Trans>IO Job</Trans>
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <CircleHelp className="h-3.5 w-3.5 text-muted-foreground/40 cursor-help hover:text-muted-foreground transition-colors" />
                      </TooltipTrigger>
                      <TooltipContent className="p-0 border-border/50 shadow-xl bg-background/95 backdrop-blur-md overflow-hidden">
                        <StatusInfoTooltip
                          icon={<Database className="w-4 h-4" />}
                          title={<Trans>IO Job Timeout</Trans>}
                          theme="violet"
                        >
                          <p className="text-xs leading-relaxed text-muted-foreground">
                            <Trans>
                              Timeout before cancelling IO-bound processors.
                            </Trans>
                          </p>
                        </StatusInfoTooltip>
                      </TooltipContent>
                    </Tooltip>
                  </FormLabel>
                  <FormControl>
                    <InputWithUnit
                      unitType="duration"
                      min={1}
                      {...field}
                      onChange={(val) => field.onChange(val ?? 0)}
                    />
                  </FormControl>
                  <FormMessage />
                </FormItem>
              )}
            />

            <FormField
              control={control}
              name="pipeline_execute_timeout_secs"
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="flex items-center gap-1.5">
                    <Zap className="h-3.5 w-3.5 text-orange-500/80" />
                    <Trans>Execution</Trans>
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <CircleHelp className="h-3.5 w-3.5 text-muted-foreground/40 cursor-help hover:text-muted-foreground transition-colors" />
                      </TooltipTrigger>
                      <TooltipContent className="p-0 border-border/50 shadow-xl bg-background/95 backdrop-blur-md overflow-hidden">
                        <StatusInfoTooltip
                          icon={<Zap className="w-4 h-4" />}
                          title={<Trans>Execute Timeout</Trans>}
                          theme="orange"
                        >
                          <p className="text-xs leading-relaxed text-muted-foreground">
                            <Trans>
                              Timeout before cancelling `execute` processor
                              commands.
                            </Trans>
                          </p>
                        </StatusInfoTooltip>
                      </TooltipContent>
                    </Tooltip>
                  </FormLabel>
                  <FormControl>
                    <InputWithUnit
                      unitType="duration"
                      min={1}
                      {...field}
                      onChange={(val) => field.onChange(val ?? 0)}
                    />
                  </FormControl>
                  <FormMessage />
                </FormItem>
              )}
            />
          </div>
        </div>

        <Separator />
        <FormField
          control={control}
          name="default_download_engine"
          render={({ field }) => (
            <FormItem>
              <FormLabel>
                <Trans>Default Engine</Trans>
              </FormLabel>
              <Select
                onValueChange={field.onChange}
                value={field.value}
                disabled={enginesLoading}
              >
                <FormControl>
                  <SelectTrigger>
                    <SelectValue
                      placeholder={i18n._(msg`Select a default engine`)}
                    />
                  </SelectTrigger>
                </FormControl>
                <SelectContent>
                  {engines?.map((engine) => (
                    <SelectItem key={engine.id} value={engine.name}>
                      {engine.name} ({engine.engine_type})
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <FormDescription>
                <Trans>
                  Engine used for downloads when not specified by
                  platform/streamer.
                </Trans>
              </FormDescription>
              <FormMessage />
            </FormItem>
          )}
        />
      </div>
    </SettingsCard>
  );
});

ConcurrencyCard.displayName = 'ConcurrencyCard';
