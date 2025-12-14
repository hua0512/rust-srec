import { Trans } from '@lingui/macro';
import {
  FormField,
  FormItem,
  FormLabel,
  FormControl,
  FormMessage,
  FormDescription,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Switch } from '@/components/ui/switch';
import { ProcessorConfigFormProps } from './common-props';
import { CompressionConfigSchema } from '../processor-schemas';
import { z } from 'zod';
import { motion } from 'motion/react';
import { Archive, Settings2 } from 'lucide-react';

type CompressionConfig = z.infer<typeof CompressionConfigSchema>;

export function CompressionConfigForm({
  control,
  pathPrefix,
}: ProcessorConfigFormProps<CompressionConfig>) {
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
        <div className="p-4 rounded-xl bg-muted/10 border border-border/40 space-y-4">
          <div className="flex items-center gap-2 pb-2 border-b border-border/40 mb-2">
            <Archive className="w-4 h-4 text-orange-500" />
            <h3 className="font-semibold text-sm mr-auto">
              <Trans>Archive Settings</Trans>
            </h3>
          </div>

          <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
            <FormField
              control={control}
              name={`${prefix}format` as any}
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="text-xs text-muted-foreground ml-1">
                    <Trans>Archive Format</Trans>
                  </FormLabel>
                  <Select
                    onValueChange={field.onChange}
                    defaultValue={field.value}
                  >
                    <FormControl>
                      <SelectTrigger className="h-11 bg-background/50 border-border/50 focus:bg-background transition-colors rounded-lg">
                        <SelectValue placeholder="Select format" />
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
              name={`${prefix}compression_level` as any}
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="text-xs text-muted-foreground ml-1">
                    <Trans>Compression Level</Trans>
                  </FormLabel>
                  <FormControl>
                    <Input
                      className="h-11 bg-background/50 border-border/50 focus:bg-background rounded-lg font-mono text-sm"
                      type="number"
                      min={0}
                      max={9}
                      {...field}
                      onChange={(e) => field.onChange(parseInt(e.target.value))}
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
        </div>

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
              name={`${prefix}preserve_paths` as any}
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
              name={`${prefix}overwrite` as any}
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
    </motion.div>
  );
}
