import { Button } from '@/components/ui/button';
import { Form } from '@/components/ui/form';
import { ScrollArea } from '@/components/ui/scroll-area';
import { zodResolver } from '@hookform/resolvers/zod';
import { Trans, t } from '@lingui/macro';
import { useLingui } from '@lingui/react';
import { useQuery } from '@tanstack/react-query';
import { Loader2, Unlink, X } from 'lucide-react';
import { useEffect, useMemo, useState } from 'react';
import { useForm } from 'react-hook-form';
import { z } from 'zod';
import { DagStepDefinition, PipelineStep } from '@/api/schemas';
import { getProcessorDefinition } from '@/components/pipeline/presets/processors/registry';
import { listJobPresets } from '@/server/functions/job';
import { createPortal } from 'react-dom';
import { motion, AnimatePresence } from 'motion/react';
import { Checkbox } from '@/components/ui/checkbox';
import { Input as UiInput } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';

interface StepConfigDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  dagStep: DagStepDefinition | null;
  onSave: (dagStep: DagStepDefinition) => void;
  allSteps?: DagStepDefinition[];
  currentStepIndex?: number;
}

export function StepConfigDialog({
  open,
  onOpenChange,
  dagStep,
  onSave,
  allSteps = [],
  currentStepIndex = -1,
}: StepConfigDialogProps) {
  const { i18n } = useLingui();
  const step = dagStep?.step || null;
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
    if (!dagStep) return;

    const finalStepContent: PipelineStep =
      isPreset && isDetached && presetDetail
        ? {
          type: 'inline',
          processor: presetDetail.processor,
          config: data,
        }
        : ({
          ...step!,
          config: step?.type === 'inline' ? data : (step as any)?.config,
        } as PipelineStep);

    onSave({
      id: idValue,
      depends_on: dependsOn,
      step: finalStepContent,
    });
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

  const [idValue, setIdValue] = useState(dagStep?.id || '');
  const [dependsOn, setDependsOn] = useState<string[]>(dagStep?.depends_on || []);

  useEffect(() => {
    if (open && dagStep) {
      setIdValue(dagStep.id || '');
      setDependsOn(dagStep.depends_on || []);
    }
  }, [open, dagStep]);

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
            <Tabs defaultValue="config" className="flex-1 min-h-0 flex flex-col overflow-hidden">
              <div className="px-6 pt-6 pb-2 shrink-0">
                <TabsList className="w-full justify-start h-auto p-1 bg-muted/50 rounded-full gap-1">
                  <TabsTrigger
                    value="config"
                    className="rounded-full px-4 py-2 flex-1 data-[state=active]:bg-background data-[state=active]:text-foreground data-[state=active]:shadow-sm transition-all"
                  >
                    <Trans>Step Configuration</Trans>
                  </TabsTrigger>
                  <TabsTrigger
                    value="flow"
                    className="rounded-full px-4 py-2 flex-1 data-[state=active]:bg-background data-[state=active]:text-foreground data-[state=active]:shadow-sm transition-all"
                  >
                    <Trans>Flow & Dependencies</Trans>
                  </TabsTrigger>
                </TabsList>
              </div>

              {/* TAB 1: CONFIGURATION */}
              <TabsContent value="config" className="flex-1 min-h-0 mt-0 overflow-hidden flex flex-col data-[state=inactive]:hidden">
                <ScrollArea className="flex-1 h-full bg-background/50">
                  {isPreset && !isDetached ? (
                    // PRESET VIEW
                    <div className="p-6">
                      {isLoadingPreset && (
                        <div className="flex items-center justify-center p-8 text-muted-foreground">
                          <Loader2 className="h-6 w-6 animate-spin mr-2" />
                          <Trans>Loading preset details...</Trans>
                        </div>
                      )}
                      {presetDetail && (
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
                              <form className="contents">
                                {(() => {
                                  const Def = getProcessorDefinition(
                                    presetDetail.processor,
                                  );
                                  return Def ? (
                                    <Def.component control={presetForm.control} />
                                  ) : null;
                                })()}
                              </form>
                            </Form>
                          </div>
                        </div>
                      )}
                      {!isLoadingPreset && !presetDetail && (
                        <div className="flex items-center justify-center text-destructive text-sm p-6">
                          <Trans>Error: Could not load preset details.</Trans>
                        </div>
                      )}
                    </div>
                  ) : (
                    // EDIT VIEW
                    <div className="p-6">
                      {processorDef ? (
                        <Form {...form}>
                          <form
                            onSubmit={form.handleSubmit(handleSubmit)}
                            className="contents"
                          >
                            <processorDef.component control={form.control} />
                          </form>
                        </Form>
                      ) : (
                        <div className="flex items-center justify-center text-muted-foreground p-6">
                          <Trans>
                            No configuration form available for this processor.
                          </Trans>
                        </div>
                      )}
                    </div>
                  )}
                </ScrollArea>
              </TabsContent>

              {/* TAB 2: FLOW & DEPENDENCIES */}
              <TabsContent value="flow" className="flex-1 min-h-0 mt-0 overflow-hidden flex flex-col data-[state=inactive]:hidden">
                <ScrollArea className="flex-1 h-full bg-background/50">
                  <div className="p-6 max-w-2xl mx-auto space-y-6">
                    <div className="flex items-center gap-2 mb-6">
                      <div className="h-8 w-1 rounded bg-blue-500/50" />
                      <div>
                        <h3 className="font-semibold text-lg tracking-tight">
                          <Trans>DAG Configuration</Trans>
                        </h3>
                        <p className="text-sm text-muted-foreground">
                          <Trans>Define how this step identifies itself and connects to others.</Trans>
                        </p>
                      </div>
                    </div>

                    <div className="space-y-6">
                      <div className="space-y-3">
                        <Label htmlFor="step-id" className="text-sm font-medium">
                          <Trans>Step Identifier (Unique)</Trans>
                        </Label>
                        <div className="relative">
                          <div className="absolute inset-y-0 left-0 pl-3 flex items-center pointer-events-none">
                            <span className="text-muted-foreground text-xs font-mono">#</span>
                          </div>
                          <UiInput
                            id="step-id"
                            value={idValue}
                            onChange={(e) => setIdValue(e.target.value)}
                            placeholder={t`e.g., process-video`}
                            className="pl-7 bg-background/50 font-mono text-sm"
                          />
                        </div>
                        <p className="text-[10px] text-muted-foreground">
                          <Trans>A unique ID used by other steps to reference this one.</Trans>
                        </p>
                      </div>

                      <div className="space-y-3">
                        <Label className="text-sm font-medium">
                          <Trans>Depends On (Ancestors)</Trans>
                        </Label>
                        <div className="border border-border/40 rounded-lg bg-background/50 overflow-hidden">
                          <div className="p-1 max-h-[300px] overflow-y-auto">
                            {allSteps.length > 1 ? (
                              <div className="space-y-1 p-2">
                                {allSteps
                                  .filter((_, idx) => idx !== currentStepIndex) // Cannot depend on self
                                  .map((otherStep) => {
                                    const otherId = otherStep.id;
                                    const isDep = dependsOn.includes(otherId);

                                    return (
                                      <div
                                        key={otherId}
                                        className={`flex items-center space-x-3 p-3 rounded-md transition-colors ${isDep ? 'bg-primary/10 border border-primary/20' : 'hover:bg-muted/50 border border-transparent'
                                          }`}
                                      >
                                        <Checkbox
                                          id={`dep-${otherId}`}
                                          checked={isDep}
                                          onCheckedChange={(checked) => {
                                            if (checked) {
                                              setDependsOn([...dependsOn, otherId]);
                                            } else {
                                              setDependsOn(
                                                dependsOn.filter((d) => d !== otherId),
                                              );
                                            }
                                          }}
                                        />
                                        <label
                                          htmlFor={`dep-${otherId}`}
                                          className="flex-1 cursor-pointer flex items-center justify-between"
                                        >
                                          <span className="font-mono text-xs font-semibold">
                                            {otherId}
                                          </span>
                                          <span className="text-xs text-muted-foreground bg-muted px-2 py-0.5 rounded-full">
                                            {otherStep.step.type === 'inline'
                                              ? otherStep.step.processor
                                              : otherStep.step.name}
                                          </span>
                                        </label>
                                      </div>
                                    );
                                  })}
                              </div>
                            ) : (
                              <div className="p-8 text-center text-muted-foreground text-sm italic">
                                <Trans>No other steps available to depend on.</Trans>
                              </div>
                            )}
                          </div>
                        </div>
                        <p className="text-[10px] text-muted-foreground">
                          <Trans>Select the steps that must complete successfully before this step runs.</Trans>
                        </p>
                      </div>
                    </div>
                  </div>
                </ScrollArea>
              </TabsContent>

            </Tabs>

            {/* Footer Actions */}
            <div className="p-6 pt-4 border-t bg-background/50 backdrop-blur shrink-0 flex justify-between items-center z-10">
              <Button
                type="button"
                variant="ghost"
                onClick={() => onOpenChange(false)}
              >
                <Trans>Cancel</Trans>
              </Button>

              <div className="flex items-center gap-2">
                {isPreset && !isDetached && (
                  <Button
                    type="button"
                    variant="outline"
                    onClick={handleDetach}
                    disabled={isLoadingPreset || !presetDetail}
                    className="gap-2"
                  >
                    <Unlink className="h-4 w-4" />
                    <Trans>Detach & Edit</Trans>
                  </Button>
                )}
                <Button type="submit" onClick={form.handleSubmit(handleSubmit)}>
                  <Trans>Save Changes</Trans>
                </Button>
              </div>
            </div>
          </motion.div>
        </div>
      )}
    </AnimatePresence>,
    document.body,
  );
}
