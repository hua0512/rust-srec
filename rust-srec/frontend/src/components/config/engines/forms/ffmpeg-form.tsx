// ... imports
import { Control } from 'react-hook-form';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
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
} from 'lucide-react';
import { msg } from '@lingui/core/macro';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';

interface FfmpegFormProps {
  control: Control<any>;
  basePath?: string;
}

export function FfmpegForm({ control, basePath = 'config' }: FfmpegFormProps) {
  const { i18n } = useLingui();
  return (
    <div className="space-y-6">
      <div className="grid gap-6 md:grid-cols-2">
        <FormField
          control={control}
          name={`${basePath}.binary_path`}
          render={({ field }) => (
            <FormItem>
              <FormLabel className="flex items-center gap-2 text-xs uppercase tracking-wider text-muted-foreground font-semibold">
                <Terminal className="w-3.5 h-3.5 text-primary" />
                <Trans>Binary Path</Trans>
              </FormLabel>
              <FormControl>
                <Input
                  {...field}
                  placeholder={i18n._(msg`/usr/bin/ffmpeg or ffmpeg`)}
                  className="bg-background/50"
                />
              </FormControl>
              <FormDescription>
                <Trans>Absolute path or 'ffmpeg' if in PATH</Trans>
              </FormDescription>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          control={control}
          name={`${basePath}.timeout_secs`}
          render={({ field }) => (
            <FormItem>
              <FormLabel className="flex items-center gap-2 text-xs uppercase tracking-wider text-muted-foreground font-semibold">
                <Clock className="w-3.5 h-3.5 text-primary" />
                <Trans>Timeout</Trans>
              </FormLabel>
              <FormControl>
                <div className="relative">
                  <Input
                    type="number"
                    {...field}
                    className="pr-12 bg-background/50"
                  />
                  <span className="absolute right-3 top-2.5 text-xs text-muted-foreground">
                    <Trans>secs</Trans>
                  </span>
                </div>
              </FormControl>
              <FormDescription>
                <Trans>Connection/activity timeout</Trans>
              </FormDescription>
              <FormMessage />
            </FormItem>
          )}
        />
      </div>

      <FormField
        control={control}
        name={`${basePath}.user_agent`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="flex items-center gap-2 text-xs uppercase tracking-wider text-muted-foreground font-semibold">
              <Shield className="w-3.5 h-3.5 text-primary" />
              <Trans>User Agent</Trans>
            </FormLabel>
            <FormControl>
              <Input
                {...field}
                placeholder={i18n._(msg`Mozilla/5.0...`)}
                className="bg-background/50"
              />
            </FormControl>
            <FormDescription>
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
              control={control}
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
                  <FormDescription className="text-[10px]">
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
              control={control}
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
                  <FormDescription className="text-[10px]">
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
