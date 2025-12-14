import { Control } from 'react-hook-form';
import { SettingsCard } from '../settings-card';
import { FormField, FormItem, FormMessage } from '@/components/ui/form';
import { Workflow, Plus } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { PipelineEditor } from '@/components/pipeline/editor/pipeline-editor';
import { StepLibrary } from '@/components/pipeline/workflows/step-library';
import { Button } from '@/components/ui/button';

interface PipelineConfigCardProps {
  control: Control<any>;
}

export function PipelineConfigCard({ control }: PipelineConfigCardProps) {
  return (
    <div className="space-y-6">
      <FormField
        control={control}
        name="pipeline"
        render={({ field }) => {
          // Parse JSON string to array safely
          let steps: any[] = [];
          try {
            if (field.value) {
              steps = JSON.parse(field.value);
              if (!Array.isArray(steps)) steps = [];
            }
          } catch (_) {
            // If invalid JSON, showing empty steps or handling error might be needed.
          }

          const updateSteps = (newSteps: any[]) => {
            field.onChange(JSON.stringify(newSteps));
          };

          const handleAddStep = (name: string) => {
            updateSteps([...steps, name]);
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
              action={
                <StepLibrary
                  onAddStep={handleAddStep}
                  currentSteps={steps.map((s) =>
                    typeof s === 'string' ? s : s.processor,
                  )}
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
                <div className="space-y-4">
                  <PipelineEditor
                    steps={steps}
                    onChange={updateSteps}
                    hideAddButton={true}
                  />
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
