import { Button } from '@/components/ui/button';
import { Form } from '@/components/ui/form';
import { ScrollArea } from '@/components/ui/scroll-area';
import { zodResolver } from '@hookform/resolvers/zod';
import { Trans } from '@lingui/macro';
import { useLingui } from '@lingui/react';
import { useQuery } from '@tanstack/react-query';
import { Loader2, Unlink, X } from 'lucide-react';
import { useEffect, useMemo, useState } from 'react';
import { useForm } from 'react-hook-form';
import { z } from 'zod';
import { PipelineStep } from '@/api/schemas';
import { getProcessorDefinition } from '@/components/pipeline/presets/processors/registry';
import { listJobPresets } from '@/server/functions/job';
import { createPortal } from 'react-dom';
import { motion, AnimatePresence } from 'motion/react';

interface StepConfigDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  step: PipelineStep | null;
  onSave: (config: any) => void;
}

export function StepConfigDialog({
  open,
  onOpenChange,
  step,
  onSave,
}: StepConfigDialogProps) {
  const { i18n } = useLingui();
  // 0. Handle Step Types
  const isPreset = step?.type === 'preset';
  const presetName = isPreset ? step.name : null;
  const [isDetached, setIsDetached] = useState(false);

  // Reset detached state when dialog closes
  useEffect(() => {
    if (!open) {
      setIsDetached(false);
    }
  }, [open]);

  // Fetch preset details if it's a string step to allow detaching
  const { data: presetData, isLoading: isLoadingPreset } = useQuery({
    queryKey: ['job', 'presets', 'detail', presetName],
    queryFn: () =>
      listJobPresets({ data: { search: presetName || undefined, limit: 1 } }),
    enabled: isPreset && !!presetName && open,
  });

  const presetDetail = useMemo(() => {
    if (!presetData || !presetName) return null;
    // Find exact match by name
    return (
      presetData.presets.find((p) => p.name === presetName) ||
      presetData.presets[0]
    );
  }, [presetData, presetName]);

  // 1. Determine processor definition
  const processorDef = useMemo(() => {
    if (!step) return null;

    // If it's a preset, we only have a processor definition if we've detached it locally
    if (isPreset) {
      if (isDetached && presetDetail) {
        return getProcessorDefinition(presetDetail.processor);
      }
      return null;
    }

    // For inline steps, use the processor directly
    if (step.type === 'inline') {
      return getProcessorDefinition(step.processor);
    }

    return null;
  }, [step, isPreset, isDetached, presetDetail]);

  // 2. Create form schema (dynamically)
  const formSchema = useMemo(() => {
    return processorDef?.schema || z.any();
  }, [processorDef]);

  // 3. Initialize form
  const form = useForm({
    resolver: zodResolver(formSchema as any),
    defaultValues: {},
  });

  // 4. Reset form when step changes
  useEffect(() => {
    if (open && step && step.type === 'inline') {
      form.reset(step.config || {});
    } else if (open && !isDetached) {
      // Only reset if NOT detached (if detached, we want to keep current edits or initial preset value)
      form.reset({});
    }
  }, [open, step, form, isDetached]);

  // Handle Escape key
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && open) {
        onOpenChange(false);
      }
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [open, onOpenChange]);

  const handleSubmit = (data: any) => {
    if (isPreset && isDetached && presetDetail) {
      // We are saving a detached preset.
      // We need to pass the full object so StepsList knows the processor type.
      onSave({
        type: 'inline',
        processor: presetDetail.processor,
        config: data,
      });
    } else {
      // Normal update
      onSave(data);
    }
    onOpenChange(false);
  };

  // Determine processor definition for the preset (for display purposes)
  const presetProcessorDef = useMemo(() => {
    if (!presetDetail) return null;
    return getProcessorDefinition(presetDetail.processor);
  }, [presetDetail]);

  // Form for displaying the preset config (read-only)
  const presetForm = useForm({
    values:
      typeof presetDetail?.config === 'string'
        ? JSON.parse(presetDetail.config)
        : presetDetail?.config || {},
  });

  const handleDetach = () => {
    if (!presetDetail) return;

    // Copy current preset values to the main form
    // We use presetForm.getValues() because it already handles parsing logic
    form.reset(presetForm.getValues());

    // Switch to "Edit" mode locally
    setIsDetached(true);
  };

  if (!document.body) return null;

  return createPortal(
    <AnimatePresence>
      {open && step && (
        <div className="fixed inset-0 z-[100] flex items-center justify-center p-4 sm:p-6">
          {/* Backdrop */}
          <motion.div
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            onClick={() => onOpenChange(false)}
            className="absolute inset-0 bg-background/60 backdrop-blur-sm"
          />

          {/* Modal Container */}
          <motion.div
            initial={{ opacity: 0, scale: 0.95, y: 10 }}
            animate={{ opacity: 1, scale: 1, y: 0 }}
            exit={{ opacity: 0, scale: 0.95, y: 10 }}
            className="relative w-full max-w-4xl max-h-[90vh] flex flex-col bg-card/95 backdrop-blur-xl border border-border/50 shadow-2xl rounded-2xl overflow-hidden"
          >
            {/* Custom Header */}
            <div className="flex items-center justify-between p-6 pb-4 border-b border-border/40 shrink-0">
              <div className="space-y-1">
                <h2 className="text-lg font-semibold tracking-tight">
                  {isPreset && !isDetached ? (
                    <Trans>Preset Step</Trans>
                  ) : (
                    <span className="flex items-center gap-2">
                      <span className="text-muted-foreground font-normal">
                        <Trans>Configure</Trans>
                      </span>
                      <span>
                        {(processorDef && i18n._(processorDef.label)) ||
                          (step?.type === 'inline'
                            ? step.processor
                            : presetDetail?.processor)}
                      </span>
                    </span>
                  )}
                </h2>
                <p className="text-sm text-muted-foreground">
                  {isPreset && !isDetached ? (
                    <Trans>
                      This step is linked to the preset{' '}
                      <strong className="text-foreground">{presetName}</strong>.
                    </Trans>
                  ) : (
                    <Trans>
                      Modify the parameters for this processing step.
                    </Trans>
                  )}
                </p>
              </div>
              <Button
                variant="ghost"
                size="icon"
                className="h-8 w-8 rounded-full"
                onClick={() => onOpenChange(false)}
              >
                <X className="h-4 w-4" />
              </Button>
            </div>

            {/* Content Body */}
            <div className="flex-1 min-h-0 relative flex flex-col">
              {isPreset && !isDetached ? (
                // PRESET VIEW
                <>
                  {isLoadingPreset && (
                    <div className="flex items-center justify-center flex-1">
                      <Loader2 className="h-8 w-8 animate-spin text-muted-foreground" />
                    </div>
                  )}
                  {!isLoadingPreset && presetDetail && (
                    <ScrollArea className="flex-1 bg-muted/5">
                      <div className="p-6">
                        {presetProcessorDef?.component ? (
                          <div className="space-y-6 [&_input]:pointer-events-none [&_textarea]:pointer-events-none [&_button:not([role=tab])]:pointer-events-none [&_input]:opacity-70 [&_textarea]:opacity-70 [&_button:not([role=tab])]:opacity-70 grayscale-[0.1]">
                            <div className="flex items-center gap-2 mb-4">
                              <div className="h-8 w-1 rounded bg-primary/20" />
                              <h3 className="font-semibold text-lg tracking-tight">
                                {presetProcessorDef &&
                                  i18n._(presetProcessorDef.label)}{' '}
                                <Trans>Configuration</Trans>
                              </h3>
                            </div>
                            <div className="rounded-xl border bg-card p-6 shadow-sm">
                              <Form {...presetForm}>
                                <presetProcessorDef.component
                                  control={presetForm.control}
                                />
                              </Form>
                            </div>
                          </div>
                        ) : (
                          <div className="rounded-lg border bg-card p-4 font-mono text-sm overflow-x-auto">
                            <pre>
                              {JSON.stringify(
                                typeof presetDetail.config === 'string'
                                  ? JSON.parse(presetDetail.config)
                                  : presetDetail.config,
                                null,
                                2,
                              )}
                            </pre>
                          </div>
                        )}
                      </div>
                    </ScrollArea>
                  )}
                  {!isLoadingPreset && !presetDetail && (
                    <div className="flex-1 flex items-center justify-center text-destructive text-sm p-6">
                      <Trans>Error: Could not load preset details.</Trans>
                    </div>
                  )}
                </>
              ) : (
                // EDIT VIEW
                <>
                  {processorDef ? (
                    <Form {...form}>
                      <form
                        onSubmit={form.handleSubmit(handleSubmit)}
                        className="contents"
                      >
                        <ScrollArea className="flex-1 px-6 bg-background/50">
                          <div className="py-6">
                            <processorDef.component control={form.control} />
                          </div>
                        </ScrollArea>
                      </form>
                    </Form>
                  ) : (
                    <div className="flex-1 flex items-center justify-center text-muted-foreground p-6">
                      <Trans>
                        No configuration form available for this processor.
                      </Trans>
                    </div>
                  )}
                </>
              )}
            </div>

            {/* Footer Actions */}
            <div className="p-6 pt-4 border-t bg-background/50 backdrop-blur shrink-0 flex justify-between items-center z-10">
              <Button
                type="button"
                variant="ghost"
                onClick={() => onOpenChange(false)}
              >
                {isPreset && !isDetached ? (
                  <Trans>Close</Trans>
                ) : (
                  <Trans>Cancel</Trans>
                )}
              </Button>

              {isPreset && !isDetached ? (
                <Button
                  type="button"
                  variant="default"
                  onClick={handleDetach}
                  disabled={isLoadingPreset || !presetDetail}
                  className="gap-2"
                >
                  <Unlink className="h-4 w-4" />
                  <Trans>Detach & Edit</Trans>
                </Button>
              ) : (
                <Button type="submit" onClick={form.handleSubmit(handleSubmit)}>
                  <Trans>Save Changes</Trans>
                </Button>
              )}
            </div>
          </motion.div>
        </div>
      )}
    </AnimatePresence>,
    document.body,
  );
}
