import { motion, Reorder, AnimatePresence } from 'motion/react';
import {
  X,
  Workflow,
  GripVertical,
  Settings,
  ArrowRight,
} from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Badge } from '@/components/ui/badge';
import { Trans } from '@lingui/react/macro';
import { t } from '@lingui/core/macro';
import { getStepColor, getStepIcon } from '@/components/pipeline/constants';
import { listJobPresets } from '@/server/functions/job';
import { useQuery } from '@tanstack/react-query';
import { DagStepDefinition } from '@/api/schemas';



interface StepsListProps {
  steps: DagStepDefinition[];
  onReorder: (newOrder: DagStepDefinition[]) => void;
  onRemove?: (index: number) => void;
  onUpdate?: (index: number, newStep: DagStepDefinition) => void;
  onEdit?: (index: number) => void;
}

export function StepsList({
  steps,
  onReorder,
  onRemove,
  onUpdate,
  onEdit,
}: StepsListProps) {
  // Fetch available job presets to get metadata (icons, colors, desc)
  const { data: presetsData } = useQuery({
    queryKey: ['job', 'presets', 'all'],
    queryFn: () => listJobPresets({ data: { limit: 100 } }),
  });

  const presets = presetsData?.presets || [];



  return (
    <div className="rounded-2xl border border-dashed border-border/60 bg-muted/5 min-h-[120px] p-4 sm:p-6 h-full flex flex-col relative">
      {steps.length > 0 ? (
        <Reorder.Group
          axis="y"
          values={steps}
          onReorder={onReorder}
          className="space-y-3"
        >
          <AnimatePresence mode="popLayout">
            {steps.map((dagStep, index) => {
              const { step, id } = dagStep;
              const stepName =
                step.type === 'inline' ? step.processor : step.name;
              const presetInfo = presets.find(
                (p) => p.name === stepName || p.processor === stepName,
              );
              const Icon = getStepIcon(stepName);
              const isInline = step.type === 'inline';
              // Use different color style for inline detached steps to distinguish them
              let stepColorClass = presetInfo
                ? getStepColor(
                  presetInfo.processor,
                  presetInfo.category || undefined,
                )
                : 'from-muted/20 to-muted/10 text-muted-foreground border-border';

              if (isInline && !presetInfo) {
                // Fallback color for purely custom inline steps
                stepColorClass =
                  'from-primary/10 to-primary/5 text-primary border-primary/20';
              }

              return (
                <Reorder.Item
                  key={id || `step-${index}`}
                  value={dagStep}
                  className="relative group"
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
                    whileHover={{ scale: 1.005 }}
                    whileTap={{ scale: 0.995 }}
                    className={`
                      flex items-center gap-4 p-4 rounded-xl border bg-gradient-to-r ${stepColorClass}
                      shadow-sm hover:shadow-md transition-all cursor-grab active:cursor-grabbing
                    `}
                  >
                    <div className="flex items-center justify-center h-8 w-8 rounded-lg bg-background/40 backdrop-blur text-inherit shrink-0 border border-current border-opacity-20 shadow-sm font-mono text-sm font-bold opacity-80">
                      {index + 1}
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
                      <div className="flex items-center gap-1.5 min-w-0">
                        {id && (
                          <Badge
                            variant="outline"
                            className="text-[10px] h-4 px-1.5 font-mono opacity-60 bg-background/20"
                          >
                            {id}
                          </Badge>
                        )}
                        <span className="font-semibold tracking-tight truncate text-foreground/90 min-w-0">
                          {isInline ? (
                            <Trans>{stepName} (Custom)</Trans>
                          ) : (
                            stepName
                          )}
                        </span>
                      </div>
                      <div className="flex items-center gap-2 flex-wrap mt-0.5">
                        {presetInfo?.category && (
                          <Badge
                            variant="outline"
                            className="text-[9px] uppercase h-3.5 px-1 bg-background/30 backdrop-blur border-current border-opacity-20 text-inherit"
                          >
                            {presetInfo.category}
                          </Badge>
                        )}
                        {dagStep.depends_on && dagStep.depends_on.length > 0 && (
                          <div className="flex items-center gap-1 opacity-60">
                            <ArrowRight className="h-2.5 w-2.5" />
                            <span className="text-[9px] font-mono leading-none">
                              AFTER: {dagStep.depends_on.join(', ')}
                            </span>
                          </div>
                        )}
                      </div>
                    </div>

                    <div className="flex items-center opacity-0 group-hover:opacity-100 transition-opacity">
                      {onUpdate && (
                        <Button
                          type="button"
                          variant="ghost"
                          size="icon"
                          className="h-8 w-8 text-muted-foreground hover:text-foreground hover:bg-background/40 rounded-lg mr-1"
                          onClick={() => onEdit?.(index)}
                          title={t`Configure Step`}
                        >
                          <Settings className="h-4 w-4" />
                        </Button>
                      )}
                      {onRemove && (
                        <Button
                          type="button"
                          variant="ghost"
                          size="icon"
                          className="h-8 w-8 hover:bg-destructive/10 hover:text-destructive rounded-lg"
                          onClick={() => onRemove(index)}
                        >
                          <X className="h-4 w-4" />
                        </Button>
                      )}
                      <GripVertical className="h-4 w-4 opacity-20 group-hover:opacity-50 transition-opacity ml-1 cursor-grab" />
                    </div>
                  </motion.div>
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

    </div>
  );
}
