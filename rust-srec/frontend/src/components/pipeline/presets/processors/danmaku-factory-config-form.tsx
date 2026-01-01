import {
  FormField,
  FormItem,
  FormLabel,
  FormControl,
  FormMessage,
  FormDescription,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import { Switch } from '@/components/ui/switch';
import { ProcessorConfigFormProps } from './common-props';
import { DanmakuFactoryConfigSchema } from '../processor-schemas';
import { z } from 'zod';
import { Trans } from '@lingui/react/macro';
import { ListInput } from '@/components/ui/list-input';
import { useLingui } from '@lingui/react';
import { t } from '@lingui/core/macro';
import { Settings, Terminal, Trash2 } from 'lucide-react';
import { motion } from 'motion/react';

type DanmakuFactoryConfig = z.infer<typeof DanmakuFactoryConfigSchema>;

export function DanmakuFactoryConfigForm({
  control,
  pathPrefix,
}: ProcessorConfigFormProps<DanmakuFactoryConfig>) {
  const { i18n } = useLingui();
  const prefix = pathPrefix ? `${pathPrefix}.` : '';

  const containerVariants = {
    hidden: { opacity: 0, y: 20 },
    visible: { opacity: 1, y: 0, transition: { duration: 0.3 } },
  };

  return (
    <motion.div
      variants={containerVariants}
      initial="hidden"
      animate="visible"
      className="w-full"
    >
      <div className="space-y-6">
        {/* Execution Settings */}
        <div className="p-4 rounded-xl bg-muted/10 border border-border/40 space-y-4">
          <div className="flex items-center gap-2 pb-2 border-b border-border/40 mb-2">
            <Terminal className="w-4 h-4 text-blue-500" />
            <h3 className="font-semibold text-sm mr-auto">
              <Trans>Execution Settings</Trans>
            </h3>
          </div>

          <div className="grid grid-cols-1 gap-6">
            <FormField
              control={control}
              name={`${prefix}binary_path` as any}
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="text-xs text-muted-foreground ml-1">
                    <Trans>Binary Path</Trans>
                  </FormLabel>
                  <FormControl>
                    <Input
                      className="h-11 bg-background/50 border-border/50 focus:bg-background rounded-lg font-mono text-sm"
                      placeholder="DanmakuFactory"
                      {...field}
                    />
                  </FormControl>
                  <FormDescription className="text-[11px] ml-1">
                    <Trans>
                      Path to DanmakuFactory binary. If empty, uses environment
                      variable or PATH.
                    </Trans>
                  </FormDescription>
                  <FormMessage />
                </FormItem>
              )}
            />

            <FormField
              control={control}
              name={`${prefix}args` as any}
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="text-xs text-muted-foreground ml-1">
                    <Trans>Command Arguments</Trans>
                  </FormLabel>
                  <FormControl>
                    <ListInput
                      value={field.value || []}
                      onChange={field.onChange}
                      placeholder={t(i18n)`Add argument template`}
                    />
                  </FormControl>
                  <FormDescription className="text-[11px] ml-1">
                    <Trans>
                      Command template args. Use {'{input}'} and {'{output}'}{' '}
                      placeholders.
                    </Trans>
                  </FormDescription>
                  <FormMessage />
                </FormItem>
              )}
            />

            <FormField
              control={control}
              name={`${prefix}extra_args` as any}
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="text-xs text-muted-foreground ml-1">
                    <Trans>Extra Arguments</Trans>
                  </FormLabel>
                  <FormControl>
                    <ListInput
                      value={field.value || []}
                      onChange={field.onChange}
                      placeholder={t(i18n)`Add extra argument`}
                    />
                  </FormControl>
                  <FormDescription className="text-[11px] ml-1">
                    <Trans>Additional arguments appended to the command.</Trans>
                  </FormDescription>
                  <FormMessage />
                </FormItem>
              )}
            />
          </div>
        </div>

        {/* Behavior Settings */}
        <div className="p-4 rounded-xl bg-muted/10 border border-border/40 space-y-4">
          <div className="flex items-center gap-2 pb-2 border-b border-border/40 mb-2">
            <Settings className="w-4 h-4 text-orange-500" />
            <h3 className="font-semibold text-sm mr-auto">
              <Trans>Processing Behavior</Trans>
            </h3>
          </div>

          <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
            <FormField
              control={control}
              name={`${prefix}overwrite` as any}
              render={({ field }) => (
                <FormItem className="flex flex-row items-center justify-between rounded-lg border p-3 shadow-sm bg-background/50">
                  <div className="space-y-0.5">
                    <FormLabel className="text-xs">
                      <Trans>Overwrite Output</Trans>
                    </FormLabel>
                    <FormDescription className="text-[10px]">
                      <Trans>Overwrite existing .ass files</Trans>
                    </FormDescription>
                  </div>
                  <FormControl>
                    <Switch
                      checked={field.value}
                      onCheckedChange={field.onChange}
                    />
                  </FormControl>
                </FormItem>
              )}
            />

            <FormField
              control={control}
              name={`${prefix}verify_output_exists` as any}
              render={({ field }) => (
                <FormItem className="flex flex-row items-center justify-between rounded-lg border p-3 shadow-sm bg-background/50">
                  <div className="space-y-0.5">
                    <FormLabel className="text-xs">
                      <Trans>Verify Output</Trans>
                    </FormLabel>
                    <FormDescription className="text-[10px]">
                      <Trans>Check if file exists after conversion</Trans>
                    </FormDescription>
                  </div>
                  <FormControl>
                    <Switch
                      checked={field.value}
                      onCheckedChange={field.onChange}
                    />
                  </FormControl>
                </FormItem>
              )}
            />

            <FormField
              control={control}
              name={`${prefix}prefer_manifest` as any}
              render={({ field }) => (
                <FormItem className="flex flex-row items-center justify-between rounded-lg border p-3 shadow-sm bg-background/50">
                  <div className="space-y-0.5">
                    <FormLabel className="text-xs">
                      <Trans>Prefer Manifest</Trans>
                    </FormLabel>
                    <FormDescription className="text-[10px]">
                      <Trans>Use danmu_inputs from job manifest</Trans>
                    </FormDescription>
                  </div>
                  <FormControl>
                    <Switch
                      checked={field.value}
                      onCheckedChange={field.onChange}
                    />
                  </FormControl>
                </FormItem>
              )}
            />

            <FormField
              control={control}
              name={`${prefix}passthrough_inputs` as any}
              render={({ field }) => (
                <FormItem className="flex flex-row items-center justify-between rounded-lg border p-3 shadow-sm bg-background/50">
                  <div className="space-y-0.5">
                    <FormLabel className="text-xs">
                      <Trans>Passthrough Inputs</Trans>
                    </FormLabel>
                    <FormDescription className="text-[10px]">
                      <Trans>Include original files in outputs</Trans>
                    </FormDescription>
                  </div>
                  <FormControl>
                    <Switch
                      checked={field.value}
                      onCheckedChange={field.onChange}
                    />
                  </FormControl>
                </FormItem>
              )}
            />
          </div>
        </div>

        {/* Cleanup Settings */}
        <div className="p-4 rounded-xl bg-muted/10 border border-border/40 space-y-4">
          <div className="flex items-center gap-2 pb-2 border-b border-border/40 mb-2">
            <Trash2 className="w-4 h-4 text-red-500" />
            <h3 className="font-semibold text-sm mr-auto">
              <Trans>Cleanup</Trans>
            </h3>
          </div>

          <div className="grid grid-cols-1 gap-6">
            <FormField
              control={control}
              name={`${prefix}delete_source_xml_on_success` as any}
              render={({ field }) => (
                <FormItem className="flex flex-row items-center justify-between rounded-lg border p-3 shadow-sm bg-background/50 border-red-500/20">
                  <div className="space-y-0.5">
                    <FormLabel className="text-xs text-red-600 dark:text-red-400">
                      <Trans>Delete Source XML</Trans>
                    </FormLabel>
                    <FormDescription className="text-[10px]">
                      <Trans>
                        Delete XML files after successful conversion
                      </Trans>
                    </FormDescription>
                  </div>
                  <FormControl>
                    <Switch
                      checked={field.value}
                      onCheckedChange={field.onChange}
                    />
                  </FormControl>
                </FormItem>
              )}
            />
          </div>
        </div>
      </div>
    </motion.div>
  );
}
