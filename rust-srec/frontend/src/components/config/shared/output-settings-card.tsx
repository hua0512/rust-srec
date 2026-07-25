import { memo } from 'react';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormMessage,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { FolderOpen } from 'lucide-react';
import { UseFormReturn } from 'react-hook-form';
import { EngineConfig } from '@/api/schemas';
import {
  CONFIG_DESCRIPTION,
  CONFIG_SELECT_TRIGGER,
  CONFIG_INPUT,
  CONFIG_SELECT_CONTENT,
  ConfigFieldLabel,
} from './config-field';

interface OutputSettingsCardProps {
  form: UseFormReturn<any>;
  basePath?: string;
  engines?: EngineConfig[];
}

export const OutputSettingsCard = memo(
  ({ form, basePath, engines }: OutputSettingsCardProps) => {
    const { i18n } = useLingui();
    return (
      <Card className="border-border/50 shadow-sm hover:shadow-md transition-all">
        <CardHeader className="pb-3">
          <div className="flex items-center gap-3">
            <div className="p-2 rounded-lg bg-orange-500/10 text-orange-600 dark:text-orange-400">
              <FolderOpen className="w-5 h-5" />
            </div>
            <div className="space-y-1">
              <CardTitle className="text-lg">
                <Trans>Output Configuration</Trans>
              </CardTitle>
              <p className="text-sm text-muted-foreground">
                <Trans>Manage storage paths and file formats.</Trans>
              </p>
            </div>
          </div>
        </CardHeader>
        <CardContent className="space-y-6">
          <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
            <FormField
              control={form.control}
              name={basePath ? `${basePath}.output_folder` : 'output_folder'}
              render={({ field }) => (
                <FormItem className="space-y-2">
                  <ConfigFieldLabel>
                    <Trans>Output Folder</Trans>
                  </ConfigFieldLabel>
                  <FormControl>
                    <Input
                      {...field}
                      value={field.value ?? ''}
                      onChange={field.onChange}
                      placeholder="/app/output"
                      className={CONFIG_INPUT}
                    />
                  </FormControl>
                  <FormDescription className={CONFIG_DESCRIPTION}>
                    <Trans>Override output folder.</Trans>
                  </FormDescription>
                  <FormMessage />
                </FormItem>
              )}
            />

            <FormField
              control={form.control}
              name={
                basePath
                  ? `${basePath}.output_filename_template`
                  : 'output_filename_template'
              }
              render={({ field }) => (
                <FormItem className="space-y-2">
                  <ConfigFieldLabel>
                    <Trans>Filename Template</Trans>
                  </ConfigFieldLabel>
                  <FormControl>
                    <Input
                      {...field}
                      value={field.value ?? ''}
                      onChange={field.onChange}
                      placeholder="{streamer}-%Y%m%d-%H%M%S-{title}"
                      className={CONFIG_INPUT}
                    />
                  </FormControl>
                  <FormDescription className={CONFIG_DESCRIPTION}>
                    <Trans>Override filename template.</Trans>
                  </FormDescription>
                  <FormMessage />
                </FormItem>
              )}
            />
          </div>

          <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
            <FormField
              control={form.control}
              name={
                basePath
                  ? `${basePath}.output_file_format`
                  : 'output_file_format'
              }
              render={({ field }) => (
                <FormItem className="space-y-2">
                  <ConfigFieldLabel>
                    <Trans>Output Format</Trans>
                  </ConfigFieldLabel>
                  <Select
                    onValueChange={(val) =>
                      field.onChange(val === 'default' ? undefined : val)
                    }
                    value={field.value || 'default'}
                  >
                    <FormControl>
                      <SelectTrigger className={CONFIG_SELECT_TRIGGER}>
                        <SelectValue
                          placeholder={i18n._(msg`Select a format`)}
                        />
                      </SelectTrigger>
                    </FormControl>
                    <SelectContent className={CONFIG_SELECT_CONTENT}>
                      <SelectItem value="default">
                        <Trans>Global Default</Trans>
                      </SelectItem>
                      <SelectItem value="mp4">MP4</SelectItem>
                      <SelectItem value="flv">FLV</SelectItem>
                      <SelectItem value="mkv">MKV</SelectItem>
                      <SelectItem value="ts">TS</SelectItem>
                    </SelectContent>
                  </Select>
                  <FormDescription className={CONFIG_DESCRIPTION}>
                    <Trans>Force specific output format.</Trans>
                  </FormDescription>
                  <FormMessage />
                </FormItem>
              )}
            />

            <FormField
              control={form.control}
              name={
                basePath ? `${basePath}.download_engine` : 'download_engine'
              }
              render={({ field }) => (
                <FormItem className="space-y-2">
                  <ConfigFieldLabel>
                    <Trans>Download Engine</Trans>
                  </ConfigFieldLabel>
                  <Select
                    onValueChange={(val) =>
                      field.onChange(val === 'default' ? undefined : val)
                    }
                    value={field.value || 'default'}
                  >
                    <FormControl>
                      <SelectTrigger className={CONFIG_SELECT_TRIGGER}>
                        <SelectValue
                          placeholder={i18n._(msg`Select an engine`)}
                        />
                      </SelectTrigger>
                    </FormControl>
                    <SelectContent className={CONFIG_SELECT_CONTENT}>
                      <SelectItem value="default">
                        <Trans>Inherited / Default</Trans>
                      </SelectItem>
                      {engines?.map((engine) => (
                        <SelectItem key={engine.id} value={engine.id}>
                          {engine.name} ({engine.engine_type})
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                  <FormDescription className={CONFIG_DESCRIPTION}>
                    <Trans>
                      Which tool downloads the stream once its URL is known.
                    </Trans>
                  </FormDescription>
                  <FormMessage />
                </FormItem>
              )}
            />

            <FormField
              control={form.control}
              name={basePath ? `${basePath}.extractor` : 'extractor'}
              render={({ field }) => (
                <FormItem className="space-y-2">
                  <ConfigFieldLabel>
                    <Trans>Stream lookup</Trans>
                  </ConfigFieldLabel>
                  <Select
                    onValueChange={(val) =>
                      field.onChange(val === 'default' ? undefined : val)
                    }
                    value={field.value || 'default'}
                  >
                    <FormControl>
                      <SelectTrigger className={CONFIG_SELECT_TRIGGER}>
                        <SelectValue />
                      </SelectTrigger>
                    </FormControl>
                    <SelectContent className={CONFIG_SELECT_CONTENT}>
                      <SelectItem value="default">
                        <Trans>Inherited / Default</Trans>
                      </SelectItem>
                      <SelectItem value="auto">
                        <Trans>Automatic (match the site)</Trans>
                      </SelectItem>
                      <SelectItem value="streamlink">
                        <Trans>Always use Streamlink</Trans>
                      </SelectItem>
                    </SelectContent>
                  </Select>
                  <FormDescription className={CONFIG_DESCRIPTION}>
                    <Trans>
                      How the stream address is found. Switch to Streamlink when
                      a site stops working; it needs the Streamlink tool
                      installed.
                    </Trans>
                  </FormDescription>
                  <FormMessage />
                </FormItem>
              )}
            />
          </div>
        </CardContent>
      </Card>
    );
  },
);

OutputSettingsCard.displayName = 'OutputSettingsCard';
