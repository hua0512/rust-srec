import { useState } from 'react';
import { Button } from '@/components/ui/button';
import { RotateCcw, Wand2 } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import {
  getProcessorDefaultConfig,
  getProcessorDefinition,
} from '../processors/registry';
import { ProcessorConfigManager } from '../processors/processor-config-manager';
import { motion } from 'motion/react';
import type { Control, FieldValues, UseFormReturn } from 'react-hook-form';
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog';
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card';
import type { PresetFormValues } from '../preset-editor';

// The one remux recipe that is more than the schema defaults (stream copy).
const H264_TRANSCODE = {
  video_codec: 'h264',
  audio_codec: 'aac',
  resolution: '1920x1080',
  crf: 23,
  preset: 'medium',
} as const;

const BUTTON_CLASS =
  'h-8 text-xs font-medium bg-secondary/50 hover:bg-secondary text-secondary-foreground gap-1.5 rounded-lg transition-colors';

interface PresetConfigFormProps {
  form: UseFormReturn<PresetFormValues>;
  currentProcessor: string;
}

export function PresetConfigForm({
  form,
  currentProcessor,
}: PresetConfigFormProps) {
  const [pendingConfig, setPendingConfig] = useState<Record<
    string,
    unknown
  > | null>(null);
  const defaults = getProcessorDefaultConfig(
    getProcessorDefinition(currentProcessor),
  );
  const configDirty = form.getFieldState('config', form.formState).isDirty;

  const applyConfig = (config: Record<string, unknown>) => {
    form.setValue('config', config, { shouldDirty: true });
  };
  // Replacing the whole config discards edits, so ask first when there are any.
  const requestConfig = (config: Record<string, unknown>) => {
    if (configDirty) setPendingConfig(config);
    else applyConfig(config);
  };

  return (
    <motion.div
      initial={{ opacity: 0, y: 20 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.4, delay: 0.1 }}
    >
      <Card className="border-border/40 shadow-sm bg-card/80 backdrop-blur-sm">
        <CardHeader className="pb-6 border-b border-border/40 bg-muted/10">
          <div className="flex justify-between items-center">
            <div className="flex flex-col gap-0.5">
              <CardTitle className="text-lg font-semibold tracking-tight">
                <Trans>Configuration</Trans>
              </CardTitle>
              <CardDescription className="text-xs font-normal text-muted-foreground/80">
                <Trans>Detailed processor settings</Trans>
              </CardDescription>
            </div>
            {defaults && (
              <div className="flex gap-2">
                {currentProcessor === 'remux' && (
                  <Button
                    type="button"
                    variant="secondary"
                    size="sm"
                    onClick={() =>
                      requestConfig({ ...defaults, ...H264_TRANSCODE })
                    }
                    className={BUTTON_CLASS}
                  >
                    <Wand2 className="w-3.5 h-3.5" /> <Trans>H.264</Trans>
                  </Button>
                )}
                <Button
                  type="button"
                  variant="secondary"
                  size="sm"
                  onClick={() => requestConfig(defaults)}
                  className={BUTTON_CLASS}
                >
                  <RotateCcw className="w-3.5 h-3.5" />{' '}
                  <Trans>Reset to defaults</Trans>
                </Button>
              </div>
            )}
          </div>
        </CardHeader>
        <CardContent className="p-6 md:p-8">
          <ProcessorConfigManager
            processorType={currentProcessor}
            // Each processor form in the registry is typed against its own config object and
            // addresses it relative to `pathPrefix`. That relation between a prefix and the
            // surrounding form is not expressible here, so the control crosses the boundary
            // under the base form type.
            control={form.control as unknown as Control<FieldValues>}
            pathPrefix="config"
          />
        </CardContent>
      </Card>
      <AlertDialog
        open={pendingConfig !== null}
        onOpenChange={(open) => {
          if (!open) setPendingConfig(null);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              <Trans>Replace configuration?</Trans>
            </AlertDialogTitle>
            <AlertDialogDescription>
              <Trans>Your changes to this configuration will be lost.</Trans>
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>
              <Trans>Cancel</Trans>
            </AlertDialogCancel>
            <AlertDialogAction
              onClick={() => {
                if (pendingConfig) applyConfig(pendingConfig);
              }}
            >
              <Trans>Replace</Trans>
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </motion.div>
  );
}
