import { Control } from 'react-hook-form';
import { SettingsCard } from '../settings-card';
import { FormField, FormItem, FormMessage } from '@/components/ui/form';
import { Workflow } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { DagStepDefinition, DagPipelineDefinition } from '@/api/schemas';
import { useState, useEffect } from 'react';
import { useWatch } from 'react-hook-form';
import { PipelineWorkflowEditor } from '@/components/pipeline/workflows/pipeline-workflow-editor';

interface PipelineConfigCardProps {
  control: Control<any>;
}

export function PipelineConfigCard({ control }: PipelineConfigCardProps) {
  // Use useWatch to keep track of the field value from RHF
  const fieldValue = useWatch({
    control,
    name: 'pipeline',
  });

  // Config state
  const [currentSteps, setCurrentSteps] = useState<DagStepDefinition[]>([]);
  // We need to know if we are initialized to avoid overwriting with empty array on first render if field value is delayed
  const [initialized, setInitialized] = useState(false);

  // Initialize state from field value
  useEffect(() => {
    // fieldValue is now a DagPipelineDefinition object (parsed by GlobalConfigSchema), not a string

    // Only initialize once or if fieldValue changes significantly (e.g. after form reset)
    if (initialized) {
      // If initialized, we only want to update if the value from the form (server)
      // is different from our local state (meaning it was updated externally)
      if (fieldValue && typeof fieldValue === 'object') {
        const steps = (fieldValue?.steps || []) as DagStepDefinition[];
        // Simple equality check to avoid infinite loops if possible
        if (JSON.stringify(steps) !== JSON.stringify(currentSteps)) {
          setCurrentSteps(steps);
        }
      } else if (currentSteps.length > 0) {
        setCurrentSteps([]);
      }
      return;
    }

    let loadedSteps: DagStepDefinition[] = [];

    // Handle fieldValue as an object (from parsed schema) or as a string (legacy)
    if (fieldValue) {
      if (typeof fieldValue === 'object' && Array.isArray(fieldValue.steps)) {
        // New format: fieldValue is already a DagPipelineDefinition object
        loadedSteps = fieldValue.steps;
      } else if (typeof fieldValue === 'string') {
        // Legacy format: fieldValue is a JSON string (shouldn't happen after schema changes, but handle just in case)
        try {
          const parsed = JSON.parse(fieldValue);
          if (
            parsed &&
            typeof parsed === 'object' &&
            Array.isArray(parsed.steps)
          ) {
            loadedSteps = parsed.steps;
          }
        } catch (e) {
          console.error('Failed to parse pipeline config string', e);
        }
      }
    }

    setCurrentSteps(loadedSteps);
    setInitialized(true);
  }, [fieldValue, initialized, currentSteps]);

  return (
    <div className="space-y-6">
      <FormField
        control={control}
        name="pipeline"
        render={({ field }) => {
          const updateSteps = (newSteps: DagStepDefinition[]) => {
            setCurrentSteps(newSteps);
            // Pass as DAG object (will be stringified by GlobalConfigUpdateSchema when sending to backend)
            const dagConfig: DagPipelineDefinition = {
              name: 'global_pipeline',
              steps: newSteps,
            };
            field.onChange(dagConfig);
          };

          return (
            <SettingsCard
              title={<Trans>Pipeline Configuration</Trans>}
              description={
                <Trans>
                  Default pipeline flow. Configure the sequence of processors
                  for new jobs.
                </Trans>
              }
              icon={Workflow}
              iconColor="text-orange-500"
              iconBgColor="bg-orange-500/10"
            >
              <FormItem>
                {initialized ? (
                  <PipelineWorkflowEditor
                    steps={currentSteps}
                    onChange={updateSteps}
                  />
                ) : (
                  <div className="flex items-center justify-center min-h-[500px] text-muted-foreground bg-background/20 backdrop-blur-sm border-white/5 rounded-lg">
                    <Trans>Loading pipeline editor...</Trans>
                  </div>
                )}
                <FormMessage />
              </FormItem>
            </SettingsCard>
          );
        }}
      />
    </div>
  );
}
