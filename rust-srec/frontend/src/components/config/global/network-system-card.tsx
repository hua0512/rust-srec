import { memo } from 'react';
import { SettingsCard } from '../settings-card';
import {
  FormControl,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import { Separator } from '@/components/ui/separator';
import { InputWithUnit } from '@/components/ui/input-with-unit';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import {
  Activity,
  Bell,
  Database,
  History,
  Network,
  ShieldAlert,
  Timer,
} from 'lucide-react';
import { ProxyConfigSettings } from '../shared/proxy-settings-card';
import { FlagFormField } from '@/components/ui/flag-form-field';
import {
  CONFIG_INPUT,
  ConfigFieldLabel,
  ConfigSectionHeading,
  FieldInfo,
} from '@/components/config/shared/config-field';

export const NetworkSystemCard = memo(() => {
  const { i18n } = useLingui();
  const allowPrivateTargetsLabel = i18n._(
    msg`Allow private stream proxy targets`,
  );

  return (
    <SettingsCard
      title={<Trans>Network & System</Trans>}
      description={<Trans>Delays, proxy, and retention policies.</Trans>}
      icon={Network}
      iconColor="text-purple-500"
      iconBgColor="bg-purple-500/10"
    >
      <div className="space-y-8">
        <section className="space-y-4">
          <ConfigSectionHeading icon={Activity}>
            <Trans>Monitoring</Trans>
          </ConfigSectionHeading>

          {/* Three fields, so a three-up grid — a two-up left the last one beside an empty cell. */}
          <div className="grid grid-cols-1 gap-6 @md:grid-cols-2 @2xl:grid-cols-3">
            <FormField
              name="streamer_check_delay_ms"
              render={({ field }) => (
                <FormItem className="space-y-2">
                  <ConfigFieldLabel>
                    <Trans>Streamer Check</Trans>
                    <FieldInfo
                      icon={<Activity className="h-4 w-4" />}
                      title={<Trans>Streamer Check</Trans>}
                    >
                      <Trans>
                        Interval between checks to see if a streamer is
                        currently live. Slower intervals reduce API usage but
                        might delay recording starts.
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
                      placeholder="0"
                    />
                  </FormControl>
                  <FormMessage />
                </FormItem>
              )}
            />

            <FormField
              name="offline_check_delay_ms"
              render={({ field }) => (
                <FormItem className="space-y-2">
                  <ConfigFieldLabel>
                    <Trans>Offline Check</Trans>
                    <FieldInfo
                      icon={<Timer className="h-4 w-4" />}
                      title={<Trans>Offline Check</Trans>}
                      theme="orange"
                    >
                      <Trans>
                        Delay between the re-checks used to confirm a streamer
                        has really gone offline.
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
                      placeholder="0"
                    />
                  </FormControl>
                  <FormMessage />
                </FormItem>
              )}
            />

            <FormField
              name="offline_check_count"
              render={({ field }) => (
                <FormItem className="space-y-2">
                  <ConfigFieldLabel>
                    <Trans>Offline Detection Count</Trans>
                    <FieldInfo
                      icon={<Database className="h-4 w-4" />}
                      title={<Trans>Offline Detection</Trans>}
                      theme="slate"
                    >
                      <Trans>
                        Number of consecutive failed checks required to
                        definitively confirm a streamer has gone offline. Higher
                        values prevent "fake" offline detection.
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
        </section>

        <Separator />

        <section className="space-y-4">
          <ConfigSectionHeading icon={History} accent="indigo">
            <Trans>Retention</Trans>
          </ConfigSectionHeading>

          <div className="grid grid-cols-1 gap-6 @md:grid-cols-2">
            <FormField
              name="job_history_retention_days"
              render={({ field }) => (
                <FormItem className="space-y-2">
                  <ConfigFieldLabel>
                    <Trans>Pipeline History Retention</Trans>
                    <FieldInfo
                      icon={<History className="h-4 w-4" />}
                      title={<Trans>Pipeline History Retention</Trans>}
                      theme="violet"
                    >
                      <Trans>
                        Number of days to keep completed, failed, or cancelled
                        jobs and workflow executions. Set to 0 to retain them
                        indefinitely.
                      </Trans>
                    </FieldInfo>
                  </ConfigFieldLabel>
                  <FormControl>
                    <InputWithUnit
                      unitType="duration"
                      value={(field.value ?? 0) * 86400}
                      onChange={(val) =>
                        field.onChange(
                          val !== null ? Math.round(val / 86400) : 0,
                        )
                      }
                      placeholder="0"
                    />
                  </FormControl>
                  <FormMessage />
                </FormItem>
              )}
            />

            <FormField
              name="notification_event_log_retention_days"
              render={({ field }) => (
                <FormItem className="space-y-2">
                  <ConfigFieldLabel>
                    <Trans>Notification Log Retention</Trans>
                    <FieldInfo
                      icon={<Bell className="h-4 w-4" />}
                      title={<Trans>Notification Retention</Trans>}
                      theme="rose"
                    >
                      <Trans>
                        Number of days to keep the notification event log. Set
                        to 0 to retain events indefinitely.
                      </Trans>
                    </FieldInfo>
                  </ConfigFieldLabel>
                  <FormControl>
                    <InputWithUnit
                      unitType="duration"
                      value={(field.value ?? 0) * 86400}
                      onChange={(val) =>
                        field.onChange(
                          val !== null ? Math.round(val / 86400) : 0,
                        )
                      }
                      placeholder="0"
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
          <ConfigSectionHeading icon={Network} accent="sky">
            <Trans>Proxy</Trans>
          </ConfigSectionHeading>

          <FormField
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

          <FlagFormField
            fieldName="stream_proxy_allow_private_targets"
            title={<Trans>Allow Private Stream Proxy Targets</Trans>}
            ariaLabel={allowPrivateTargetsLabel}
            description={
              <Trans>
                Let the player proxy sources on private or local networks.
              </Trans>
            }
            info={
              <FieldInfo
                icon={<ShieldAlert className="h-4 w-4" />}
                title={<Trans>Private Network Access</Trans>}
                theme="amber"
              >
                <Trans>
                  Covers LAN restreamers, cameras and tailnet addresses. Leave
                  this off unless you stream from local sources: it re-opens
                  requests to internal addresses for any signed-in user.
                </Trans>
              </FieldInfo>
            }
          />
        </section>
      </div>
    </SettingsCard>
  );
});

NetworkSystemCard.displayName = 'NetworkSystemCard';
