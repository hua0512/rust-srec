import { Control } from 'react-hook-form';
import { SettingsCard } from '../settings-card';
import { FormField, FormItem, FormMessage } from '@/components/ui/form';
import { Workflow, Plus } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { StepLibrary } from '@/components/pipeline/workflows/step-library';
import { Button } from '@/components/ui/button';
import { PipelineStep, DagStepDefinition } from '@/api/schemas';
import { WorkflowFlowEditor } from '@/components/pipeline/workflows/flow-editor/workflow-flow-editor';
import { useState, useEffect, useCallback } from 'react';

// Simple UUID generator using crypto API
const uuidv4 = () => crypto.randomUUID();

interface PipelineConfigCardProps {
  control: Control<any>;
}

export function PipelineConfigCard({ control }: PipelineConfigCardProps) {
  // Config state
  const [currentSteps, setCurrentSteps] = useState<DagStepDefinition[]>([]);
  // We need to know if we are initialized to avoid overwriting with empty array on first render if field value is delayed
  const [initialized, setInitialized] = useState(false);

  return (
    <div className="space-y-6">
      <FormField
        control={control}
        name="pipeline"
        render={({ field }) => {
          // Initialize state from field value
          useEffect(() => {
            if (initialized) return;

            let loadedSteps: DagStepDefinition[] = [];
            try {
              if (field.value) {
                const parsed = JSON.parse(field.value);

                // Strict DAG support only
                if (parsed && typeof parsed === 'object' && Array.isArray(parsed.steps)) {
                  loadedSteps = parsed.steps;
                } else if (Array.isArray(parsed)) {
                  // If user has old config, we could show an error or just clear it. 
                  // For "do not support legacy", we might just ignore it or treat as empty.
                  console.warn("Legacy pipeline config detected and ignored.");
                }
              }
            } catch (e) {
              console.error("Failed to parse pipeline config", e);
            }

            setCurrentSteps(loadedSteps);
            setInitialized(true);
          }, [field.value, initialized]);

          const updateSteps = useCallback((newSteps: DagStepDefinition[]) => {
            setCurrentSteps(newSteps);
            // Always save as DAG object
            const dagConfig = {
              id: "global_pipeline",
              steps: newSteps
            };
            field.onChange(JSON.stringify(dagConfig));
          }, [field]);

          const handleAddStep = (step: PipelineStep) => {
            // Create new node
            const newNode: DagStepDefinition = {
              id: uuidv4(),
              step: step,
              depends_on: []
            };
            // Add to graph
            updateSteps([...currentSteps, newNode]);
          };

          const handleEditStep = (id: string) => {
            // For now, no deep editing of step config in global view implemented yet in this snippet
            // Ideally we open a dialog here.
            // Given the scope, we might rely on node double-click logic if WorkflowFlowEditor handles it, 
            // or just basic graph structure editing for now.
            // The WorkflowFlowEditor props suggest it supports onEditStep.
            console.log("Edit step", id);
          };

          return (
            <SettingsCard
              title={<Trans>Pipeline Configuration</Trans>}
              description={
                <Trans>
                  Default pipeline flow. Configure the sequence of processors
                  for new jobs. Support Sequential and Parallel (DAG) execution.
                </Trans>
              }
              icon={Workflow}
              iconColor="text-orange-500"
              iconBgColor="bg-orange-500/10"
              action={
                <StepLibrary
                  onAddStep={handleAddStep}
                  currentSteps={currentSteps.map((s) => {
                    // StepLibrary expects existing usages to check for validation/uniqueness?
                    // It mostly uses it for "already added" checks if unique is required.
                    // We can pass names.
                    if (typeof s.step === 'string') return s.step; // Should typically be object
                    const anyStep = s.step as any;
                    if (anyStep.type === 'inline') return anyStep.processor;
                    return anyStep.name || "";
                  })}
                  trigger={
                    <Button
                      type="button"
                      variant="secondary"
                      size="sm"
                      className="gap-2 bg-background/50 border-input border hover:bg-accent/50 transition-all shadow-sm"
                    >
                      <Plus className="h-4 w-4" />
                      <Trans>Add Step</Trans>
                    </Button>
                  }
                />
              }
            >
              <FormItem>
                <div className="space-y-4 h-[600px] border rounded-lg overflow-hidden bg-background/50">
                  {initialized ? (
                    <WorkflowFlowEditor
                      steps={currentSteps}
                      onUpdateSteps={updateSteps}
                      onEditStep={handleEditStep}
                    />
                  ) : (
                    <div className="flex items-center justify-center h-full text-muted-foreground">
                      <Trans>Loading pipeline editor...</Trans>
                    </div>
                  )}
                </div>
                <FormMessage />
              </FormItem>
            </SettingsCard>
          );
        }}
      />
    </div>
  );
}
