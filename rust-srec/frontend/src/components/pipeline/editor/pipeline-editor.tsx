import { Button } from '@/components/ui/button';
import { Plus } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { StepLibrary } from '@/components/pipeline/workflows/step-library';
import { StepsList } from '@/components/pipeline/workflows/steps-list';
import { PipelineStep } from '@/api/schemas';

interface PipelineEditorProps {
  steps: PipelineStep[];
  onChange: (steps: PipelineStep[]) => void;
  readonly?: boolean;
  hideAddButton?: boolean;
}

export function PipelineEditor({
  steps,
  onChange,
  readonly = false,
  hideAddButton = false,
}: PipelineEditorProps) {
  const handleAddStep = (step: PipelineStep) => {
    onChange([...steps, step]);
  };

  const handleRemoveStep = (index: number) => {
    onChange(steps.filter((_, i) => i !== index));
  };

  const handleReorder = (newOrder: PipelineStep[]) => {
    onChange(newOrder);
  };

  const handleUpdateStep = (index: number, newStep: PipelineStep) => {
    const updatedSteps = [...steps];
    updatedSteps[index] = newStep;
    onChange(updatedSteps);
  };

  // Extract step names for the step library (to prevent duplicate additions)
  const currentStepNames = steps.map((s) =>
    s.type === 'inline' ? s.processor : s.name,
  );

  return (
    <div className="space-y-4">
      {!hideAddButton && (
        <div className="flex justify-end">
          {!readonly && (
            <StepLibrary
              onAddStep={handleAddStep}
              currentSteps={currentStepNames}
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
          )}
        </div>
      )}

      <StepsList
        steps={steps}
        onReorder={handleReorder}
        onRemove={readonly ? undefined : handleRemoveStep}
        onUpdate={readonly ? undefined : handleUpdateStep}
      />
    </div>
  );
}
