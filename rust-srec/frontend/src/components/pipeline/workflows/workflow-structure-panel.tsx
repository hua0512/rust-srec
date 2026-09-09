import { useState } from 'react';
import { Layout, List, Share2 } from 'lucide-react';
import { Trans } from '@lingui/react/macro';

import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { StepLibrary } from './step-library';
import { StepsList } from './steps-list';
import { StepConfigDialog } from './step-config-dialog';
import { WorkflowFlowEditor } from './flow-editor/workflow-flow-editor';
import type { WorkflowStepsController } from './use-workflow-steps';

interface WorkflowStepLibraryProps {
  controller: WorkflowStepsController;
  trigger?: React.ReactNode;
}

/** Step library bound to the editing state, used both inline and from an "Add Step" trigger. */
export function WorkflowStepLibrary({
  controller,
  trigger,
}: WorkflowStepLibraryProps) {
  return (
    <StepLibrary
      onAddStep={controller.selectStep}
      currentSteps={controller.usedStepNames}
      open={controller.libraryOpen}
      onOpenChange={controller.setLibraryOpen}
      selectionMode={controller.selectionMode}
      trigger={trigger}
    />
  );
}

interface WorkflowStructurePanelProps {
  controller: WorkflowStepsController;
  /**
   * `standalone` is the full-page workflow editor; `embedded` is the denser panel shown inside a
   * configuration form, which also carries its own "Add Step" trigger.
   */
  variant: 'standalone' | 'embedded';
  className?: string;
  /** Rendered next to the view tabs, in the `embedded` variant only. */
  libraryTrigger?: React.ReactNode;
}

/** Steps of a workflow as a list or a graph, with the step library and configuration dialog. */
export function WorkflowStructurePanel({
  controller,
  variant,
  className,
  libraryTrigger,
}: WorkflowStructurePanelProps) {
  const [viewMode, setViewMode] = useState<'list' | 'graph'>('list');
  const embedded = variant === 'embedded';

  const stepsList = (
    <StepsList
      steps={controller.steps}
      onReorder={controller.reorder}
      onRemove={controller.removeStepAt}
      onUpdate={controller.updateStepAt}
      onEdit={controller.editStep}
      onReplace={controller.replaceStepAt}
    />
  );

  const flowEditor = (
    <WorkflowFlowEditor
      steps={controller.steps}
      onUpdateSteps={controller.reorder}
      onEditStep={controller.editStepById}
      onRemoveStep={controller.removeStepById}
      onReplaceStep={controller.replaceStepById}
    />
  );

  return (
    <div className={className}>
      <div
        className={
          embedded
            ? 'flex flex-col sm:flex-row sm:items-center justify-between gap-4 px-1 shrink-0'
            : 'flex items-center justify-between px-1 shrink-0'
        }
      >
        <div className="flex items-center gap-2">
          <div className="p-2 rounded-lg bg-primary/10">
            <Layout className="h-4 w-4 text-primary" />
          </div>
          <h3
            className={
              embedded
                ? 'text-sm font-semibold tracking-tight'
                : 'font-semibold tracking-tight'
            }
          >
            <Trans>Pipeline Structure</Trans>
          </h3>
        </div>
        <div
          className={
            embedded
              ? 'flex flex-wrap items-center gap-2 sm:gap-4'
              : 'flex items-center gap-4'
          }
        >
          <Tabs
            value={viewMode}
            onValueChange={(v) => setViewMode(v as 'list' | 'graph')}
            className={embedded ? 'h-8' : 'h-9'}
          >
            <TabsList
              className={
                embedded
                  ? 'grid w-full grid-cols-2 h-8 p-1'
                  : 'grid w-full grid-cols-2 h-9 p-1'
              }
            >
              <TabsTrigger
                value="list"
                className={embedded ? 'h-6 px-3' : 'h-7 px-4'}
              >
                {embedded ? (
                  <List className="h-3 w-3 sm:mr-2" />
                ) : (
                  <List className="h-3.5 w-3.5 mr-2" />
                )}
                <span className={embedded ? 'text-[10px]' : 'text-xs'}>
                  <Trans>List</Trans>
                </span>
              </TabsTrigger>
              <TabsTrigger
                value="graph"
                className={embedded ? 'h-6 px-3' : 'h-7 px-4'}
              >
                {embedded ? (
                  <Share2 className="h-3 w-3 sm:mr-2" />
                ) : (
                  <Share2 className="h-3.5 w-3.5 mr-2" />
                )}
                <span className={embedded ? 'text-[10px]' : 'text-xs'}>
                  <Trans>Graph</Trans>
                </span>
              </TabsTrigger>
            </TabsList>
          </Tabs>

          {embedded && (
            <WorkflowStepLibrary
              controller={controller}
              trigger={libraryTrigger}
            />
          )}
        </div>
      </div>

      {embedded ? (
        <div className="h-[400px] sm:h-[500px] border rounded-lg overflow-hidden bg-background/50 relative">
          {viewMode === 'list' ? (
            <div className="h-full min-h-0 overflow-y-auto overscroll-contain p-4">
              {stepsList}
            </div>
          ) : (
            flowEditor
          )}
        </div>
      ) : viewMode === 'list' ? (
        <div className="flex-1">{stepsList}</div>
      ) : (
        <div className="flex-1 border border-border/40 rounded-2xl overflow-hidden bg-muted/5 relative min-h-[500px]">
          {flowEditor}
        </div>
      )}

      <StepConfigDialog
        open={controller.editingStep !== null}
        onOpenChange={(open) => !open && controller.closeEditor()}
        dagStep={controller.editingStep}
        onSave={controller.saveEditedStep}
        allSteps={controller.steps}
        currentStepIndex={controller.editingIndex}
      />
    </div>
  );
}
