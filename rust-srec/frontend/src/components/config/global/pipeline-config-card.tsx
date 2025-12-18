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
    // Only initialize once or if fieldValue changes significantly (e.g. after form reset)
    if (initialized) {
      // If initialized, we only want to update if the value from the form (server)
      // is different from our local state (meaning it was updated externally)
      if (fieldValue) {
        try {
          const parsed = JSON.parse(fieldValue);
          const steps = (parsed?.steps || []) as DagStepDefinition[];
          // Simple equality check to avoid infinite loops if possible
          if (JSON.stringify(steps) !== JSON.stringify(currentSteps)) {
            setCurrentSteps(steps);
          }
        } catch (e) {
          console.error('Failed to parse pipeline config', e);
          // ignore parse errors for existing state sync
        }
      } else if (currentSteps.length > 0) {
        setCurrentSteps([]);
      }
      return;
    }

    let loadedSteps: DagStepDefinition[] = [];
    try {
      if (fieldValue) {
        const parsed = JSON.parse(fieldValue);

        // Strict DAG support only
        if (
          parsed &&
          typeof parsed === 'object' &&
          Array.isArray(parsed.steps)
        ) {
          loadedSteps = parsed.steps;
        } else if (Array.isArray(parsed)) {
          // If user has old config, we could show an error or just clear it.
          console.warn('Legacy pipeline config detected and ignored.');
        }
      }
    } catch (e) {
      console.error('Failed to parse pipeline config', e);
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
            // Always save as DAG object
            const dagConfig: DagPipelineDefinition = {
              name: 'global_pipeline',
              steps: newSteps,
            };
            field.onChange(JSON.stringify(dagConfig));
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
