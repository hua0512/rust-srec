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
import { Terminal, Settings, Command } from 'lucide-react';
import { t } from '@lingui/core/macro';
import { Trans } from '@lingui/react/macro';

interface StreamlinkFormProps {
  control: Control<any>;
  basePath?: string;
}

export function StreamlinkForm({
  control,
  basePath = 'config',
}: StreamlinkFormProps) {
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
                  placeholder={t`streamlink`}
                  className="bg-background/50"
                />
              </FormControl>
              <FormDescription>
                <Trans>Absolute path or 'streamlink' in PATH</Trans>
              </FormDescription>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          control={control}
          name={`${basePath}.quality`}
          render={({ field }) => (
            <FormItem>
              <FormLabel className="flex items-center gap-2 text-xs uppercase tracking-wider text-muted-foreground font-semibold">
                <Settings className="w-3.5 h-3.5 text-primary" />
                <Trans>Quality</Trans>
              </FormLabel>
              <FormControl>
                <Input
                  {...field}
                  placeholder={t`best`}
                  className="bg-background/50"
                />
              </FormControl>
              <FormDescription>
                <Trans>e.g. 'best', 'worst', '720p', 'audio_only'</Trans>
              </FormDescription>
              <FormMessage />
            </FormItem>
          )}
        />
      </div>

      <Separator className="bg-border/50" />

      <Card className="border-border/40 bg-background/40 shadow-sm">
        <CardHeader className="pb-3 pt-4 px-4">
          <CardTitle className="text-sm font-medium flex items-center gap-2">
            <Command className="w-4 h-4 text-primary" />
            <Trans>Extra Arguments</Trans>
          </CardTitle>
        </CardHeader>
        <CardContent className="px-4 pb-4">
          <FormField
            control={control}
            name={`${basePath}.extra_args`}
            render={({ field }) => (
              <FormItem>
                <FormControl>
                  <ListInput
                    value={field.value}
                    onChange={field.onChange}
                    placeholder={t`--hls-live-edge 3`}
                    className="bg-background/50"
                  />
                </FormControl>
                <FormDescription className="text-[10px]">
                  <Trans>
                    Any additional command line arguments to pass to Streamlink
                  </Trans>
                </FormDescription>
                <FormMessage />
              </FormItem>
            )}
          />
        </CardContent>
      </Card>
    </div>
  );
}
