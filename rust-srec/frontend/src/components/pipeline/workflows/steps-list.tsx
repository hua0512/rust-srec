import { useState } from 'react';
import { motion, Reorder, AnimatePresence } from 'motion/react';
import {
  X,
  Workflow,
  GripVertical,
  Settings,
  Code,
  FileJson,
} from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Badge } from '@/components/ui/badge';
import { Trans } from '@lingui/react/macro';
import { t } from '@lingui/core/macro';
import { getStepColor, getStepIcon } from '@/components/pipeline/constants';
import { listJobPresets } from '@/server/functions/job';
import { useQuery } from '@tanstack/react-query';
import { PipelineStep } from '@/api/schemas';

import { StepConfigDialog } from './step-config-dialog';

interface StepsListProps {
  steps: PipelineStep[];
  onReorder: (newOrder: PipelineStep[]) => void;
  onRemove?: (index: number) => void;
  onUpdate?: (index: number, newStep: PipelineStep) => void;
}

export function StepsList({
  steps,
  onReorder,
  onRemove,
  onUpdate,
}: StepsListProps) {
  // Fetch available job presets to get metadata (icons, colors, desc)
  const { data: presetsData } = useQuery({
    queryKey: ['job', 'presets', 'all'],
    queryFn: () => listJobPresets({ data: { limit: 100 } }),
  });

  const presets = presetsData?.presets || [];

  const getPresetInfo = (step: PipelineStep) => {
    if (typeof step === 'string') {
      return presets.find((p) => p.name === step);
    }
    // If it's an object, we can try to find preset matching the processor name as fallback
    return presets.find((p) => p.processor === step.processor);
  };

  const [editingIndex, setEditingIndex] = useState<number | null>(null);

  const handleEditStart = (index: number) => {
    setEditingIndex(index);
    // config logic removed as it was unused
  };

  const handleConfigSave = (data: any) => {
    if (editingIndex === null || !onUpdate) return;
    const originalStep = steps[editingIndex];

    // Check if data is a full step object (has processor)
    // This comes from "Detach" action in StepConfigDialog
    if (
      typeof data === 'object' &&
      data !== null &&
      'processor' in data &&
      'config' in data
    ) {
      onUpdate(editingIndex, data as PipelineStep);
      return;
    }

    // Otherwise data is just the config object
    const config = data;

    let newStep: PipelineStep;
    if (typeof originalStep === 'string') {
      const preset = getPresetInfo(originalStep);
      newStep = {
        processor: preset?.processor || 'unknown',
        config: config,
      };
    } else {
      newStep = {
        ...originalStep,
        config: config,
      };
    }
    onUpdate(editingIndex, newStep);
  };

  return (
    <div className="rounded-2xl border border-dashed border-border/60 bg-muted/5 min-h-[120px] p-4 sm:p-6 h-full flex flex-col">
      {steps.length > 0 ? (
        <Reorder.Group
          axis="y"
          values={steps}
          onReorder={onReorder}
          className="space-y-3"
        >
          <AnimatePresence mode="popLayout">
            {steps.map((step, index) => {
              const isInline = typeof step !== 'string';
              const stepName = isInline ? step.processor : step;
              const presetInfo = getPresetInfo(step);
              const Icon = presetInfo
                ? getStepIcon(presetInfo.processor)
                : isInline
                  ? getStepIcon(step.processor)
                  : Workflow;

              // Use different color style for inline detached steps to distinguish them
              let colorClass = presetInfo
                ? getStepColor(
                    presetInfo.processor,
                    presetInfo.category || undefined,
                  )
                : 'from-muted/20 to-muted/10 text-muted-foreground border-border';

              if (isInline && !presetInfo) {
                // Fallback color for purely custom inline steps
                colorClass =
                  'from-primary/10 to-primary/5 text-primary border-primary/20';
              }

              return (
                <Reorder.Item
                  key={
                    typeof step === 'string'
                      ? `${step}-${index}`
                      : `${step.processor}-${index}`
                  }
                  value={step}
                  className="relative"
                >
                  <motion.div
                    layout
                    initial={{ opacity: 0, y: 10, scale: 0.98 }}
                    animate={{ opacity: 1, y: 0, scale: 1 }}
                    exit={{
                      opacity: 0,
                      scale: 0.9,
                      transition: { duration: 0.2 },
                    }}
                    whileHover={{ scale: 1.01 }}
                    whileTap={{ scale: 0.99 }}
                    className={`
                                            group flex items-center gap-4 p-4 rounded-xl border bg-gradient-to-r ${colorClass}
                                            shadow-sm hover:shadow-md transition-all cursor-grab active:cursor-grabbing
                                        `}
                  >
                    <div className="flex items-center justify-center h-8 w-8 rounded-lg bg-background/40 backdrop-blur text-inherit shrink-0 border border-current border-opacity-20 shadow-sm">
                      <span className="text-sm font-bold font-mono opacity-80">
                        {index + 1}
                      </span>
                    </div>

                    <div className="h-10 w-10 rounded-xl bg-background/40 backdrop-blur flex items-center justify-center shrink-0 border border-current border-opacity-20 shadow-sm relative">
                      <Icon className="h-5 w-5 opacity-90" />
                      {isInline && (
                        <div
                          className="absolute -top-1 -right-1 h-4 w-4 rounded-full bg-background border border-current flex items-center justify-center"
                          title={t`Custom Config`}
                        >
                          <Settings className="h-2.5 w-2.5" />
                        </div>
                      )}
                    </div>

                    <div className="flex-1 min-w-0 flex flex-col justify-center">
                      <div className="flex items-center gap-2">
                        <span className="font-semibold tracking-tight truncate text-foreground/90">
                          {isInline ? (
                            <Trans>{stepName} (Custom)</Trans>
                          ) : (
                            stepName
                          )}
                        </span>
                        {presetInfo?.category && (
                          <Badge
                            variant="outline"
                            className="text-[10px] uppercase h-4 px-1.5 bg-background/30 backdrop-blur border-current border-opacity-20 text-inherit"
                          >
                            {presetInfo.category}
                          </Badge>
                        )}
                        {isInline && (
                          <Badge
                            variant="outline"
                            className="text-[10px] uppercase h-4 px-1.5 bg-background/30 backdrop-blur border-current border-opacity-20 text-inherit flex gap-1"
                          >
                            <Code className="h-3 w-3" />
                            <Trans>Config</Trans>
                          </Badge>
                        )}
                      </div>
                      {presetInfo?.description && !isInline && (
                        <span className="text-xs text-muted-foreground/80 truncate opacity-90 max-w-[80%]">
                          {presetInfo.description}
                        </span>
                      )}
                    </div>

                    <div className="flex items-center opacity-0 group-hover:opacity-100 transition-opacity">
                      {onUpdate && (
                        <Button
                          type="button"
                          variant="ghost"
                          size="icon"
                          className="h-9 w-9 text-muted-foreground hover:text-foreground hover:bg-background/40 rounded-xl mr-1"
                          onClick={() => handleEditStart(index)}
                          title={
                            isInline ? t`Edit Configuration` : t`Customize Step`
                          }
                        >
                          {isInline ? (
                            <FileJson className="h-4 w-4" />
                          ) : (
                            <Settings className="h-4 w-4" />
                          )}
                        </Button>
                      )}
                      {onRemove && (
                        <Button
                          type="button"
                          variant="ghost"
                          size="icon"
                          className="h-9 w-9 hover:bg-destructive/10 hover:text-destructive rounded-xl"
                          onClick={() => onRemove(index)}
                        >
                          <X className="h-4 w-4" />
                        </Button>
                      )}
                      <GripVertical className="h-5 w-5 opacity-20 group-hover:opacity-50 transition-opacity ml-2 cursor-grab" />
                    </div>
                  </motion.div>

                  {/* Connector Line */}
                  {index < steps.length - 1 && (
                    <div className="absolute left-8 top-full h-3 w-px bg-border/60 -ml-px z-0" />
                  )}
                </Reorder.Item>
              );
            })}
          </AnimatePresence>
        </Reorder.Group>
      ) : (
        <div className="flex-1 min-h-[150px] flex flex-col items-center justify-center text-center space-y-4">
          <div className="p-4 rounded-full bg-muted/30 border border-dashed border-border mb-2 animate-pulse">
            <Workflow className="h-8 w-8 text-muted-foreground/40" />
          </div>
          <div className="space-y-1 max-w-sm">
            <h3 className="font-medium text-foreground">
              <Trans>No Steps Added</Trans>
            </h3>
            <p className="text-sm text-muted-foreground">
              <Trans>Pipeline is empty.</Trans>
            </p>
          </div>
        </div>
      )}

      {/* Configuration Dialog */}
      <StepConfigDialog
        open={editingIndex !== null}
        onOpenChange={(open) => !open && setEditingIndex(null)}
        step={editingIndex !== null ? steps[editingIndex] : null}
        onSave={handleConfigSave}
      />
    </div>
  );
}
