import { memo } from 'react';
import { useFormState } from 'react-hook-form';
import { SettingsCard } from '../settings-card';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormMessage,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import { Separator } from '@/components/ui/separator';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import {
  AlertTriangle,
  ArrowDownToLine,
  ArrowUpFromLine,
  Cpu,
  Database,
  Gauge,
  Timer,
  Zap,
} from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { useQuery } from '@tanstack/react-query';
import { listEngines } from '@/server/functions';
import { InputWithUnit } from '@/components/ui/input-with-unit';
import {
  CONFIG_DESCRIPTION,
  CONFIG_INPUT,
  CONFIG_SELECT_TRIGGER,
  ConfigFieldLabel,
  ConfigSectionHeading,
  FieldInfo,
} from '@/components/config/shared/config-field';

export const ConcurrencyCard = memo(() => {
  const { i18n } = useLingui();
  const { data: enginesData, isLoading: enginesLoading } = useQuery({
    queryKey: ['engines'],
    queryFn: () => listEngines(),
  });

  // The pipeline timeouts are read when the worker pools are built, which currently only happens
  // at startup, so a save alone does not apply them. Warn once any of the three is edited.
  const { dirtyFields } = useFormState();
  const restartRequired = Boolean(
    dirtyFields?.pipeline_cpu_job_timeout_secs ||
    dirtyFields?.pipeline_io_job_timeout_secs ||
    dirtyFields?.pipeline_execute_timeout_secs,
  );

  const engines = enginesData || [];

  return (
    <SettingsCard
      title={<Trans>Execution</Trans>}
      description={
        <Trans>
          Concurrency limits, pipeline timeouts, and engine defaults.
        </Trans>
      }
      icon={Cpu}
      iconColor="text-green-500"
      iconBgColor="bg-green-500/10"
    >
      <div className="space-y-8">
        <section className="space-y-4">
          <ConfigSectionHeading icon={Gauge}>
            <Trans>Concurrency</Trans>
          </ConfigSectionHeading>

          <div className="space-y-6">
            <div className="grid grid-cols-1 gap-6 @sm:grid-cols-2 @3xl:grid-cols-4">
              <FormField
                name="max_concurrent_downloads"
                render={({ field }) => (
                  <FormItem className="space-y-2">
                    <ConfigFieldLabel>
                      <Trans>Max Downloads</Trans>
                      <FieldInfo
                        icon={<ArrowDownToLine className="h-4 w-4" />}
                        title={<Trans>Max Downloads</Trans>}
                      >
                        <Trans>
                          How many streamers may record at the same time.
                          Recordings past this limit wait in the queue until a
                          slot frees up.
                        </Trans>
                      </FieldInfo>
                    </ConfigFieldLabel>
                    <FormControl>
                      <Input
                        className={CONFIG_INPUT}
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
                name="max_concurrent_uploads"
                render={({ field }) => (
                  <FormItem className="space-y-2">
                    <ConfigFieldLabel>
                      <Trans>Max Uploads</Trans>
                      <FieldInfo
                        icon={<ArrowUpFromLine className="h-4 w-4" />}
                        title={<Trans>Max Uploads</Trans>}
                        theme="rose"
                      >
                        <Trans>
                          How many pipeline upload steps may run at the same
                          time.
                        </Trans>
                      </FieldInfo>
                    </ConfigFieldLabel>
                    <FormControl>
                      <Input
                        className={CONFIG_INPUT}
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
                name="max_concurrent_cpu_jobs"
                render={({ field }) => (
                  <FormItem className="space-y-2">
                    <ConfigFieldLabel>
                      <Trans>Max CPU Jobs</Trans>
                      <FieldInfo
                        icon={<Cpu className="h-4 w-4" />}
                        title={<Trans>Max CPU Jobs</Trans>}
                      >
                        <Trans>
                          How many CPU-bound pipeline steps, such as transcodes,
                          may run at the same time. Raising this past your core
                          count slows every job down.
                        </Trans>
                      </FieldInfo>
                    </ConfigFieldLabel>
                    <FormControl>
                      <Input
                        className={CONFIG_INPUT}
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
                name="max_concurrent_io_jobs"
                render={({ field }) => (
                  <FormItem className="space-y-2">
                    <ConfigFieldLabel>
                      <Trans>Max IO Jobs</Trans>
                      <FieldInfo
                        icon={<Database className="h-4 w-4" />}
                        title={<Trans>Max IO Jobs</Trans>}
                        theme="violet"
                      >
                        <Trans>
                          How many disk-bound pipeline steps, such as moves and
                          copies, may run at the same time.
                        </Trans>
                      </FieldInfo>
                    </ConfigFieldLabel>
                    <FormControl>
                      <Input
                        className={CONFIG_INPUT}
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

            <FormField
              name="queue_freshness_threshold_ms"
              render={({ field }) => (
                <FormItem className="space-y-2 @md:max-w-xs">
                  <ConfigFieldLabel>
                    <Trans>Queued Refresh Threshold</Trans>
                    <FieldInfo
                      icon={<Timer className="h-4 w-4" />}
                      title={<Trans>Queued Refresh Threshold</Trans>}
                      theme="amber"
                    >
                      <Trans>
                        When a recording has been waiting in the concurrency
                        queue longer than this, rust-srec re-checks the streamer
                        to refresh stream URLs and headers before starting.
                        Below this threshold the URLs captured at the original
                        live event are reused. Default 60 seconds. Set to 0 to
                        refresh on every queue wait.
                      </Trans>
                    </FieldInfo>
                  </ConfigFieldLabel>
                  <FormControl>
                    <InputWithUnit
                      unitType="duration"
                      value={(field.value ?? 0) / 1000}
                      onChange={(val) =>
                        field.onChange(val !== null ? val * 1000 : 0)
                      }
                      placeholder="60"
                    />
                  </FormControl>
                  <FormMessage />
                </FormItem>
              )}
            />
          </div>
        </section>

        <Separator />

        <section className="space-y-4">
          <ConfigSectionHeading icon={Timer} accent="sky">
            <span className="inline-flex items-center gap-2">
              <Trans>Pipeline Timeouts</Trans>
              <FieldInfo
                icon={<Timer className="h-4 w-4" />}
                title={<Trans>Job Timeouts</Trans>}
              >
                <Trans>
                  How long a pipeline step may run before it is cancelled. These
                  are read when the worker pools are built, so a change needs a
                  restart.
                </Trans>
              </FieldInfo>
            </span>
          </ConfigSectionHeading>

          <div className="space-y-4">
            <div className="grid grid-cols-1 gap-6 @md:grid-cols-2 @2xl:grid-cols-3">
              <FormField
                name="pipeline_cpu_job_timeout_secs"
                render={({ field }) => (
                  <FormItem className="space-y-2">
                    <ConfigFieldLabel accent="sky">
                      <Trans>CPU Job</Trans>
                      <FieldInfo
                        icon={<Cpu className="h-4 w-4" />}
                        title={<Trans>CPU Job Timeout</Trans>}
                      >
                        <Trans>
                          Timeout before cancelling CPU-bound processors.
                        </Trans>
                      </FieldInfo>
                    </ConfigFieldLabel>
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
                name="pipeline_io_job_timeout_secs"
                render={({ field }) => (
                  <FormItem className="space-y-2">
                    <ConfigFieldLabel accent="sky">
                      <Trans>IO Job</Trans>
                      <FieldInfo
                        icon={<Database className="h-4 w-4" />}
                        title={<Trans>IO Job Timeout</Trans>}
                        theme="violet"
                      >
                        <Trans>
                          Timeout before cancelling IO-bound processors.
                        </Trans>
                      </FieldInfo>
                    </ConfigFieldLabel>
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
                name="pipeline_execute_timeout_secs"
                render={({ field }) => (
                  <FormItem className="space-y-2">
                    {/* "Execute", not "Execution": this is the `execute` processor's own
                        timeout, and the card is already titled Execution. */}
                    <ConfigFieldLabel accent="sky">
                      <Trans>Execute</Trans>
                      <FieldInfo
                        icon={<Zap className="h-4 w-4" />}
                        title={<Trans>Execute Timeout</Trans>}
                        theme="orange"
                      >
                        <Trans>
                          Timeout before cancelling `execute` processor
                          commands.
                        </Trans>
                      </FieldInfo>
                    </ConfigFieldLabel>
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

            {/* Stated in the page rather than behind the heading's tooltip: a user who has just
                edited a timeout needs to know saving is not enough without hovering to find out. */}
            {restartRequired && (
              <div
                role="status"
                className="flex items-start gap-2 rounded-xl border border-orange-500/30 bg-orange-500/10 px-3 py-2.5 text-xs font-medium text-orange-700 dark:text-orange-300"
              >
                <AlertTriangle className="mt-px h-3.5 w-3.5 shrink-0" />
                <Trans>
                  Restart rust-srec after saving for the new timeouts to take
                  effect.
                </Trans>
              </div>
            )}
          </div>
        </section>

        <Separator />

        <section className="space-y-4">
          <ConfigSectionHeading icon={Zap} accent="emerald">
            <Trans>Engine & Hardware</Trans>
          </ConfigSectionHeading>

          <div className="grid grid-cols-1 gap-6 @md:grid-cols-2">
            <FormField
              name="default_download_engine"
              render={({ field }) => (
                <FormItem className="space-y-2">
                  <ConfigFieldLabel accent="emerald">
                    <Trans>Default Engine</Trans>
                  </ConfigFieldLabel>
                  <Select
                    onValueChange={field.onChange}
                    value={field.value}
                    disabled={enginesLoading}
                  >
                    <FormControl>
                      <SelectTrigger className={CONFIG_SELECT_TRIGGER}>
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
                  <FormDescription className={CONFIG_DESCRIPTION}>
                    <Trans>
                      Engine used for downloads when not specified by
                      platform/streamer.
                    </Trans>
                  </FormDescription>
                  <FormMessage />
                </FormItem>
              )}
            />

            <FormField
              name="gpu_health_probe_interval_secs"
              render={({ field }) => (
                <FormItem className="space-y-2">
                  <ConfigFieldLabel accent="emerald">
                    <Trans>GPU Health Probe Interval</Trans>
                    <FieldInfo
                      icon={<Cpu className="h-4 w-4" />}
                      title={<Trans>GPU Health Probe Interval</Trans>}
                      theme="violet"
                    >
                      <Trans>
                        How often rust-srec runs nvidia-smi to detect when the
                        container loses GPU access (a known NVIDIA Container
                        Toolkit issue on cgroup v2 hosts). Only active when
                        nvidia-smi is available; otherwise the GPU row is not
                        registered. Default 30 seconds. Changes apply on the
                        next probe, with no restart required. Values below 30
                        seconds are discouraged.
                      </Trans>
                    </FieldInfo>
                  </ConfigFieldLabel>
                  <FormControl>
                    <InputWithUnit
                      unitType="duration"
                      min={1}
                      {...field}
                      onChange={(val) => field.onChange(val ?? 30)}
                      placeholder="30"
                    />
                  </FormControl>
                  <FormDescription className={CONFIG_DESCRIPTION}>
                    <Trans>
                      How often nvidia-smi is polled to confirm the GPU is still
                      reachable.
                    </Trans>
                  </FormDescription>
                  <FormMessage />
                </FormItem>
              )}
            />
          </div>
        </section>
      </div>
    </SettingsCard>
  );
});

ConcurrencyCard.displayName = 'ConcurrencyCard';
