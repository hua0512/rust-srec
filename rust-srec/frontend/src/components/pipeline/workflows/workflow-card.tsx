import { Button } from '@/components/ui/button';
import {
  Card,
  CardContent,
  CardFooter,
  CardHeader,
  CardTitle,
} from '@/components/ui/card';
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
import { Edit, MoreHorizontal, Trash, Workflow } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { t, plural } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import type { PipelinePreset } from '@/server/functions/pipeline';
import {
  getStepBadgeColor,
  getStepIcon,
} from '@/components/pipeline/constants';
import {
  getJobPresetName,
  getPipelinePresetDescription,
  getPipelinePresetName,
} from '../presets/default-presets-i18n';

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
    <Card className="relative h-full flex flex-col transition-all duration-500 hover:-translate-y-1 hover:shadow-2xl hover:shadow-primary/10 group overflow-hidden bg-gradient-to-br from-background/80 to-background/40 backdrop-blur-xl border-border/40 hover:border-primary/20">
      <div className="absolute inset-x-0 top-0 h-0.5 bg-gradient-to-r from-transparent via-primary/40 to-transparent opacity-0 group-hover:opacity-100 transition-opacity duration-700" />

      {/* Hover Glow Effect */}
      <div className="absolute -inset-0.5 bg-gradient-to-br from-primary/5 to-transparent opacity-0 group-hover:opacity-100 blur-2xl transition-opacity duration-500 pointer-events-none" />

      <CardHeader className="relative flex flex-row items-center gap-4 pb-2 space-y-0 z-10">
        <div className="p-3 rounded-2xl bg-primary/10 text-primary ring-1 ring-inset ring-black/5 dark:ring-white/5 transition-transform duration-500 group-hover:scale-110 group-hover:rotate-3">
          <Workflow className="h-5 w-5" />
        </div>
        <div className="flex-1 min-w-0 space-y-1">
          <CardTitle className="text-base font-medium truncate tracking-tight text-foreground/90 group-hover:text-primary transition-colors duration-300">
            {getPipelinePresetName(workflow, i18n)}
          </CardTitle>
          <div className="flex items-center gap-2">
            <span className="text-[10px] uppercase tracking-wider font-semibold text-muted-foreground/60">
              {t(
                i18n,
              )`${plural(steps.length, { one: '# step', other: '# steps' })}`}
            </span>
          </div>
        </div>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button
              variant="ghost"
              size="icon"
              className="h-8 w-8 -mr-2 text-muted-foreground/40 hover:text-foreground transition-colors"
            >
              <MoreHorizontal className="h-4 w-4" />
              <span className="sr-only">
                <Trans>Open menu</Trans>
              </span>
            </Button>
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
                      This will permanently delete the workflow "{workflow.name}
                      ". Streamers using this workflow will need to be updated.
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
      </CardHeader>
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
            const stepName =
              step.type === 'inline' ? step.processor : step.name;
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
                  <span className="text-[9px] font-mono opacity-50 mr-0.5 border-r border-current border-opacity-20 pr-1 leading-none h-2.5 flex items-center">
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
      <CardFooter className="relative pt-0 text-[10px] text-muted-foreground flex justify-between items-center z-10 border-t border-border/20 mt-auto px-6 py-3 bg-muted/5">
        <span className="font-mono opacity-50">#{workflow.id.slice(0, 8)}</span>
      </CardFooter>
    </Card>
  );
}
