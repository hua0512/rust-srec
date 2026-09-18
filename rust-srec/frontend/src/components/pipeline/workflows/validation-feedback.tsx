import type { I18n } from '@lingui/core';
import { msg } from '@lingui/core/macro';
import { CheckCircle2 } from 'lucide-react';
import { toast } from 'sonner';

import type { DagPipelineDefinition } from '@/api/schemas';
import { validateDagDefinition } from '@/server/functions/pipeline';

/** The part of the backend's validation response the feedback needs. */
export interface DagValidationResult {
  valid: boolean;
  errors: string[];
  warnings: string[];
  max_depth: number;
}

function IssueList({
  issues,
  tone,
}: {
  issues: string[];
  tone: 'error' | 'warning';
}) {
  if (issues.length === 0) return null;
  return (
    <div className="space-y-1 mt-1">
      {issues.map((issue, index) => (
        <p
          key={index}
          className={
            tone === 'error'
              ? 'text-xs font-mono bg-destructive/10 p-1 rounded text-destructive'
              : 'text-xs font-mono bg-yellow-500/10 p-1 rounded text-yellow-500'
          }
        >
          {issue}
        </p>
      ))}
    </div>
  );
}

/**
 * Shows a validation result as toasts and reports whether the workflow is
 * valid. Errors get an error toast that lists them together with any
 * warnings. A valid result with warnings gets a warning toast, and a clean one
 * a success toast only when `announceSuccess` is set: the save flow has its
 * own confirmation and should not stack two toasts.
 */
export function reportValidation(
  i18n: I18n,
  result: DagValidationResult,
  options: { announceSuccess: boolean },
): boolean {
  if (!result.valid) {
    toast.error(i18n._(msg`Validation Failed`), {
      description: (
        <>
          <IssueList issues={result.errors} tone="error" />
          <IssueList issues={result.warnings} tone="warning" />
        </>
      ),
      duration: 5000,
    });
    return false;
  }
  if (result.warnings.length > 0) {
    toast.warning(i18n._(msg`Pipeline is valid, with warnings`), {
      description: <IssueList issues={result.warnings} tone="warning" />,
      duration: 8000,
    });
  } else if (options.announceSuccess) {
    toast.success(i18n._(msg`Pipeline is valid`), {
      description: i18n._(msg`No errors found. Max depth: ${result.max_depth}`),
      icon: <CheckCircle2 className="h-4 w-4 text-green-500" />,
    });
  }
  return true;
}

/**
 * Validates a workflow with the backend before it is saved: errors block the
 * save and warnings are shown while the save proceeds. When the validation
 * service itself is unavailable the save goes ahead, because the backend
 * rejects an unrunnable definition on save anyway.
 */
export async function validateBeforeSave(
  i18n: I18n,
  dag: DagPipelineDefinition,
  validate: (dag: DagPipelineDefinition) => Promise<DagValidationResult> = (
    definition,
  ) => validateDagDefinition({ data: definition }),
): Promise<boolean> {
  let result: DagValidationResult;
  try {
    result = await validate(dag);
  } catch (error) {
    console.error('Workflow validation unavailable before save:', error);
    return true;
  }
  return reportValidation(i18n, result, { announceSuccess: false });
}
