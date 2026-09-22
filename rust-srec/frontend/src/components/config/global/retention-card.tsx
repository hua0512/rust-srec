import { memo } from 'react';
import { Trans } from '@lingui/react/macro';
import { Bell, History } from 'lucide-react';
import { SettingsCard } from '../settings-card';
import { OutputRetentionFields } from './output-retention-fields';
import {
  FormControl,
  FormField,
  FormItem,
  FormMessage,
} from '@/components/ui/form';
import { InputWithUnit } from '@/components/ui/input-with-unit';
import {
  ConfigFieldLabel,
  FieldInfo,
} from '@/components/config/shared/config-field';

export const RetentionCard = memo(() => (
  <SettingsCard
    title={<Trans>Retention</Trans>}
    description={<Trans>Output cleanup and history retention.</Trans>}
    icon={History}
    iconColor="text-indigo-500"
    iconBgColor="bg-indigo-500/10"
  >
    <div className="grid grid-cols-1 gap-6 @md:grid-cols-2">
      <OutputRetentionFields />
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
                  Number of days to keep completed, failed, or cancelled jobs
                  and workflow executions. Set to 0 to retain them indefinitely.
                </Trans>
              </FieldInfo>
            </ConfigFieldLabel>
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
                  Number of days to keep the notification event log. Set to 0 to
                  retain events indefinitely.
                </Trans>
              </FieldInfo>
            </ConfigFieldLabel>
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
    </div>
  </SettingsCard>
));

RetentionCard.displayName = 'RetentionCard';
