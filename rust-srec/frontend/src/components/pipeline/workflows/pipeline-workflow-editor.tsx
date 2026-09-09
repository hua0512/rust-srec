import { memo } from 'react';
import { Plus } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { Button } from '@/components/ui/button';
import { DagStepDefinition } from '@/api/schemas';
import { useWorkflowSteps } from '@/components/pipeline/workflows/use-workflow-steps';
import { WorkflowStructurePanel } from '@/components/pipeline/workflows/workflow-structure-panel';

interface PipelineWorkflowEditorProps {
  steps: DagStepDefinition[];
  onChange: (steps: DagStepDefinition[]) => void;
}

export const PipelineWorkflowEditor = memo(
  ({ steps, onChange }: PipelineWorkflowEditorProps) => {
    const controller = useWorkflowSteps({ steps, onChange });

    return (
      <WorkflowStructurePanel
        controller={controller}
        variant="embedded"
        className="flex flex-col space-y-4"
        libraryTrigger={
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
    );
  },
);

PipelineWorkflowEditor.displayName = 'PipelineWorkflowEditor';
