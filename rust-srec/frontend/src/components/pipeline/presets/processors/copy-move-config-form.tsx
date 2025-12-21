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
import { CopyMoveConfigSchema } from '../processor-schemas';
import { z } from 'zod';
import { motion } from 'motion/react';
import { Copy, Settings2 } from 'lucide-react';

type CopyMoveConfig = z.infer<typeof CopyMoveConfigSchema>;

export function CopyMoveConfigForm({
  control,
  pathPrefix,
}: ProcessorConfigFormProps<CopyMoveConfig>) {
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
            <Copy className="w-4 h-4 text-blue-500" />
            <h3 className="font-semibold text-sm mr-auto">
              <Trans>Operation</Trans>
            </h3>
          </div>

          <div className="grid grid-cols-1 gap-6">
            <FormField
              control={control}
              name={`${prefix}operation` as any}
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="text-xs text-muted-foreground ml-1">
                    <Trans>Operation Type</Trans>
                  </FormLabel>
                  <Select
                    onValueChange={field.onChange}
                    value={field.value || 'copy'}
                  >
                    <FormControl>
                      <SelectTrigger className="h-11 bg-background/50 border-border/50 focus:bg-background transition-colors rounded-lg">
                        <SelectValue placeholder="Select operation" />
                      </SelectTrigger>
                    </FormControl>
                    <SelectContent>
                      <SelectItem value="copy">
                        <Trans>Copy Files</Trans>
                      </SelectItem>
                      <SelectItem value="move">
                        <Trans>Move Files</Trans>
                      </SelectItem>
                    </SelectContent>
                  </Select>
                  <FormMessage />
                </FormItem>
              )}
            />

            <FormField
              control={control}
              name={`${prefix}destination` as any}
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="text-xs text-muted-foreground ml-1">
                    <Trans>Destination Path (Optional)</Trans>
                  </FormLabel>
                  <FormControl>
                    <Input
                      className="h-11 bg-background/50 border-border/50 focus:bg-background rounded-lg font-mono text-sm"
                      {...field}
                      value={field.value ?? ''}
                      placeholder="/path/to/destination"
                    />
                  </FormControl>
                  <FormDescription className="text-[11px] ml-1">
                    <Trans>If empty, uses next output path in pipeline</Trans>
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
              name={`${prefix}create_dirs` as any}
              render={({ field }) => (
                <FormItem className="flex flex-row items-center justify-between rounded-xl border border-border/40 p-4 shadow-sm bg-muted/10 transition-colors hover:bg-muted/20">
                  <div className="space-y-1">
                    <FormLabel className="text-sm font-medium">
                      <Trans>Create Directories</Trans>
                    </FormLabel>
                    <FormDescription className="text-xs">
                      <Trans>Create missing folders</Trans>
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
              name={`${prefix}verify_integrity` as any}
              render={({ field }) => (
                <FormItem className="flex flex-row items-center justify-between rounded-xl border border-border/40 p-4 shadow-sm bg-muted/10 transition-colors hover:bg-muted/20">
                  <div className="space-y-1">
                    <FormLabel className="text-sm font-medium">
                      <Trans>Verify Integrity</Trans>
                    </FormLabel>
                    <FormDescription className="text-xs">
                      <Trans>Check size after copy</Trans>
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
