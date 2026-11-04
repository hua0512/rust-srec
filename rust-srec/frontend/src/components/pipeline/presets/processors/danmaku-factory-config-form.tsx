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
import {
  ProcessorConfigFormProps,
  useProcessorFieldPath,
} from './common-props';
import { ProcessorFormShell, ProcessorSection } from './processor-form-layout';
import { DanmakuFactoryConfigSchema } from '../processor-schemas';
import { z } from 'zod';
import { Trans } from '@lingui/react/macro';
import { ListInput } from '@/components/ui/list-input';
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';
import { Settings, Terminal, Trash2 } from 'lucide-react';

type DanmakuFactoryConfig = z.infer<typeof DanmakuFactoryConfigSchema>;

const INPUT_TOKEN = '{input}';
const OUTPUT_TOKEN = '{output}';

export function DanmakuFactoryConfigForm({
  control,
  pathPrefix,
}: ProcessorConfigFormProps) {
  const { i18n } = useLingui();
  const fieldPath = useProcessorFieldPath<DanmakuFactoryConfig>(pathPrefix);

  return (
    <ProcessorFormShell>
      <div className="space-y-6">
        {/* Execution Settings */}
        <ProcessorSection
          icon={Terminal}
          iconClassName="text-blue-500"
          title={<Trans>Execution Settings</Trans>}
        >
          <div className="grid grid-cols-1 gap-6">
            <FormField
              control={control}
              name={fieldPath('binary_path')}
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="text-xs text-muted-foreground ml-1">
                    <Trans>Binary Path</Trans>
                  </FormLabel>
                  <FormControl>
                    <Input
                      className="h-11 bg-background/50 border-border/50 focus:bg-background rounded-lg font-mono text-sm"
                      placeholder={i18n._(msg`DanmakuFactory`)}
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
              name={fieldPath('args')}
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="text-xs text-muted-foreground ml-1">
                    <Trans>Command Arguments</Trans>
                  </FormLabel>
                  <FormControl>
                    <ListInput
                      value={field.value || []}
                      onChange={field.onChange}
                      placeholder={i18n._(msg`Add argument template`)}
                    />
                  </FormControl>
                  <FormDescription className="text-[11px] ml-1">
                    {/* Tokens are bound values: literal `{...}` inlined into a
                        lingui message becomes an unbound ICU placeholder and
                        renders as an empty string. */}
                    <Trans>
                      Command template args. Use {INPUT_TOKEN} and{' '}
                      {OUTPUT_TOKEN} placeholders.
                    </Trans>
                  </FormDescription>
                  <FormMessage />
                </FormItem>
              )}
            />

            <FormField
              control={control}
              name={fieldPath('extra_args')}
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="text-xs text-muted-foreground ml-1">
                    <Trans>Extra Arguments</Trans>
                  </FormLabel>
                  <FormControl>
                    <ListInput
                      value={field.value || []}
                      onChange={field.onChange}
                      placeholder={i18n._(msg`Add extra argument`)}
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
        </ProcessorSection>

        {/* Behavior Settings */}
        <ProcessorSection
          icon={Settings}
          iconClassName="text-orange-500"
          title={<Trans>Processing Behavior</Trans>}
        >
          <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
            <FormField
              control={control}
              name={fieldPath('overwrite')}
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
              name={fieldPath('verify_output_exists')}
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
              name={fieldPath('prefer_manifest')}
              render={({ field }) => (
                <FormItem className="flex flex-row items-center justify-between rounded-lg border p-3 shadow-sm bg-background/50">
                  <div className="space-y-0.5">
                    <FormLabel className="text-xs">
                      <Trans>Prefer session pairing</Trans>
                    </FormLabel>
                    <FormDescription className="text-[10px]">
                      <Trans>
                        Convert only the danmu recorded for this session's
                        segments
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

            <FormField
              control={control}
              name={fieldPath('passthrough_inputs')}
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
        </ProcessorSection>

        {/* Cleanup Settings */}
        <ProcessorSection
          icon={Trash2}
          iconClassName="text-red-500"
          title={<Trans>Cleanup</Trans>}
        >
          <div className="grid grid-cols-1 gap-6">
            <FormField
              control={control}
              name={fieldPath('delete_source_xml_on_success')}
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
        </ProcessorSection>
      </div>
    </ProcessorFormShell>
  );
}
