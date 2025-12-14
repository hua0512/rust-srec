import { InputWithUnit } from '@/components/ui/input-with-unit';
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
import { ProcessorConfigFormProps } from './common-props';
import { ThumbnailConfigSchema } from '../processor-schemas';
import { z } from 'zod';
import { motion } from 'motion/react';
import { Camera, FileImage } from 'lucide-react';

type ThumbnailConfig = z.infer<typeof ThumbnailConfigSchema>;

export function ThumbnailConfigForm({
  control,
  pathPrefix,
}: ProcessorConfigFormProps<ThumbnailConfig>) {
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
            <Camera className="w-4 h-4 text-indigo-500" />
            <h3 className="font-semibold text-sm mr-auto">
              <Trans>Capture Settings</Trans>
            </h3>
          </div>

          <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
            <FormField
              control={control}
              name={`${prefix}timestamp_secs` as any}
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="text-xs text-muted-foreground ml-1">
                    <Trans>Timestamp</Trans>
                  </FormLabel>
                  <FormControl>
                    <InputWithUnit
                      unitType="duration"
                      min={0}
                      step={1}
                      value={field.value}
                      onChange={(val) => field.onChange(val ?? 0)}
                      className="bg-background/50"
                    />
                  </FormControl>
                  <FormDescription className="text-[11px] ml-1">
                    <Trans>Time offset to take screenshot from</Trans>
                  </FormDescription>
                  <FormMessage />
                </FormItem>
              )}
            />

            <FormField
              control={control}
              name={`${prefix}width` as any}
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="text-xs text-muted-foreground ml-1">
                    <Trans>Width</Trans>
                  </FormLabel>
                  <FormControl>
                    <Input
                      className="h-11 bg-background/50 border-border/50 focus:bg-background rounded-lg font-mono text-sm"
                      type="number"
                      min={1}
                      step={1}
                      placeholder="320"
                      {...field}
                      onChange={(e) => field.onChange(parseInt(e.target.value))}
                    />
                  </FormControl>
                  <FormDescription className="text-[11px] ml-1">
                    <Trans>Width in pixels (keeps aspect ratio)</Trans>
                  </FormDescription>
                  <FormMessage />
                </FormItem>
              )}
            />

            <FormField
              control={control}
              name={`${prefix}quality` as any}
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="text-xs text-muted-foreground ml-1">
                    <Trans>Quality (qscale)</Trans>
                  </FormLabel>
                  <FormControl>
                    <Input
                      className="h-11 bg-background/50 border-border/50 focus:bg-background rounded-lg font-mono text-sm"
                      type="number"
                      min={1}
                      max={31}
                      placeholder="2"
                      {...field}
                      onChange={(e) => field.onChange(parseInt(e.target.value))}
                    />
                  </FormControl>
                  <FormDescription className="text-[11px] ml-1">
                    <Trans>1 (best) - 31 (worst)</Trans>
                  </FormDescription>
                  <FormMessage />
                </FormItem>
              )}
            />
          </div>
        </div>

        <div className="p-4 rounded-xl bg-muted/10 border border-border/40 space-y-4">
          <div className="flex items-center gap-2 pb-2 border-b border-border/40 mb-2">
            <FileImage className="w-4 h-4 text-cyan-500" />
            <h3 className="font-semibold text-sm mr-auto">
              <Trans>Output</Trans>
            </h3>
          </div>

          <div className="grid grid-cols-1 gap-6">
            <FormField
              control={control}
              name={`${prefix}output_pattern` as any}
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="text-xs text-muted-foreground ml-1">
                    <Trans>Output Pattern (Optional)</Trans>
                  </FormLabel>
                  <FormControl>
                    <Input
                      className="h-11 bg-background/50 border-border/50 focus:bg-background rounded-lg font-mono text-sm"
                      {...field}
                      placeholder="thumb_%03d.jpg"
                    />
                  </FormControl>
                  <FormDescription className="text-[11px] ml-1">
                    <Trans>Filename pattern (e.g., thumb.jpg)</Trans>
                  </FormDescription>
                  <FormMessage />
                </FormItem>
              )}
            />
          </div>
        </div>
      </div>
    </motion.div>
  );
}
