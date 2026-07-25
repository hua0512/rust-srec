// ... imports
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormMessage,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import { ListInput } from '@/components/ui/list-input';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Separator } from '@/components/ui/separator';
import {
  Terminal,
  Clock,
  Shield,
  ArrowRightFromLine,
  ArrowLeftFromLine,
  TimerOff,
} from 'lucide-react';
import { msg } from '@lingui/core/macro';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';

import { InputWithUnit } from '@/components/ui/input-with-unit';
import {
  CONFIG_DESCRIPTION,
  ConfigFieldLabel,
} from '@/components/config/shared/config-field';

interface FfmpegFormProps {
  basePath?: string;
}

export function FfmpegForm({ basePath = 'config' }: FfmpegFormProps) {
  const { i18n } = useLingui();
  return (
    <div className="space-y-6">
      <div className="grid gap-6 md:grid-cols-2">
        <FormField
          name={`${basePath}.binary_path`}
          render={({ field }) => (
            <FormItem>
              <ConfigFieldLabel>
                <Terminal className="w-3.5 h-3.5 text-primary" />
                <Trans>Binary Path</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <Input
                  {...field}
                  placeholder={i18n._(msg`/usr/bin/ffmpeg or ffmpeg`)}
                  className="bg-background/50"
                />
              </FormControl>
              <FormDescription className={CONFIG_DESCRIPTION}>
                <Trans>Absolute path or 'ffmpeg' if in PATH</Trans>
              </FormDescription>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          name={`${basePath}.timeout_secs`}
          render={({ field }) => (
            <FormItem>
              <ConfigFieldLabel>
                <Clock className="w-3.5 h-3.5 text-primary" />
                <Trans>Timeout</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <InputWithUnit
                  value={field.value}
                  onChange={field.onChange}
                  unitType="duration"
                  className="bg-background/50"
                />
              </FormControl>
              <FormDescription className={CONFIG_DESCRIPTION}>
                <Trans>Connection/activity timeout</Trans>
              </FormDescription>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          name={`${basePath}.graceful_stop_timeout_secs`}
          render={({ field }) => (
            <FormItem>
              <ConfigFieldLabel>
                <TimerOff className="w-3.5 h-3.5 text-primary" />
                <Trans>Graceful Stop Timeout</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <InputWithUnit
                  value={field.value}
                  onChange={field.onChange}
                  unitType="duration"
                  className="bg-background/50"
                />
              </FormControl>
              <FormDescription className={CONFIG_DESCRIPTION}>
                <Trans>
                  Time to wait for ffmpeg to finalize the file before
                  force-killing it
                </Trans>
              </FormDescription>
              <FormMessage />
            </FormItem>
          )}
        />
      </div>

      <FormField
        name={`${basePath}.user_agent`}
        render={({ field }) => (
          <FormItem>
            <ConfigFieldLabel>
              <Shield className="w-3.5 h-3.5 text-primary" />
              <Trans>User Agent</Trans>
            </ConfigFieldLabel>
            <FormControl>
              <Input
                {...field}
                placeholder={i18n._(msg`Mozilla/5.0...`)}
                className="bg-background/50"
              />
            </FormControl>
            <FormDescription className={CONFIG_DESCRIPTION}>
              <Trans>Custom User-Agent string (Optional)</Trans>
            </FormDescription>
            <FormMessage />
          </FormItem>
        )}
      />

      <Separator className="bg-border/50" />

      <div className="grid gap-6 md:grid-cols-2">
        <Card className="border-border/40 bg-background/40 shadow-sm">
          <CardHeader className="pb-3 pt-4 px-4">
            <CardTitle className="text-sm font-medium flex items-center gap-2">
              <ArrowRightFromLine className="w-4 h-4 text-emerald-500" />
              <Trans>Input Arguments</Trans>
            </CardTitle>
          </CardHeader>
          <CardContent className="px-4 pb-4">
            <FormField
              name={`${basePath}.input_args`}
              render={({ field }) => (
                <FormItem>
                  <FormControl>
                    <ListInput
                      value={field.value}
                      onChange={field.onChange}
                      placeholder={i18n._(msg`-reconnect 1`)}
                      className="bg-background/50"
                    />
                  </FormControl>
                  <FormDescription className={CONFIG_DESCRIPTION}>
                    <Trans>Args inserted before -i input_url</Trans>
                  </FormDescription>
                  <FormMessage />
                </FormItem>
              )}
            />
          </CardContent>
        </Card>

        <Card className="border-border/40 bg-background/40 shadow-sm">
          <CardHeader className="pb-3 pt-4 px-4">
            <CardTitle className="text-sm font-medium flex items-center gap-2">
              <ArrowLeftFromLine className="w-4 h-4 text-sky-500" />
              <Trans>Output Arguments</Trans>
            </CardTitle>
          </CardHeader>
          <CardContent className="px-4 pb-4">
            <FormField
              name={`${basePath}.output_args`}
              render={({ field }) => (
                <FormItem>
                  <FormControl>
                    <ListInput
                      value={field.value}
                      onChange={field.onChange}
                      placeholder={i18n._(msg`-c copy`)}
                      className="bg-background/50"
                    />
                  </FormControl>
                  <FormDescription className={CONFIG_DESCRIPTION}>
                    <Trans>Args used for processing/encoding</Trans>
                  </FormDescription>
                  <FormMessage />
                </FormItem>
              )}
            />
          </CardContent>
        </Card>
      </div>
    </div>
  );
}
