import { CardContent } from '@/components/ui/card';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from '@/components/ui/alert-dialog';
import { Edit, Trash, Workflow } from 'lucide-react';
import {
  CardHeaderRow,
  CardIconTile,
  CardMenuButton,
  CardMetaFooter,
  DecoratedCard,
} from '@/components/shared/decorated-card';
import { Trans } from '@lingui/react/macro';
import { t, plural } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import type { PipelinePreset } from '@/api/schemas';
import {
  getStepBadgeColor,
  getStepIcon,
} from '@/components/pipeline/constants';
import {
  getJobPresetName,
  getPipelinePresetDescription,
  getPipelinePresetName,
} from '../presets/default-presets-i18n';
import { getStepName } from './step-operations';

interface WorkflowCardProps {
  workflow: PipelinePreset;
  onEdit: (workflow: PipelinePreset) => void;
  onDelete: (id: string) => void;
}

export function WorkflowCard({
  workflow,
  onEdit,
  onDelete,
}: WorkflowCardProps) {
  const { i18n } = useLingui();
  const steps = workflow.dag.steps;
  const description = getPipelinePresetDescription(workflow, i18n);

  return (
    <DecoratedCard>
      <CardHeaderRow
        icon={
          <CardIconTile
            icon={Workflow}
            className="bg-primary/10 text-primary"
          />
        }
        title={getPipelinePresetName(workflow, i18n)}
        subtitle={t(
          i18n,
        )`${plural(steps.length, { one: '# step', other: '# steps' })}`}
        actions={
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <CardMenuButton />
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="w-48">
              <DropdownMenuItem onClick={() => onEdit(workflow)}>
                <Edit className="mr-2 h-4 w-4" /> <Trans>Edit</Trans>
              </DropdownMenuItem>
              <AlertDialog>
                <AlertDialogTrigger asChild>
                  <DropdownMenuItem
                    className="text-destructive focus:text-destructive"
                    onSelect={(e) => e.preventDefault()}
                  >
                    <Trash className="mr-2 h-4 w-4" /> <Trans>Delete</Trans>
                  </DropdownMenuItem>
                </AlertDialogTrigger>
                <AlertDialogContent>
                  <AlertDialogHeader>
                    <AlertDialogTitle>
                      <Trans>Delete Workflow?</Trans>
                    </AlertDialogTitle>
                    <AlertDialogDescription>
                      <Trans>
                        This will permanently delete the workflow "
                        {workflow.name}
                        ". Streamers using this workflow will need to be
                        updated.
                      </Trans>
                    </AlertDialogDescription>
                  </AlertDialogHeader>
                  <AlertDialogFooter>
                    <AlertDialogCancel>
                      <Trans>Cancel</Trans>
                    </AlertDialogCancel>
                    <AlertDialogAction
                      onClick={() => onDelete(workflow.id)}
                      className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
                    >
                      <Trans>Delete</Trans>
                    </AlertDialogAction>
                  </AlertDialogFooter>
                </AlertDialogContent>
              </AlertDialog>
            </DropdownMenuContent>
          </DropdownMenu>
        }
      />
      <CardContent className="relative pb-4 flex-1 z-10">
        {description && (
          <p className="text-xs text-muted-foreground/80 line-clamp-2 mb-4 leading-relaxed font-light">
            {description}
          </p>
        )}

        {/* Pipeline Steps Visualization */}
        <div className="flex items-center gap-2 flex-wrap">
          {steps.map((dagStep, index) => {
            const { step, id } = dagStep;
            const stepName = getStepName(step);
            const StepIcon = getStepIcon(stepName);
            const badgeColor = getStepBadgeColor(stepName);
            const isInline = step.type === 'inline';

            return (
              <div
                key={index}
                className={`flex items-center gap-1.5 px-2 py-1 rounded-lg border ${badgeColor} transition-all hover:scale-105 group/step`}
                title={
                  id
                    ? `${id}: ${stepName}`
                    : stepName + (isInline ? ' (Inline)' : '')
                }
              >
                <StepIcon className="h-3 w-3" />
                {id && (
                  <span className="text-[9px] font-mono opacity-50 mr-0.5 border-r border-current/20 pr-1 leading-none h-2.5 flex items-center">
                    {id}
                  </span>
                )}
                <span className="text-[10px] font-medium truncate max-w-[80px]">
                  {step.type === 'preset'
                    ? getJobPresetName({ id: step.name, name: step.name }, i18n)
                    : stepName}
                </span>
              </div>
            );
          })}
          {steps.length === 0 && (
            <span className="italic opacity-40 text-xs text-center w-full py-2">
              <Trans>No steps defined</Trans>
            </span>
          )}
        </div>
      </CardContent>
      <CardMetaFooter>
        <span className="font-mono opacity-50">#{workflow.id.slice(0, 8)}</span>
      </CardMetaFooter>
    </DecoratedCard>
  );
}
