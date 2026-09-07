import { memo, useCallback, useMemo, useRef } from 'react';
import { UseFormReturn, useWatch } from 'react-hook-form';
import { PipelineWorkflowEditor } from '@/components/pipeline/workflows/pipeline-workflow-editor';
import { DagStepDefinition, DagPipelineDefinition } from '@/api/schemas';

interface PipelineConfigAdapterProps {
  form: UseFormReturn<any>;
  /** Field holding the DAG definition. */
  name: string;
  /** `json` fields keep the DAG as a serialized string; `object` fields hold it as-is. */
  mode?: 'json' | 'object';
  /** Name stored inside the DAG definition when steps are written back. */
  dagName?: string;
}

/** Shared empty result so a field without steps keeps a stable identity across renders. */
const NO_STEPS: DagStepDefinition[] = [];

/** Reads a field holding either a DAG object or its serialized form. */
function readSteps(value: unknown): DagStepDefinition[] {
  if (!value) return NO_STEPS;

  let parsed: unknown = value;
  if (typeof value === 'string') {
    try {
      parsed = JSON.parse(value);
    } catch (error) {
      console.warn('Invalid Pipeline data', error);
      return NO_STEPS;
    }
  }

  const steps = (parsed as DagPipelineDefinition | null)?.steps;
  return Array.isArray(steps) ? steps : NO_STEPS;
}

/**
 * Connects a form field holding a DAG definition to the step editor.
 *
 * The steps are derived from the field instead of mirrored into local state, so a form reset or a
 * freshly loaded config shows up immediately and the editor can never drift from the value that
 * gets submitted. Clearing the last step writes `null`, which the config schemas read as "not
 * configured" rather than "configured with no steps".
 */
export const PipelineConfigAdapter = memo(
  ({
    form,
    name,
    mode = 'object',
    dagName = 'pipeline',
  }: PipelineConfigAdapterProps) => {
    const fieldValue = useWatch({ control: form.control, name });
    const lastEmitted = useRef<{
      serialized: string;
      steps: DagStepDefinition[];
    } | null>(null);

    const steps = useMemo(() => {
      const nextSteps = readSteps(fieldValue);
      const emitted = lastEmitted.current;
      // A `json` field parses back into new objects on every edit, and an `object` field can be
      // rewritten with equal contents. Reusing the emitted array in those cases lets the editor
      // keep its own step identities; only a genuinely different value replaces them.
      return emitted && JSON.stringify(nextSteps) === emitted.serialized
        ? emitted.steps
        : nextSteps;
    }, [fieldValue]);

    const handleChange = useCallback(
      (nextSteps: DagStepDefinition[]) => {
        lastEmitted.current = {
          serialized: JSON.stringify(nextSteps),
          steps: nextSteps,
        };

        const dagConfig: DagPipelineDefinition = {
          name: dagName,
          steps: nextSteps,
        };
        const value = mode === 'json' ? JSON.stringify(dagConfig) : dagConfig;

        form.setValue(name, nextSteps.length > 0 ? value : null, {
          shouldDirty: true,
          shouldTouch: true,
          shouldValidate: true,
        });
      },
      [dagName, form, mode, name],
    );

    return <PipelineWorkflowEditor steps={steps} onChange={handleChange} />;
  },
);

PipelineConfigAdapter.displayName = 'PipelineConfigAdapter';
