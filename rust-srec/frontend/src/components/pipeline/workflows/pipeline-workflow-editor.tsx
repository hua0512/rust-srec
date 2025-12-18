import { useState } from 'react';
import { Plus, Layout, List, Share2 } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { StepLibrary } from '@/components/pipeline/workflows/step-library';
import { StepsList } from '@/components/pipeline/workflows/steps-list';
import { StepConfigDialog } from '@/components/pipeline/workflows/step-config-dialog';
import { Button } from '@/components/ui/button';
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { PipelineStep, DagStepDefinition } from '@/api/schemas';
import { WorkflowFlowEditor } from '@/components/pipeline/workflows/flow-editor/workflow-flow-editor';

interface PipelineWorkflowEditorProps {
  steps: DagStepDefinition[];
  onChange: (steps: DagStepDefinition[]) => void;
}

export function PipelineWorkflowEditor({
  steps,
  onChange,
}: PipelineWorkflowEditorProps) {
  // Debug log to trace steps in editor
  console.log('PipelineWorkflowEditor render:', {
    stepsCount: steps.length,
    steps,
  });

  const [viewMode, setViewMode] = useState<'list' | 'graph'>('list');
  const [editingIndex, setEditingIndex] = useState<number | null>(null);

  const handleAddStep = (step: PipelineStep) => {
    const stepName = step.type === 'inline' ? step.processor : step.name;
    const newNode: DagStepDefinition = {
      id: `${stepName}-${steps.length}`,
      step: step,
      depends_on: steps.length > 0 ? [steps[steps.length - 1].id] : [],
    };
    onChange([...steps, newNode]);
  };

  const handleUpdateStep = (index: number, newStep: DagStepDefinition) => {
    const updatedSteps = [...steps];
    updatedSteps[index] = newStep;
    onChange(updatedSteps);
  };

  const handleEditStepById = (id: string) => {
    const index = steps.findIndex((s) => s.id === id);
    if (index !== -1) {
      setEditingIndex(index);
    }
  };

  const handleRemoveStep = (index: number) => {
    const updatedSteps = steps.filter((_, i) => i !== index);
    onChange(updatedSteps);
  };

  return (
    <div className="flex flex-col space-y-4">
      <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-4 px-1 shrink-0">
        <div className="flex items-center gap-2">
          <div className="p-2 rounded-lg bg-primary/10">
            <Layout className="h-4 w-4 text-primary" />
          </div>
          <h3 className="text-sm font-semibold tracking-tight">
            <Trans>Pipeline Structure</Trans>
          </h3>
        </div>
        <div className="flex flex-wrap items-center gap-2 sm:gap-4">
          <Tabs
            value={viewMode}
            onValueChange={(v) => setViewMode(v as any)}
            className="h-8"
          >
            <TabsList className="grid w-full grid-cols-2 h-8 p-1">
              <TabsTrigger value="list" className="h-6 px-3">
                <List className="h-3 w-3 sm:mr-2" />
                <span className="text-[10px] hidden sm:inline">
                  <Trans>List</Trans>
                </span>
                <span className="text-[10px] sm:hidden">
                  <Trans>List</Trans>
                </span>
              </TabsTrigger>
              <TabsTrigger value="graph" className="h-6 px-3">
                <Share2 className="h-3 w-3 sm:mr-2" />
                <span className="text-[10px] hidden sm:inline">
                  <Trans>Graph</Trans>
                </span>
                <span className="text-[10px] sm:hidden">
                  <Trans>Graph</Trans>
                </span>
              </TabsTrigger>
            </TabsList>
          </Tabs>

          <StepLibrary
            onAddStep={handleAddStep}
            currentSteps={steps.map((s) => {
              if (typeof s.step === 'string') return s.step;
              const anyStep = s.step as any;
              if (anyStep.type === 'inline') return anyStep.processor;
              return anyStep.name || '';
            })}
            trigger={
              <Button
                type="button"
                variant="secondary"
                size="sm"
                className="gap-2 bg-background/50 border-input border hover:bg-accent/50 transition-all shadow-sm flex-1 sm:flex-none justify-center whitespace-nowrap"
              >
                <Plus className="h-4 w-4" />
                <Trans>Add Step</Trans>
              </Button>
            }
          />
        </div>
      </div>

      <div className="h-[400px] sm:h-[500px] border rounded-lg overflow-hidden bg-background/50 relative">
        {viewMode === 'list' ? (
          <div className="p-4 h-full">
            <StepsList
              steps={steps}
              onReorder={onChange}
              onRemove={handleRemoveStep}
              onUpdate={handleUpdateStep}
              onEdit={setEditingIndex}
            />
          </div>
        ) : (
          <WorkflowFlowEditor
            steps={steps}
            onUpdateSteps={onChange}
            onEditStep={handleEditStepById}
          />
        )}
      </div>

      <StepConfigDialog
        open={editingIndex !== null}
        onOpenChange={(open) => !open && setEditingIndex(null)}
        dagStep={editingIndex !== null ? steps[editingIndex] : null}
        onSave={(data) => {
          if (editingIndex !== null) {
            handleUpdateStep(editingIndex, data);
            setEditingIndex(null);
          }
        }}
        allSteps={steps}
        currentStepIndex={editingIndex ?? -1}
      />
    </div>
  );
}
