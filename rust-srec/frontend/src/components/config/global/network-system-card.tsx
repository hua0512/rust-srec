import { memo } from 'react';
import { Control } from 'react-hook-form';
import { SettingsCard } from '../settings-card';
import {
  FormControl,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import { InputWithUnit } from '@/components/ui/input-with-unit';
import { Separator } from '@/components/ui/separator';
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from '@/components/ui/tooltip';
import {
  Activity,
  CircleHelp,
  Clock,
  Database,
  History,
  Info,
  Layers,
  Network,
  Timer,
} from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { ProxyConfigSettings } from '../shared/proxy-settings-card';
import { StatusInfoTooltip } from '@/components/shared/status-info-tooltip';

interface NetworkSystemCardProps {
  control: Control<any>;
}

export const NetworkSystemCard = memo(({ control }: NetworkSystemCardProps) => {
  return (
    <SettingsCard
      title={<Trans>Network & System</Trans>}
      description={<Trans>Delays, proxy, and retention policies.</Trans>}
      icon={Network}
      iconColor="text-purple-500"
      iconBgColor="bg-purple-500/10"
    >
      <div className="space-y-8">
        {/* Monitoring Section */}
        <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
          <FormField
            control={control}
            name="streamer_check_delay_ms"
            render={({ field }) => (
              <FormItem>
                <FormLabel className="flex items-center gap-1.5">
                  <Layers className="h-3.5 w-3.5 text-blue-500/80" />
                  <Trans>Streamer Check</Trans>
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <Info className="h-3.5 w-3.5 text-muted-foreground/40 cursor-help hover:text-muted-foreground transition-colors" />
                    </TooltipTrigger>
                    <TooltipContent className="p-0 border-border/50 shadow-xl bg-background/95 backdrop-blur-md overflow-hidden">
                      <StatusInfoTooltip
                        icon={<Activity className="w-4 h-4" />}
                        title={<Trans>Streamer Check</Trans>}
                        theme="blue"
                      >
                        <p className="text-xs leading-relaxed text-muted-foreground">
                          <Trans>
                            Interval between checks to see if a streamer is
                            currently live. Slower intervals reduce API usage
                            but might delay recording starts.
                          </Trans>
                        </p>
                      </StatusInfoTooltip>
                    </TooltipContent>
                  </Tooltip>
                </FormLabel>
                <FormControl>
                  <InputWithUnit
                    unitType="duration"
                    value={(field.value ?? 0) / 1000}
                    onChange={(val) =>
                      field.onChange(val !== null ? val * 1000 : 0)
                    }
                    placeholder="0"
                  />
                </FormControl>
                <FormMessage />
              </FormItem>
            )}
          />
          <FormField
            control={control}
            name="offline_check_delay_ms"
            render={({ field }) => (
              <FormItem>
                <FormLabel className="flex items-center gap-1.5">
                  <Timer className="h-3.5 w-3.5 text-orange-500/80" />
                  <Trans>Offline Check</Trans>
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <CircleHelp className="h-3.5 w-3.5 text-muted-foreground/40 cursor-help hover:text-muted-foreground transition-colors" />
                    </TooltipTrigger>
                    <TooltipContent className="p-0 border-border/50 shadow-xl bg-background/95 backdrop-blur-md overflow-hidden">
                      <StatusInfoTooltip
                        icon={<Timer className="w-4 h-4" />}
                        title={<Trans>Offline Check</Trans>}
                        theme="orange"
                      >
                        <p className="text-xs leading-relaxed text-muted-foreground">
                          <Trans>
                            Interval between checks when a streamer is live to
                            detect when the stream ends.
                          </Trans>
                        </p>
                      </StatusInfoTooltip>
                    </TooltipContent>
                  </Tooltip>
                </FormLabel>
                <FormControl>
                  <InputWithUnit
                    unitType="duration"
                    value={(field.value ?? 0) / 1000}
                    onChange={(val) =>
                      field.onChange(val !== null ? val * 1000 : 0)
                    }
                    placeholder="0"
                  />
                </FormControl>
                <FormMessage />
              </FormItem>
            )}
          />
          <FormField
            control={control}
            name="offline_check_count"
            render={({ field }) => (
              <FormItem>
                <FormLabel className="flex items-center gap-1.5">
                  <Database className="h-3.5 w-3.5 text-slate-500/80" />
                  <Trans>Offline Detection Count</Trans>
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <CircleHelp className="h-3.5 w-3.5 text-muted-foreground/40 cursor-help hover:text-muted-foreground transition-colors" />
                    </TooltipTrigger>
                    <TooltipContent className="p-0 border-border/50 shadow-xl bg-background/95 backdrop-blur-md overflow-hidden">
                      <StatusInfoTooltip
                        icon={<Database className="w-4 h-4" />}
                        title={<Trans>Offline Detection</Trans>}
                        theme="slate"
                      >
                        <p className="text-xs leading-relaxed text-muted-foreground">
                          <Trans>
                            Number of consecutive failed checks required to
                            definitively confirm a streamer has gone offline.
                            Higher values prevent "fake" offline detection.
                          </Trans>
                        </p>
                      </StatusInfoTooltip>
                    </TooltipContent>
                  </Tooltip>
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

        <Separator className="bg-white/5" />

        {/* Persistence Section */}
        <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
          <FormField
            control={control}
            name="job_history_retention_days"
            render={({ field }) => (
              <FormItem>
                <FormLabel className="flex items-center gap-1.5">
                  <History className="h-3.5 w-3.5 text-violet-500/80" />
                  <Trans>Retention Period</Trans>
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <CircleHelp className="h-3.5 w-3.5 text-muted-foreground/40 cursor-help hover:text-muted-foreground transition-colors" />
                    </TooltipTrigger>
                    <TooltipContent className="p-0 border-border/50 shadow-xl bg-background/95 backdrop-blur-md overflow-hidden">
                      <StatusInfoTooltip
                        icon={<History className="w-4 h-4" />}
                        title={<Trans>History Retention</Trans>}
                        theme="violet"
                      >
                        <p className="text-xs leading-relaxed text-muted-foreground">
                          <Trans>
                            Number of days to keep the history of completed,
                            failed, or interrupted jobs in the database.
                          </Trans>
                        </p>
                      </StatusInfoTooltip>
                    </TooltipContent>
                  </Tooltip>
                </FormLabel>
                <FormControl>
                  <InputWithUnit
                    unitType="duration"
                    value={(field.value ?? 0) * 86400}
                    onChange={(val) =>
                      field.onChange(val !== null ? Math.round(val / 86400) : 0)
                    }
                    placeholder="0"
                  />
                </FormControl>
                <FormMessage />
              </FormItem>
            )}
          />
          <FormField
            control={control}
            name="session_gap_time_secs"
            render={({ field }) => (
              <FormItem>
                <FormLabel className="flex items-center gap-1.5">
                  <Clock className="h-3.5 w-3.5 text-amber-500/80" />
                  <Trans>Session Merging Gap</Trans>
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <CircleHelp className="h-3.5 w-3.5 text-muted-foreground/40 cursor-help hover:text-muted-foreground transition-colors" />
                    </TooltipTrigger>
                    <TooltipContent className="p-0 border-border/50 shadow-xl bg-background/95 backdrop-blur-md overflow-hidden">
                      <StatusInfoTooltip
                        icon={<Clock className="w-4 h-4" />}
                        title={<Trans>Session Gap</Trans>}
                        theme="amber"
                      >
                        <p className="text-xs leading-relaxed text-muted-foreground">
                          <Trans>
                            Maximum idle time between segments before the system
                            starts a new recording session instead of continuing
                            the current one.
                          </Trans>
                        </p>
                      </StatusInfoTooltip>
                    </TooltipContent>
                  </Tooltip>
                </FormLabel>
                <FormControl>
                  <InputWithUnit
                    unitType="duration"
                    value={field.value}
                    onChange={field.onChange}
                    placeholder="0"
                  />
                </FormControl>
                <FormMessage />
              </FormItem>
            )}
          />
        </div>

        <Separator className="bg-white/5" />

        <FormField
          control={control}
          name="proxy_config"
          render={({ field }) => (
            <FormItem>
              <FormLabel className="sr-only">
                <Trans>Proxy Configuration</Trans>
              </FormLabel>
              <FormControl>
                <ProxyConfigSettings
                  value={field.value}
                  onChange={field.onChange}
                  outputFormat="object"
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
      </div>
    </SettingsCard>
  );
});

NetworkSystemCard.displayName = 'NetworkSystemCard';
