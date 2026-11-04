import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';
import {
  FormField,
  FormItem,
  FormLabel,
  FormControl,
  FormMessage,
  FormDescription,
} from '@/components/ui/form';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Switch } from '@/components/ui/switch';
import {
  ProcessorConfigFormProps,
  useProcessorFieldPath,
} from './common-props';
import { ProcessorFormShell, ProcessorSection } from './processor-form-layout';
import { CompressionConfigSchema } from '../processor-schemas';
import { z } from 'zod';
import { Archive, Settings2 } from 'lucide-react';
import { NumberInput } from '@/components/ui/number-input';

type CompressionConfig = z.infer<typeof CompressionConfigSchema>;

export function CompressionConfigForm({
  control,
  pathPrefix,
}: ProcessorConfigFormProps) {
  const { i18n } = useLingui();
  const fieldPath = useProcessorFieldPath<CompressionConfig>(pathPrefix);

  return (
    <ProcessorFormShell>
      <div className="space-y-6">
        <ProcessorSection
          icon={Archive}
          iconClassName="text-orange-500"
          title={<Trans>Archive Settings</Trans>}
        >
          <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
            <FormField
              control={control}
              name={fieldPath('format')}
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="text-xs text-muted-foreground ml-1">
                    <Trans>Archive Format</Trans>
                  </FormLabel>
                  <Select
                    onValueChange={field.onChange}
                    value={field.value || 'zip'}
                  >
                    <FormControl>
                      <SelectTrigger className="h-11 bg-background/50 border-border/50 focus:bg-background transition-colors rounded-lg">
                        <SelectValue placeholder={i18n._(msg`Select format`)} />
                      </SelectTrigger>
                    </FormControl>
                    <SelectContent>
                      <SelectItem value="zip">ZIP</SelectItem>
                      <SelectItem value="targz">TAR.GZ</SelectItem>
                    </SelectContent>
                  </Select>
                  <FormMessage />
                </FormItem>
              )}
            />

            <FormField
              control={control}
              name={fieldPath('compression_level')}
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="text-xs text-muted-foreground ml-1">
                    <Trans>Compression Level</Trans>
                  </FormLabel>
                  <FormControl>
                    <NumberInput
                      field={field}
                      className="h-11 bg-background/50 border-border/50 focus:bg-background rounded-lg font-mono text-sm"
                      min={0}
                      max={9}
                      step={1}
                    />
                  </FormControl>
                  <FormDescription className="text-[11px] ml-1">
                    <Trans>0 (None) - 9 (Best)</Trans>
                  </FormDescription>
                  <FormMessage />
                </FormItem>
              )}
            />
          </div>
        </ProcessorSection>

        <div className="space-y-4">
          <div className="flex items-center gap-2 pb-2 border-b border-border/40">
            <Settings2 className="w-4 h-4 text-gray-500" />
            <h3 className="font-semibold text-sm mr-auto">
              <Trans>Options</Trans>
            </h3>
          </div>

          <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
            <FormField
              control={control}
              name={fieldPath('preserve_paths')}
              render={({ field }) => (
                <FormItem className="flex flex-row items-center justify-between rounded-lg border border-border/40 p-4 shadow-sm bg-muted/10 transition-colors hover:bg-muted/20">
                  <div className="space-y-1">
                    <FormLabel className="text-sm font-medium">
                      <Trans>Preserve Paths</Trans>
                    </FormLabel>
                    <FormDescription className="text-xs">
                      <Trans>Keep directory structure</Trans>
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
              name={fieldPath('overwrite')}
              render={({ field }) => (
                <FormItem className="flex flex-row items-center justify-between rounded-lg border border-border/40 p-4 shadow-sm bg-muted/10 transition-colors hover:bg-muted/20">
                  <div className="space-y-1">
                    <FormLabel className="text-sm font-medium">
                      <Trans>Overwrite</Trans>
                    </FormLabel>
                    <FormDescription className="text-xs">
                      <Trans>Replace output files</Trans>
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
    </ProcessorFormShell>
  );
}
