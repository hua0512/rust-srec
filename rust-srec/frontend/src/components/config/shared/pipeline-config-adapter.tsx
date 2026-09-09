import { useCallback, useMemo, useRef } from 'react';
import { useWatch } from 'react-hook-form';
import type { FieldValues, Path, UseFormReturn } from 'react-hook-form';
import { PipelineWorkflowEditor } from '@/components/pipeline/workflows/pipeline-workflow-editor';
import { DagStepDefinition, DagPipelineDefinition } from '@/api/schemas';
import { genericMemo } from '@/lib/generic-component';
import { setConfigValue } from './form-path';

interface PipelineConfigAdapterProps<TFieldValues extends FieldValues> {
  form: UseFormReturn<TFieldValues>;
  /** Field holding the DAG definition. */
  name: Path<TFieldValues>;
  /** `json` fields keep the DAG as a serialized string; `object` fields hold it as-is. */
  mode?: 'json' | 'object';
  /** Name stored inside the DAG definition when steps are written back. */
  dagName?: string;
  /**
   * What to write once the last step is removed: `null` to clear the field, or `dag` for an
   * empty DAG definition. See the note on the component.
   */
  emptyValue?: 'null' | 'dag';
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
 * gets submitted.
 *
 * Removing the last step has two valid representations and the caller picks one through
 * `emptyValue`. Override fields (template, platform, streamer) clear to `null`, which their
 * schemas read as "inherit" rather than "configured with no steps". The global settings update is
 * a partial one whose fields cannot tell `null` apart from "field omitted", so a `null` there
 * would leave the stored pipeline untouched; those fields send an empty DAG definition instead.
 */
function PipelineConfigAdapterImpl<TFieldValues extends FieldValues>({
  form,
  name,
  mode = 'object',
  dagName = 'pipeline',
  emptyValue = 'null',
}: PipelineConfigAdapterProps<TFieldValues>) {
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
      const value =
        nextSteps.length === 0 && emptyValue === 'null'
          ? null
          : mode === 'json'
            ? JSON.stringify(dagConfig)
            : dagConfig;

      setConfigValue(form, name, value, {
        shouldDirty: true,
        shouldTouch: true,
        shouldValidate: true,
      });
    },
    [dagName, emptyValue, form, mode, name],
  );

  return <PipelineWorkflowEditor steps={steps} onChange={handleChange} />;
}

export const PipelineConfigAdapter = genericMemo(
  PipelineConfigAdapterImpl,
  'PipelineConfigAdapter',
);
